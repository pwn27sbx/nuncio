package ipc

import (
	"errors"
	"slices"
	"testing"
)

// MarkRead on fakeHandler records each call's jid and answers markReadTotal, or markReadErr.
func (f *fakeHandler) MarkRead(jid string) (int, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.markReads = append(f.markReads, jid)
	return f.markReadTotal, f.markReadErr
}

func TestServeConnMarkReadAnswersMarkedReadWithTheNewTotal(t *testing.T) {
	h := newFakeHandler(t)
	h.markReadTotal = 17
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "mark_read_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "marked_read_response.json"))
	if got, want := h.calls(&h.markReads), []string{"15551230001@s.whatsapp.net"}; !slices.Equal(got, want) {
		t.Errorf("handler mark reads = %q, want %q", got, want)
	}
}

func TestServeConnMarkReadErrorsKeepServing(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	for _, frame := range []string{
		`{"v":1,"op":"mark_read","jid":"status@broadcast"}`,
		`{"v":1,"op":"mark_read"}`,
		`{"v":1,"op":"mark_read","jid":15551230001}`,
	} {
		sendRaw(t, conn, frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Errorf("%s -> code %v, want bad_frame", frame, got)
		}
	}
	if got := h.calls(&h.markReads); len(got) != 0 {
		t.Errorf("handler mark reads = %q, want none for refused frames", got)
	}

	h.mu.Lock()
	h.markReadErr = ErrNotLinked
	h.mu.Unlock()
	sendRaw(t, conn, string(readFixture(t, "mark_read_request.json")))
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeNotLinked {
		t.Errorf("mark_read while unlinked -> code %v, want not_linked", got)
	}
	offline := errors.New("the WhatsApp session is offline, so the chat was not marked read")
	h.mu.Lock()
	h.markReadErr = offline
	h.mu.Unlock()
	sendRaw(t, conn, string(readFixture(t, "mark_read_request.json")))
	reply := recvRaw(t, br)
	if fieldOf(t, reply, "code") != CodeInternal || fieldOf(t, reply, "message") != offline.Error() {
		t.Errorf("mark_read offline -> %s, want internal with the reason", reply)
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Errorf("after mark_read errors op = %v, want status", got)
	}
}

// A Handler written before mark_read still compiles and serves; mark_read is a bad_frame for it.
func TestServeConnMarkReadWithoutAMarkReaderIsABadFrame(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, struct{ Handler }{h})
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "mark_read_request.json")))
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
		t.Errorf("mark_read on a handler without MarkRead -> code %v, want bad_frame", got)
	}
	sendRaw(t, conn, string(readFixture(t, "messages_request.json")))
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpMessages {
		t.Errorf("messages after it -> op %v, want messages (unchanged)", got)
	}
}
