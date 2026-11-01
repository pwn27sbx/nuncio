package ipc

import (
	"bufio"
	"encoding/json"
	"errors"
	"testing"
)

// errLinkTestBusy is what a handler answers while another flow runs.
var errLinkTestBusy = errors.New("a link flow is already running")

// linkHandler is a fakeHandler that also carries the link flow, so the dispatch can be tested
// without a daemon and without a WhatsApp client.
type linkHandler struct {
	fakeHandler
	started  []bool
	cancels  int
	startErr error
}

func (h *linkHandler) LinkStart(consent bool) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.started = append(h.started, consent)
	return h.startErr
}

func (h *linkHandler) LinkCancel() error {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.cancels++
	return nil
}

// startedSoFar and cancelsSoFar read the handler's record under its own mutex, so the test never
// depends on the socket round-trip for ordering.
func (h *linkHandler) startedSoFar() []bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]bool(nil), h.started...)
}

func (h *linkHandler) cancelsSoFar() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.cancels
}

func newLinkHandler() *linkHandler {
	return &linkHandler{fakeHandler: fakeHandler{messages: map[string][]Message{}}}
}

// assertLinkRefused reads the next frame and checks it is the error a refused link frame gets,
// without asking the handler anything.
func assertLinkRefused(t *testing.T, br *bufio.Reader, h *linkHandler) {
	t.Helper()
	var e Error
	if err := json.Unmarshal(recvRaw(t, br), &e); err != nil {
		t.Fatal(err)
	}
	if e.Code != CodeBadFrame {
		t.Errorf("error code = %q, want %q", e.Code, CodeBadFrame)
	}
	if started, cancels := h.startedSoFar(), h.cancelsSoFar(); len(started) != 0 || cancels != 0 {
		t.Errorf("the refused frame reached the handler (%d starts, %d cancels)", len(started), cancels)
	}
}

// A link_start without the user's explicit consent never reaches the handler: the daemon generates
// no QR and creates nothing. The same goes for the absent value (Go's zero value).
func TestServeConnRefusesLinkStartWithoutConsent(t *testing.T) {
	for name, frame := range map[string]string{
		"false":  `{"v":1,"op":"link_start","consent":false}`,
		"absent": `{"v":1,"op":"link_start"}`,
	} {
		t.Run(name, func(t *testing.T) {
			h := newLinkHandler()
			conn, br := rawConn(t, startServer(t, h))
			handshake(t, conn, br)
			sendRaw(t, conn, frame)
			assertLinkRefused(t, br, h)
		})
	}
}

// With consent the flow starts and the reply is the frame the spec defines.
func TestServeConnStartsTheLinkFlowWithConsent(t *testing.T) {
	h := newLinkHandler()
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"link_start","consent":true}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "link_started_response.json"))
	if started := h.startedSoFar(); len(started) != 1 || !started[0] {
		t.Errorf("handler started = %v, want one start with consent", started)
	}
}

// link_cancel stops the flow and is answered as the spec says.
func TestServeConnCancelsTheLinkFlow(t *testing.T) {
	h := newLinkHandler()
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"link_cancel"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "link_cancelled_response.json"))
	if cancels := h.cancelsSoFar(); cancels != 1 {
		t.Errorf("handler cancels = %d, want 1", cancels)
	}
}

// A daemon that cannot link refuses both frames instead of dropping the connection.
func TestServeConnRefusesLinkFramesWithoutTheFlow(t *testing.T) {
	h := &fakeHandler{messages: map[string][]Message{}}
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	for _, frame := range []string{`{"v":1,"op":"link_start","consent":true}`, `{"v":1,"op":"link_cancel"}`} {
		sendRaw(t, conn, frame)
		var e Error
		if err := json.Unmarshal(recvRaw(t, br), &e); err != nil {
			t.Fatal(err)
		}
		if e.Code != CodeBadFrame {
			t.Errorf("%s answered code %q, want %q", frame, e.Code, CodeBadFrame)
		}
	}
}

// A malformed link_start is refused before the handler sees it, and the connection keeps serving.
func TestServeConnRefusesAMalformedLinkStart(t *testing.T) {
	h := newLinkHandler()
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"link_start","consent":"yes"}`)
	assertLinkRefused(t, br, h)
	sendRaw(t, conn, `{"v":1,"op":"link_start","consent":true}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "link_started_response.json"))
}

// A flow the handler refuses answers an error frame and the connection keeps serving.
func TestServeConnAnswersAFailedLinkStartWithAnError(t *testing.T) {
	h := newLinkHandler()
	h.startErr = errLinkTestBusy
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"link_start","consent":true}`)
	var e Error
	if err := json.Unmarshal(recvRaw(t, br), &e); err != nil {
		t.Fatal(err)
	}
	if e.Op != OpError || e.Code != CodeInternal || e.Message != errLinkTestBusy.Error() {
		t.Errorf("answer = %+v, want an internal error with the refusal's text", e)
	}
	sendRaw(t, conn, `{"v":1,"op":"link_cancel"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "link_cancelled_response.json"))
}

// The one sentence a failed flow shows is the one the tracked fixture carries: a client that pins
// the fixture shows exactly what the daemon sends.
func TestTheLinkFailureSentenceIsTheFixture(t *testing.T) {
	var ev EventLinkState
	if err := json.Unmarshal(readFixture(t, "event_link_state_failed.json"), &ev); err != nil {
		t.Fatal(err)
	}
	if ev.State != LinkFailed {
		t.Fatalf("fixture state = %q, want %q", ev.State, LinkFailed)
	}
	if ev.Reason != LinkFailureReason {
		t.Errorf("LinkFailureReason = %q, want the fixture's %q", LinkFailureReason, ev.Reason)
	}
}

// A QR matrix is at most 177 modules per side (version 40): a frame never claims more.
func TestTheLargestLinkQRIsTheWireMaximum(t *testing.T) {
	if MaxLinkQRSize != 177 {
		t.Errorf("MaxLinkQRSize = %d, want 177", MaxLinkQRSize)
	}
}
