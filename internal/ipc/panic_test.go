package ipc

import (
	"bufio"
	"bytes"
	"io"
	"log"
	"net"
	"os"
	"strings"
	"sync"
	"testing"
	"time"
)

// panicOnEventConn is a connection whose write of an event frame panics with a value that quotes a
// number, as a bug in the event path would.
type panicOnEventConn struct{ net.Conn }

func (c panicOnEventConn) Write(p []byte) (int, error) {
	if bytes.Contains(p, []byte(`"op":"event_`)) {
		panic("cannot write the event of 15551230001")
	}
	return c.Conn.Write(p)
}

type syncBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *syncBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *syncBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

// A panic while an event is written to one subscriber drops only that subscriber: its connection
// closes, another subscriber still receives the event, and the log names the panic by its type only.
func TestAPanicWritingAnEventDropsOnlyThatSubscriber(t *testing.T) {
	logs := &syncBuffer{}
	log.SetOutput(logs)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })

	h := newFakeHandler(t)
	b := NewBroker()
	srv, cli := net.Pipe()
	t.Cleanup(func() { _ = cli.Close() })
	done := make(chan error, 1)
	go func() { done <- ServeConnEvents(panicOnEventConn{srv}, h, b) }()
	if err := cli.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatal(err)
	}
	br := bufio.NewReader(cli)
	handshake(t, cli, br)
	subscribe(t, cli, br)

	_, goodBr := subscribed(t, startEventServer(t, h, b))

	publish(t, b, eventMessage("A1", "hi"))
	if got := fieldOf(t, recvRaw(t, goodBr), "op"); got != OpEventMessage {
		t.Fatalf("good subscriber op = %v, want %s", got, OpEventMessage)
	}
	if rest, err := br.ReadString('\n'); err != io.EOF {
		t.Errorf("panicking subscriber read %q, %v; want its connection closed", rest, err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("the panicking subscriber's connection is still served")
	}

	publish(t, b, eventMessage("A2", "again"))
	if got := fieldOf(t, recvRaw(t, goodBr), "op"); got != OpEventMessage {
		t.Fatalf("good subscriber second op = %v, want %s", got, OpEventMessage)
	}

	want := "ipc: an event write panicked (string); its subscriber was dropped"
	deadline := time.Now().Add(5 * time.Second)
	for !strings.Contains(logs.String(), want) && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	got := logs.String()
	if !strings.Contains(got, want) {
		t.Errorf("log = %q, want %q", got, want)
	}
	if strings.Contains(got, "15551230001") || strings.Contains(got, "cannot write") {
		t.Errorf("log = %q carries the panic's text", got)
	}
}
