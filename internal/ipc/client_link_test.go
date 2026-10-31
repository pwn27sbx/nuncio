package ipc

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// startServerWithEvents serves h at a socket with a broker, so a test can drive a subscribed
// connection.
func startServerWithEvents(t *testing.T, h Handler, events *Broker) string {
	t.Helper()
	sock := filepath.Join(shortTempDir(t), "core.sock")
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
			go func() { _ = ServeConnEvents(conn, h, events) }()
		}
	}()
	return sock
}

// The CLI's client asks for the flow with the user's yes and stops it the same way the wire says.
func TestClientLinkStartAndCancelReachTheHandler(t *testing.T) {
	h := newLinkHandler()
	c, err := Dial(startServer(t, h), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })

	started, err := c.LinkStart(true)
	if err != nil || started != (LinkStarted{V: 1, Op: OpLinkStarted}) {
		t.Errorf("LinkStart(true) = %+v, %v; want link_started", started, err)
	}
	if got := h.startedSoFar(); len(got) != 1 || !got[0] {
		t.Errorf("handler saw starts %v, want [true]", got)
	}

	cancelled, err := c.LinkCancel()
	if err != nil || cancelled != (LinkCancelled{V: 1, Op: OpLinkCancelled}) {
		t.Errorf("LinkCancel() = %+v, %v; want link_cancelled", cancelled, err)
	}
	if got := h.cancelsSoFar(); got != 1 {
		t.Errorf("handler saw %d cancels, want 1", got)
	}
}

// A client that asks without consent gets the daemon's refusal, and the flow never starts.
func TestClientLinkStartWithoutConsentIsTheDaemonsRefusal(t *testing.T) {
	h := newLinkHandler()
	c, err := Dial(startServer(t, h), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })

	_, err = c.LinkStart(false)
	var perr *ProtocolError
	if !errors.As(err, &perr) || perr.Code != CodeBadFrame {
		t.Fatalf("LinkStart(false) = %v, want a %s error", err, CodeBadFrame)
	}
	if got := h.startedSoFar(); len(got) != 0 {
		t.Errorf("the refused frame reached the handler: %v", got)
	}
}

// A subscribed client reads the flow's frames off the wire, in the order the daemon published them.
func TestClientSubscribeAndNextEventCarryTheLinkFlow(t *testing.T) {
	h := newLinkHandler()
	events := NewBroker()
	c, err := Dial(startServerWithEvents(t, h, events), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	if err := c.Subscribe(); err != nil {
		t.Fatalf("Subscribe: %v", err)
	}

	qr := EventLinkQR{V: 1, Op: OpLinkQR, Size: 2, Rows: []string{"10", "01"}}
	state := EventLinkState{V: 1, Op: OpLinkState, State: LinkFailed, Reason: LinkFailureReason}
	if queued := events.Publish(qr); queued != 1 {
		t.Fatalf("Publish(qr) reached %d subscribers, want 1", queued)
	}
	events.Publish(state)

	op, frame, err := c.NextEvent()
	if err != nil || op != OpLinkQR {
		t.Fatalf("NextEvent() = %q, %v; want a %s frame", op, err, OpLinkQR)
	}
	var got EventLinkQR
	if err := json.Unmarshal(frame, &got); err != nil || got.Size != qr.Size || fmt.Sprint(got.Rows) != fmt.Sprint(qr.Rows) {
		t.Errorf("the qr frame decoded to %+v, %v; want %+v", got, err, qr)
	}

	op, frame, err = c.NextEvent()
	if err != nil || op != OpLinkState {
		t.Fatalf("NextEvent() = %q, %v; want a %s frame", op, err, OpLinkState)
	}
	var gotState EventLinkState
	if err := json.Unmarshal(frame, &gotState); err != nil || gotState != state {
		t.Errorf("the state frame decoded to %+v, %v; want %+v", gotState, err, state)
	}
}

// A quiet subscription is not an error: the read bound is the caller's to choose, and the frames
// that came before still decode.
func TestClientNextEventTimesOutOnAQuietConnection(t *testing.T) {
	h := newLinkHandler()
	c, err := DialTimeout(startServerWithEvents(t, h, NewBroker()), "test/0.1.0", 100*time.Millisecond)
	if err != nil {
		t.Fatalf("DialTimeout: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	if err := c.Subscribe(); err != nil {
		t.Fatalf("Subscribe: %v", err)
	}
	if _, _, err := c.NextEvent(); !errors.Is(err, os.ErrDeadlineExceeded) {
		t.Errorf("NextEvent on a quiet connection = %v, want the read bound", err)
	}
}
