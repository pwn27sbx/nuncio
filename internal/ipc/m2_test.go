package ipc

import (
	"encoding/json"
	"errors"
	"slices"
	"strings"
	"testing"
)

// The M2 ops on fakeHandler: searches records "jid limit query", replies "jid client_id reply_to
// text" and reacts "jid id emoji".

func (f *fakeHandler) Search(jid, query string, limit int) ([]Message, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.searches = append(f.searches, jid+" "+itoa(limit)+" "+query)
	return f.found, f.searchErr
}

func (f *fakeHandler) SendReply(jid, text, clientID, replyTo string) (Sent, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.replies = append(f.replies, jid+" "+clientID+" "+replyTo+" "+text)
	return f.sent, f.sendErr
}

func (f *fakeHandler) React(jid, id, emoji string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.reacts = append(f.reacts, jid+" "+id+" "+emoji)
	return f.reactErr
}

func (f *fakeHandler) calls(list *[]string) []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), *list...)
}

func itoa(n int) string { b, _ := json.Marshal(n); return string(b) }

func TestServeConnSearchAnswersMessagesFromTheHandler(t *testing.T) {
	h := newFakeHandler(t)
	var resp MessagesResponse
	if err := json.Unmarshal(readFixture(t, "search_response.json"), &resp); err != nil {
		t.Fatal(err)
	}
	h.found = resp.Messages
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "search_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "search_response.json"))
	if got, want := h.calls(&h.searches), []string{"15551230001@s.whatsapp.net 50 venue"}; !slices.Equal(got, want) {
		t.Errorf("handler searches = %q, want %q", got, want)
	}
}

func TestServeConnRefusesABadSearchAndKeepsServing(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	for _, frame := range []string{
		`{"v":1,"op":"search","jid":"15551230001@s.whatsapp.net","query":"   ","limit":10}`,
		`{"v":1,"op":"search","jid":"15551230001@s.whatsapp.net","limit":10}`,
		`{"v":1,"op":"search","jid":"15551230001@s.whatsapp.net","query":"x","limit":0}`,
		`{"v":1,"op":"search","jid":"15551230001@s.whatsapp.net","query":"x","limit":201}`,
		`{"v":1,"op":"search","jid":"status@broadcast","query":"x","limit":10}`,
		`{"v":1,"op":"search","jid":"15551230001@s.whatsapp.net","query":"` + strings.Repeat("a", MaxQueryLen+1) + `","limit":10}`,
	} {
		sendRaw(t, conn, frame)
		reply := recvRaw(t, br)
		if fieldOf(t, reply, "code") != CodeBadFrame {
			t.Errorf("%s -> %s, want bad_frame", frame[:60], reply)
		}
	}
	if got := h.calls(&h.searches); len(got) != 0 {
		t.Errorf("handler searches = %q, want none", got)
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Errorf("after bad searches op = %v, want status", got)
	}
}

func TestServeConnSendWithReplyToGoesToSendReply(t *testing.T) {
	h := newFakeHandler(t)
	h.sent = Sent{State: SendQueued}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "send_reply_request.json")))
	if got := fieldOf(t, recvRaw(t, br), "state"); got != SendQueued {
		t.Errorf("state = %v, want queued", got)
	}
	want := []string{"15551230001@s.whatsapp.net c-7f3a9e22 3EB0AAA0 Which venue?"}
	if got := h.calls(&h.replies); !slices.Equal(got, want) {
		t.Errorf("handler replies = %q, want %q", got, want)
	}
	if got := h.sendCalls(); len(got) != 0 {
		t.Errorf("plain sends = %q, want none", got)
	}
}

// A send without reply_to keeps calling Send, exactly as in M1.
func TestServeConnSendWithoutReplyToStaysAPlainSend(t *testing.T) {
	h := newFakeHandler(t)
	h.sent = Sent{State: SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "send_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "sent_response.json"))
	if got := h.calls(&h.replies); len(got) != 0 {
		t.Errorf("handler replies = %q, want none", got)
	}
	if got := h.sendCalls(); len(got) != 1 {
		t.Errorf("plain sends = %q, want one", got)
	}
}

func TestServeConnReactAnswersReacted(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "react_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "reacted_response.json"))
	sendRaw(t, conn, string(readFixture(t, "react_remove_request.json")))
	assertSameJSON(t, recvRaw(t, br),
		[]byte(`{"v":1,"op":"reacted","jid":"15551230001@s.whatsapp.net","id":"3EB0AAA1","emoji":""}`))
	want := []string{"15551230001@s.whatsapp.net 3EB0AAA1 👍", "15551230001@s.whatsapp.net 3EB0AAA1 "}
	if got := h.calls(&h.reacts); !slices.Equal(got, want) {
		t.Errorf("handler reacts = %q, want %q", got, want)
	}
}

func TestServeConnReactErrors(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	for _, frame := range []string{
		`{"v":1,"op":"react","jid":"15551230001@s.whatsapp.net","id":"","emoji":"x"}`,
		`{"v":1,"op":"react","jid":"nope","id":"3EB0","emoji":"x"}`,
		`{"v":1,"op":"react","jid":"15551230001@s.whatsapp.net","id":"3EB0","emoji":"` + strings.Repeat("a", MaxEmojiLen+1) + `"}`,
	} {
		sendRaw(t, conn, frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Errorf("%s -> code %v, want bad_frame", frame, got)
		}
	}
	h.mu.Lock()
	h.reactErr = ErrNotLinked
	h.mu.Unlock()
	sendRaw(t, conn, string(readFixture(t, "react_request.json")))
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeNotLinked {
		t.Errorf("react while unlinked -> code %v, want not_linked", got)
	}
	offline := errors.New("the WhatsApp session is offline, so the reaction was not sent")
	h.mu.Lock()
	h.reactErr = offline
	h.mu.Unlock()
	sendRaw(t, conn, string(readFixture(t, "react_request.json")))
	reply := recvRaw(t, br)
	if fieldOf(t, reply, "code") != CodeInternal || fieldOf(t, reply, "message") != offline.Error() {
		t.Errorf("react offline -> %s, want internal with the reason", reply)
	}
	if got := h.calls(&h.reacts); len(got) != 2 {
		t.Errorf("handler reacts = %q, want only the two valid frames", got)
	}
}
