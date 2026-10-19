package ipc

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

// fakeHandler is the smallest thing that satisfies Handler.
type fakeHandler struct {
	mu     sync.Mutex
	status Status
	chats  []Chat
	hellos []string
	// messages per chat jid; asked records every Messages call as "jid limit".
	messages map[string][]Message
	asked    []string
}

func (f *fakeHandler) Hello(client string) Status {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.hellos = append(f.hellos, client)
	return f.status
}

func (f *fakeHandler) Status() Status { return f.status }

func (f *fakeHandler) Chats() []Chat { return f.chats }

func (f *fakeHandler) Messages(jid string, limit int) ([]Message, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.asked = append(f.asked, fmt.Sprintf("%s %d", jid, limit))
	msgs := f.messages[jid]
	if len(msgs) > limit {
		msgs = msgs[:limit]
	}
	return msgs, nil
}

func (f *fakeHandler) askedMessages() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.asked...)
}

func (f *fakeHandler) clientNames() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.hellos...)
}

func newFakeHandler(t *testing.T) *fakeHandler {
	t.Helper()
	var resp ChatsResponse
	if err := json.Unmarshal(readFixture(t, "chats_response.json"), &resp); err != nil {
		t.Fatalf("unmarshal chats fixture: %v", err)
	}
	var msgs MessagesResponse
	if err := json.Unmarshal(readFixture(t, "messages_response.json"), &msgs); err != nil {
		t.Fatalf("unmarshal messages fixture: %v", err)
	}
	return &fakeHandler{
		status:   Status{Daemon: "nunciod/0.1.0", Session: SessionUnlinked, Unread: 0},
		chats:    resp.Chats,
		messages: map[string][]Message{msgs.JID: msgs.Messages},
	}
}

// startServer serves h on a private unix socket and returns its path.
func startServer(t *testing.T, h Handler) string {
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
			go func() { _ = ServeConn(conn, h) }()
		}
	}()
	return sock
}

// rawConn dials sock and returns the connection with an independent frame
// reader, so tests can assert the exact bytes the server writes.
func rawConn(t *testing.T, sock string) (net.Conn, *bufio.Reader) {
	t.Helper()
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("dial %s: %v", sock, err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	if err := conn.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatalf("set deadline: %v", err)
	}
	return conn, bufio.NewReader(conn)
}

func sendRaw(t *testing.T, conn net.Conn, frame string) {
	t.Helper()
	if _, err := io.WriteString(conn, frame+"\n"); err != nil {
		t.Fatalf("write %q: %v", frame, err)
	}
}

// recvRaw reads one frame and returns its bytes without the newline.
func recvRaw(t *testing.T, br *bufio.Reader) []byte {
	t.Helper()
	line, err := br.ReadString('\n')
	if err != nil {
		t.Fatalf("read frame: %v", err)
	}
	return []byte(strings.TrimSuffix(line, "\n"))
}

func fieldOf(t *testing.T, frame []byte, key string) any {
	t.Helper()
	raw := decodedJSON(t, frame).(map[string]any)
	v, ok := raw[key]
	if !ok {
		t.Fatalf("frame %s has no %q field", frame, key)
	}
	return v
}

func assertClosed(t *testing.T, br *bufio.Reader) {
	t.Helper()
	if _, err := br.ReadString('\n'); !errors.Is(err, io.EOF) {
		t.Fatalf("read after close: %v, want EOF", err)
	}
}

// assertClosedOrReset is for a close that follows an unwritable protocol
// violation: the peer may see a reset instead of a clean EOF when the daemon
// closes with the oversized frame still unread. What matters is that the
// connection is gone and did not answer with another frame.
func assertClosedOrReset(t *testing.T, br *bufio.Reader) {
	t.Helper()
	_, err := br.ReadString('\n')
	if errors.Is(err, io.EOF) || errors.Is(err, syscall.ECONNRESET) {
		return
	}
	t.Fatalf("read after close: %v, want EOF or ECONNRESET", err)
}

func handshake(t *testing.T, conn net.Conn, br *bufio.Reader) {
	t.Helper()
	sendRaw(t, conn, `{"v":1,"op":"hello","client":"nuncio-tui/0.1.0"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpHelloOK {
		t.Fatalf("handshake op = %v, want %s", got, OpHelloOK)
	}
}

func TestServeConnHappyPathMatchesFixtures(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)

	sendRaw(t, conn, `{"v":1,"op":"hello","client":"nuncio-tui/0.1.0"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "hello_ok_daemon.json"))
	if got := h.clientNames(); len(got) != 1 || got[0] != "nuncio-tui/0.1.0" {
		t.Fatalf("handler saw handshakes %q, want [nuncio-tui/0.1.0]", got)
	}

	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))

	sendRaw(t, conn, `{"v":1,"op":"chats","limit":40}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "chats_response.json"))
}

func TestServeConnAnswersVersionMismatchThenCloses(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	for _, v := range []int{0, 2, 99} {
		t.Run(fmt.Sprintf("v%d", v), func(t *testing.T) {
			conn, br := rawConn(t, sock)
			sendRaw(t, conn, fmt.Sprintf(`{"v":%d,"op":"hello","client":"nuncio-tui/0.1.0"}`, v))
			frame := recvRaw(t, br)
			if got := fieldOf(t, frame, "code"); got != CodeVersionMismatch {
				t.Fatalf("code = %v, want %s", got, CodeVersionMismatch)
			}
			assertSameJSON(t, frame, readFixture(t, "error_version_mismatch.json"))
			assertClosed(t, br)
		})
	}
	if got := h.clientNames(); len(got) != 0 {
		t.Fatalf("handler saw %q, want no handshake for a refused version", got)
	}
}

func TestServeConnVersionMismatchOnLaterFrameCloses(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, `{"v":2,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeVersionMismatch {
		t.Fatalf("code = %v, want %s", got, CodeVersionMismatch)
	}
	assertClosed(t, br)
}

func TestServeConnRequiresHelloFirst(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)

	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	frame := recvRaw(t, br)
	if got := fieldOf(t, frame, "code"); got != CodeBadFrame {
		t.Fatalf("code = %v, want %s", got, CodeBadFrame)
	}
	assertClosed(t, br)
}

func TestServeConnRejectsOutOfRangeLimitAndKeepsServing(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	for _, limit := range []string{"0", "-1", "201"} {
		t.Run("limit "+limit, func(t *testing.T) {
			conn, br := rawConn(t, sock)
			handshake(t, conn, br)

			sendRaw(t, conn, `{"v":1,"op":"chats","limit":`+limit+`}`)
			frame := recvRaw(t, br)
			if got := fieldOf(t, frame, "code"); got != CodeBadFrame {
				t.Fatalf("code = %v, want %s", got, CodeBadFrame)
			}

			// A rejected request must not kill a healthy connection.
			sendRaw(t, conn, `{"v":1,"op":"status"}`)
			assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
		})
	}
}

func TestServeConnAppliesLimitToChatList(t *testing.T) {
	h := newFakeHandler(t)
	h.chats = append(h.chats, Chat{JID: "15551230009@s.whatsapp.net", Name: "Fifth"})
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, `{"v":1,"op":"chats","limit":2}`)
	var resp ChatsResponse
	if err := json.Unmarshal(recvRaw(t, br), &resp); err != nil {
		t.Fatalf("unmarshal chats response: %v", err)
	}
	if len(resp.Chats) != 2 {
		t.Fatalf("got %d chats, want 2", len(resp.Chats))
	}
	if resp.Chats[0].JID != "15551230001@s.whatsapp.net" || resp.Chats[1].JID != "15551230002@s.whatsapp.net" {
		t.Fatalf("chats = %v, want the first two in order", resp.Chats)
	}
}

func TestServeConnRejectsUnknownOpAndKeepsServing(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, `{"v":1,"op":"frobnicate"}`)
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
		t.Fatalf("code = %v, want %s", got, CodeBadFrame)
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
}

func TestServeConnRejectsMalformedJSONAndKeepsServing(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	for _, frame := range []string{"{oops", `{"v":1,`, `"just a string"`, `[1,2,3]`} {
		sendRaw(t, conn, frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Fatalf("frame %q: code = %v, want %s", frame, got, CodeBadFrame)
		}
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
}

func TestServeConnRejectsOversizedFrameThenCloses(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	// A frame over the cap cannot be resynchronised, so the server answers
	// bad_frame and closes.
	if _, err := io.WriteString(conn, `{"v":1,"op":"chats","limit":40,"pad":"`+
		strings.Repeat("a", MaxFrameLen)+`"}`+"\n"); err != nil {
		t.Fatalf("write oversized frame: %v", err)
	}
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
		t.Fatalf("code = %v, want %s", got, CodeBadFrame)
	}
	assertClosedOrReset(t, br)
}

func TestServeConnEndsQuietlyOnClientClose(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, _ := rawConn(t, sock)
	if err := conn.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
}

func TestServeConnServesMessagesOfAChat(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, string(readFixture(t, "messages_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "messages_response.json"))
	if got := h.askedMessages(); len(got) != 1 || got[0] != "15551230001@s.whatsapp.net 50" {
		t.Fatalf("handler was asked %q, want the request's jid and limit", got)
	}
}

func TestServeConnAnswersEmptyMessagesForAnUnknownChat(t *testing.T) {
	sock := startServer(t, newFakeHandler(t))
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	for _, jid := range []string{"15551239999@s.whatsapp.net", "120363000000000099@g.us", "100000000000001@lid"} {
		sendRaw(t, conn, `{"v":1,"op":"messages","jid":"`+jid+`","limit":10}`)
		assertSameJSON(t, recvRaw(t, br), []byte(`{"v":1,"op":"messages","jid":"`+jid+`","messages":[]}`))
	}
}

func TestServeConnRejectsOutOfRangeMessagesLimitAndKeepsServing(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	for _, limit := range []string{"0", "-1", "201"} {
		sendRaw(t, conn, `{"v":1,"op":"messages","jid":"15551230001@s.whatsapp.net","limit":`+limit+`}`)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Fatalf("limit %s: code = %v, want %s", limit, got, CodeBadFrame)
		}
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
	if got := h.askedMessages(); len(got) != 0 {
		t.Fatalf("handler was asked %q for rejected requests, want nothing", got)
	}
}

func TestServeConnRejectsAJIDThatIsNotWhatsAppAndKeepsServing(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	for _, jid := range []string{
		"", "15551230001", "@s.whatsapp.net", "15551230001@example.org",
		"status@broadcast", "15551230001:3@s.whatsapp.net", "ada@s.whatsapp.net",
		"15551230001@s.whatsapp.net@g.us",
	} {
		sendRaw(t, conn, `{"v":1,"op":"messages","jid":"`+jid+`","limit":10}`)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Fatalf("jid %q: code = %v, want %s", jid, got, CodeBadFrame)
		}
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
	if got := h.askedMessages(); len(got) != 0 {
		t.Fatalf("handler was asked %q for rejected requests, want nothing", got)
	}
}
