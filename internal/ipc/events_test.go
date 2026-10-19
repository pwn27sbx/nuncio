package ipc

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// startEventServer serves h on a private unix socket with events fanned out by b.
func startEventServer(t *testing.T, h Handler, b *Broker) string {
	t.Helper()
	sock := filepath.Join(t.TempDir(), "core.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen %s: %v", sock, err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() { _ = ServeConnEvents(conn, h, b) }()
		}
	}()
	return sock
}

// subscribed dials sock, handshakes and subscribes, and returns the connection.
func subscribed(t *testing.T, sock string) (net.Conn, *bufio.Reader) {
	t.Helper()
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	subscribe(t, conn, br)
	return conn, br
}

func subscribe(t *testing.T, conn net.Conn, br *bufio.Reader) {
	t.Helper()
	sendRaw(t, conn, `{"v":1,"op":"subscribe"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "subscribed_response.json"))
}

// publish runs b.Publish and fails the test if it does not return promptly: Publish must never
// block, whatever the subscribers do.
func publish(t *testing.T, b *Broker, frame any) int {
	t.Helper()
	done := make(chan int, 1)
	go func() { done <- b.Publish(frame) }()
	select {
	case n := <-done:
		return n
	case <-time.After(2 * time.Second):
		t.Fatalf("Publish(%T) blocked for 2s", frame)
		return 0
	}
}

func eventMessage(id, text string) EventMessage {
	return EventMessage{V: Version, Op: OpEventMessage, JID: "15551230001@s.whatsapp.net", Message: Message{
		ID: id, Kind: KindText, Text: text, Timestamp: "2026-10-18T17:58:00Z",
	}}
}

// assertNoFrame fails if a frame arrives on conn within a short wait.
func assertNoFrame(t *testing.T, conn net.Conn, br *bufio.Reader) {
	t.Helper()
	if err := conn.SetReadDeadline(time.Now().Add(200 * time.Millisecond)); err != nil {
		t.Fatalf("set deadline: %v", err)
	}
	line, err := br.ReadString('\n')
	if !errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("read = %q, %v; want no frame before the deadline", line, err)
	}
	if err := conn.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatalf("set deadline: %v", err)
	}
}

func TestTwoSubscribersBothReceiveAnEvent(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	a, ar := subscribed(t, sock)
	_, cr := subscribed(t, sock)

	if n := publish(t, b, eventMessage("3EB0AAA3", "Running late")); n != 2 {
		t.Errorf("Publish delivered to %d subscribers, want 2", n)
	}
	want := readFixture(t, "event_message.json")
	assertSameJSON(t, recvRaw(t, ar), want)
	assertSameJSON(t, recvRaw(t, cr), want)

	// Replies still work on a subscribed connection, interleaved with events.
	sendRaw(t, a, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, ar), readFixture(t, "status_response.json"))
}

func TestEveryEventKindReachesASubscriberAsItsFixture(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	_, br := subscribed(t, sock)

	frames := []struct {
		frame   any
		fixture string
	}{
		{eventMessage("3EB0AAA3", "Running late"), "event_message.json"},
		{EventConnection{V: Version, Op: OpEventConnection, Session: SessionOffline}, "event_connection.json"},
		{EventUnread{V: Version, Op: OpEventUnread, Unread: 20, JID: "15551230001@s.whatsapp.net", ChatUnread: 3}, "event_unread.json"},
		{EventFocus{V: Version, Op: OpEventFocus, JID: "15551230002@s.whatsapp.net"}, "event_focus.json"},
	}
	for _, f := range frames {
		publish(t, b, f.frame)
		assertSameJSON(t, recvRaw(t, br), readFixture(t, f.fixture))
	}
}

func TestAConnectionThatDidNotSubscribeReceivesNoEvents(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	// A subscriber exists, so the event is really published.
	_, sr := subscribed(t, sock)

	if n := publish(t, b, eventMessage("3EB0AAA3", "Running late")); n != 1 {
		t.Errorf("Publish delivered to %d subscribers, want 1", n)
	}
	assertSameJSON(t, recvRaw(t, sr), readFixture(t, "event_message.json"))
	assertNoFrame(t, conn, br)
	// The next frame it reads is the reply to its own request.
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
}

func TestSubscribingTwiceDeliversEachEventOnce(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	conn, br := subscribed(t, sock)
	subscribe(t, conn, br)

	if n := publish(t, b, eventMessage("E1", "one")); n != 1 {
		t.Errorf("Publish delivered to %d subscribers, want 1", n)
	}
	publish(t, b, eventMessage("E2", "two"))
	for _, id := range []string{"E1", "E2"} {
		got := fieldOf(t, recvRaw(t, br), "message").(map[string]any)["id"]
		if got != id {
			t.Fatalf("event id = %v, want %s", got, id)
		}
	}
	assertNoFrame(t, conn, br)
}

func TestAStalledSubscriberIsDroppedAndBlocksNobody(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	stalled, stalledR := subscribed(t, sock)
	_, healthyR := subscribed(t, sock)
	plain, plainR := rawConn(t, sock)
	handshake(t, plain, plainR)

	// Large frames fill the stalled peer's socket buffers quickly; enough of them to overflow its
	// queue whatever the kernel buffers.
	text := strings.Repeat("x", 16*1024)
	const events = 4 * SubscriberQueue
	started := time.Now()
	for i := range events {
		publish(t, b, eventMessage(fmt.Sprintf("E%d", i), text))
		got := fieldOf(t, recvRaw(t, healthyR), "message").(map[string]any)["id"]
		if want := fmt.Sprintf("E%d", i); got != want {
			t.Fatalf("healthy subscriber got event %v, want %s", got, want)
		}
	}
	if elapsed := time.Since(started); elapsed > 5*time.Second {
		t.Errorf("publishing %d events took %v", events, elapsed)
	}

	// Another connection is still served.
	sendRaw(t, plain, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, plainR), readFixture(t, "status_response.json"))

	// The stalled subscriber was dropped: once it reads again it finds what was buffered, then
	// the end of the connection, not an endless stream nor a hang.
	if n := publish(t, b, eventMessage("after", "x")); n != 1 {
		t.Errorf("after the drop Publish delivered to %d subscribers, want 1 (the healthy one)", n)
	}
	if err := stalled.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatalf("set deadline: %v", err)
	}
	read := 0
	for {
		_, err := stalledR.ReadString('\n')
		if err == nil {
			read++
			continue
		}
		if errors.Is(err, io.EOF) || errors.Is(err, syscall.ECONNRESET) {
			break
		}
		t.Fatalf("stalled subscriber read %d frames, then %v; want the connection closed", read, err)
	}
	if read >= events {
		t.Errorf("stalled subscriber read %d frames, want fewer than the %d published", read, events)
	}
}

func TestPublishWithNoSubscribersDeliversToNone(t *testing.T) {
	if n := NewBroker().Publish(eventMessage("E1", "one")); n != 0 {
		t.Errorf("Publish delivered to %d subscribers, want 0", n)
	}
	var nilBroker *Broker
	if n := nilBroker.Publish(eventMessage("E1", "one")); n != 0 {
		t.Errorf("Publish on a nil broker delivered to %d subscribers, want 0", n)
	}
}

func TestACancelledSubscriptionReceivesNothingMore(t *testing.T) {
	b := NewBroker()
	frames, cancel := b.Subscribe(nil)
	b.Publish(eventMessage("E1", "one"))
	select {
	case got := <-frames:
		if !strings.Contains(string(got), `"E1"`) || !strings.HasSuffix(string(got), "\n") {
			t.Fatalf("first frame = %q, want the E1 event as one NDJSON line", got)
		}
	case <-time.After(time.Second):
		t.Fatal("no frame queued for the subscriber")
	}
	cancel()
	cancel() // idempotent
	if n := b.Publish(eventMessage("E2", "two")); n != 0 {
		t.Errorf("Publish after cancel delivered to %d subscribers, want 0", n)
	}
	if _, open := <-frames; open {
		t.Error("frames is still open after cancel")
	}
}

func TestSubscribeWithoutABrokerIsAnsweredAndTheConnectionKeepsServing(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := subscribed(t, sock)
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
}
