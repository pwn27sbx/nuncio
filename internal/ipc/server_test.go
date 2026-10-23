package ipc

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"slices"
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
	// sends records every Send call as "jid client_id text"; Send answers sent, or sendErr.
	sends   []string
	sent    Sent
	sendErr error
	// retries records every Retry call's client_id; Retry answers sent, or sendErr.
	retries []string
	// wipes records every Wipe call's unlink; Wipe answers wipeErr.
	wipes   []bool
	wipeErr error
	// downloads records every DownloadMedia call as "jid id"; it answers mediaPath and
	// mediaMime, or mediaErr.
	downloads []string
	mediaPath string
	mediaMime string
	mediaErr  error
	// The M2 ops (m2_test.go): Search answers found or searchErr, React answers reactErr.
	searches  []string
	found     []Message
	searchErr error
	replies   []string
	reacts    []string
	reactErr  error
	// mark_read (mark_read_test.go): MarkRead answers markReadTotal or markReadErr.
	markReads     []string
	markReadTotal int
	markReadErr   error
}

func (f *fakeHandler) DownloadMedia(jid, id string) (string, string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.downloads = append(f.downloads, jid+" "+id)
	return f.mediaPath, f.mediaMime, f.mediaErr
}

func (f *fakeHandler) downloadCalls() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.downloads...)
}

func (f *fakeHandler) Wipe(unlink bool) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.wipes = append(f.wipes, unlink)
	return f.wipeErr
}

func (f *fakeHandler) wipeCalls() []bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]bool(nil), f.wipes...)
}

func (f *fakeHandler) Send(jid, text, clientID string) (Sent, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.sends = append(f.sends, jid+" "+clientID+" "+text)
	return f.sent, f.sendErr
}

func (f *fakeHandler) Retry(clientID string) (Sent, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.retries = append(f.retries, clientID)
	return f.sent, f.sendErr
}

func (f *fakeHandler) retryCalls() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.retries...)
}

func (f *fakeHandler) sendCalls() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.sends...)
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

// shortTempDir is a temp dir with a short path, removed at cleanup, for the tests' unix sockets: a
// socket path must fit in sun_path (104 bytes on macOS, 108 on Linux, both with the NUL), and
// t.TempDir carries up to 64 bytes of the test name plus 14 more, too long under a deep $TMPDIR such
// as the one makepkg's check() sets.
func shortTempDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "ni")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}

// startServer serves h on a private unix socket and returns its path.
func startServer(t *testing.T, h Handler) string {
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

// sendFrame is a send request with the given fields, JSON-encoded.
func sendFrame(t *testing.T, jid, text, clientID string) string {
	t.Helper()
	data, err := json.Marshal(SendRequest{V: Version, Op: OpSend, JID: jid, Text: text, ClientID: clientID})
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func TestServeConnRefusesABadSendAndSendsNothing(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	const ada = "15551230001@s.whatsapp.net"
	for _, tc := range []struct{ name, frame string }{
		{"empty text", sendFrame(t, ada, "", "c-1")},
		{"whitespace-only text", sendFrame(t, ada, " \t\n ", "c-1")},
		{"text over 65536 bytes", sendFrame(t, ada, strings.Repeat("a", MaxTextLen+1), "c-1")},
		{"empty client_id", sendFrame(t, ada, "On my way", "")},
		{"jid that is not a chat", sendFrame(t, "status@broadcast", "On my way", "c-1")},
		{"malformed send", `{"v":1,"op":"send","jid":1}`},
	} {
		sendRaw(t, conn, tc.frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Errorf("%s: code = %v, want %s", tc.name, got, CodeBadFrame)
		}
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
	if got := h.sendCalls(); len(got) != 0 {
		t.Errorf("handler was asked to send %q for rejected requests, want nothing", got)
	}
}

func TestServeConnSendsTextOfExactlyTheLimit(t *testing.T) {
	h := newFakeHandler(t)
	h.sent = Sent{State: SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, sendFrame(t, "15551230001@s.whatsapp.net", strings.Repeat("a", MaxTextLen), "c-1"))
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpSent {
		t.Errorf("op = %v, want %s", got, OpSent)
	}
	if got := len(h.sendCalls()); got != 1 {
		t.Errorf("handler sends = %d, want 1", got)
	}
}

func TestServeConnAnswersSentWithTheClientID(t *testing.T) {
	h := newFakeHandler(t)
	h.sent = Sent{State: SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "send_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "sent_response.json"))
	if got, want := h.sendCalls(), []string{"15551230001@s.whatsapp.net c-7f3a9e21 On my way"}; !slices.Equal(got, want) {
		t.Errorf("handler sends = %q, want %q", got, want)
	}
}

func TestServeConnAnswersNotLinkedWhenNoDeviceIsLinked(t *testing.T) {
	h := newFakeHandler(t)
	h.sendErr = ErrNotLinked
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "send_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "error_not_linked.json"))
	// The connection stays usable.
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "status_response.json"))
}

func TestServeConnAnswersSendFailedWithTheReason(t *testing.T) {
	h := newFakeHandler(t)
	h.sendErr = errors.New("the server did not acknowledge the message")
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, sendFrame(t, "15551230001@s.whatsapp.net", "On my way", "c-7f3a9e23"))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "send_failed_response.json"))
}

func TestFocusWithNoSubscribedUIIsDeliveredToNone(t *testing.T) {
	sock := startEventServer(t, newFakeHandler(t), NewBroker())
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "focus_request.json")))
	assertSameJSON(t, recvRaw(t, br), []byte(`{"v":1,"op":"focused","jid":"15551230002@s.whatsapp.net","delivered":0}`))
}

func TestFocusReachesEverySubscribedUI(t *testing.T) {
	sock := startEventServer(t, newFakeHandler(t), NewBroker())
	_, firstR := subscribed(t, sock)
	_, secondR := subscribed(t, sock)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, string(readFixture(t, "focus_request.json")))
	assertSameJSON(t, recvRaw(t, br), []byte(`{"v":1,"op":"focused","jid":"15551230002@s.whatsapp.net","delivered":2}`))
	assertSameJSON(t, recvRaw(t, firstR), readFixture(t, "event_focus.json"))
	assertSameJSON(t, recvRaw(t, secondR), readFixture(t, "event_focus.json"))
	// The asking connection did not subscribe, so it gets no event of its own.
	assertNoFrame(t, conn, br)
}

func TestFocusRejectsAJIDThatIsNotAChat(t *testing.T) {
	b := NewBroker()
	sock := startEventServer(t, newFakeHandler(t), b)
	ui, uiR := subscribed(t, sock)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"focus","jid":"status@broadcast"}`)
	if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
		t.Errorf("code = %v, want %s", got, CodeBadFrame)
	}
	assertNoFrame(t, ui, uiR)
}

func TestValidJIDTakesGroupHyphensOnlyBetweenDigits(t *testing.T) {
	for _, tc := range []struct {
		jid  string
		want bool
	}{
		{"120363000000000000@g.us", true},
		{"15550001234-1600000000@g.us", true},
		{"-@g.us", false},
		{"1-@g.us", false},
		{"-1@g.us", false},
		{"1--2@g.us", false},
		{"1-2-3@g.us", false},
		{"1-2@s.whatsapp.net", false},
		{"15551230001@s.whatsapp.net", true},
		{"123456789@lid", true},
	} {
		if got := ValidJID(tc.jid); got != tc.want {
			t.Errorf("ValidJID(%q) = %v, want %v", tc.jid, got, tc.want)
		}
	}
}

func TestServeConnAnswersANotConfirmedSendAsNotConfirmed(t *testing.T) {
	h := newFakeHandler(t)
	h.sendErr = ErrNotConfirmed
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, sendFrame(t, "15551230001@s.whatsapp.net", "On my way", "c-1"))
	frame := recvRaw(t, br)
	if want := readFixture(t, "error_not_confirmed.json"); !sameJSON(t, []byte(frame), want) {
		t.Errorf("reply %s, want %s (a not_confirmed error, never send_failed)", frame, want)
	}
}

// serveOne serves h on one end of a pipe and returns the other end, a reader of it, and the
// channel that receives ServeConn's result.
func serveOne(t *testing.T, h Handler) (net.Conn, *bufio.Reader, <-chan error) {
	t.Helper()
	server, client := net.Pipe()
	t.Cleanup(func() { _ = client.Close() })
	done := make(chan error, 1)
	go func() { done <- ServeConn(server, h) }()
	if err := client.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatal(err)
	}
	br := bufio.NewReader(client)
	handshake(t, client, br)
	return client, br, done
}

func TestServeConnAnswersAWipeWithWipedThenStops(t *testing.T) {
	for _, unlink := range []bool{true, false} {
		h := newFakeHandler(t)
		conn, br, done := serveOne(t, h)
		sendRaw(t, conn, fmt.Sprintf(`{"v":1,"op":"wipe","unlink":%t}`, unlink))
		if got, want := string(recvRaw(t, br)), fmt.Sprintf(`{"v":1,"op":"wiped","unlinked":%t}`, unlink); got != want {
			t.Errorf("reply %s, want %s", got, want)
		}
		select {
		case err := <-done:
			if !errors.Is(err, ErrWiped) {
				t.Errorf("ServeConn after a wipe = %v, want ErrWiped", err)
			}
		case <-time.After(5 * time.Second):
			t.Error("ServeConn kept serving after a wipe")
		}
		if got := h.wipeCalls(); !slices.Equal(got, []bool{unlink}) {
			t.Errorf("Wipe calls = %v, want [%t]", got, unlink)
		}
	}
}

func TestServeConnAnswersAWipeWhileUnlinkedWithNotLinkedAndKeepsServing(t *testing.T) {
	h := newFakeHandler(t)
	h.wipeErr = ErrNotLinked
	conn, br, _ := serveOne(t, h)
	sendRaw(t, conn, `{"v":1,"op":"wipe","unlink":true}`)
	if frame := recvRaw(t, br); fieldOf(t, frame, "code") != CodeNotLinked {
		t.Errorf("reply %s, want a %s error", frame, CodeNotLinked)
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Errorf("after a refused wipe the connection answered %v, want status", got)
	}
}

func TestServeConnAnswersAFailedWipeWithAnInternalErrorAndKeepsServing(t *testing.T) {
	h := newFakeHandler(t)
	h.wipeErr = errors.New("WhatsApp did not confirm the logout; nothing was deleted")
	conn, br, _ := serveOne(t, h)
	sendRaw(t, conn, `{"v":1,"op":"wipe","unlink":true}`)
	frame := recvRaw(t, br)
	if fieldOf(t, frame, "code") != CodeInternal || fieldOf(t, frame, "message") != h.wipeErr.Error() {
		t.Errorf("reply %s, want an %s error carrying the handler's message", frame, CodeInternal)
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Errorf("after a failed wipe the connection answered %v, want status", got)
	}
}

func TestClientWipeReturnsWipedOrTheDaemonError(t *testing.T) {
	h := newFakeHandler(t)
	c, err := Dial(startServer(t, h), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	got, err := c.Wipe(false)
	if err != nil || got != (Wiped{V: 1, Op: OpWiped, Unlinked: false}) {
		t.Errorf("Wipe(false) = %+v, %v; want wiped with unlinked false", got, err)
	}

	h.wipeErr = ErrNotLinked
	c2, err := Dial(startServer(t, h), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c2.Close() })
	_, err = c2.Wipe(true)
	var perr *ProtocolError
	if !errors.As(err, &perr) || perr.Code != CodeNotLinked {
		t.Errorf("Wipe(true) while unlinked = %v, want a %s error", err, CodeNotLinked)
	}
}

func TestServeConnAnswersDownloadMediaWithMediaReady(t *testing.T) {
	h := newFakeHandler(t)
	h.mediaPath, h.mediaMime = fixtureMediaPath, "image/jpeg"
	conn, br, _ := serveOne(t, h)
	sendRaw(t, conn, string(readFixture(t, "download_media_request.json")))
	assertSameJSON(t, recvRaw(t, br), readFixture(t, "media_ready_response.json"))
	if got := h.downloadCalls(); !slices.Equal(got, []string{"15551230001@s.whatsapp.net 3EB0AAA0"}) {
		t.Errorf("DownloadMedia calls = %q, want one for the fixture message", got)
	}
}

func TestServeConnRefusesABadDownloadMediaAndAsksNothing(t *testing.T) {
	h := newFakeHandler(t)
	conn, br, _ := serveOne(t, h)
	for _, frame := range []string{
		`{"v":1,"op":"download_media","jid":"status@broadcast","id":"3EB0AAA0"}`,
		`{"v":1,"op":"download_media","jid":"15551230001:3@s.whatsapp.net","id":"3EB0AAA0"}`,
		`{"v":1,"op":"download_media","jid":"15551230001@s.whatsapp.net","id":""}`,
		`{"v":1,"op":"download_media","jid":"15551230001@s.whatsapp.net"}`,
		`{"v":1,"op":"download_media","jid":"15551230001@s.whatsapp.net","id":7}`,
	} {
		sendRaw(t, conn, frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Errorf("%s: code = %v, want %s", frame, got, CodeBadFrame)
		}
	}
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Errorf("after refused downloads the connection answered %v, want status", got)
	}
	if got := h.downloadCalls(); len(got) != 0 {
		t.Errorf("DownloadMedia was asked %q for refused frames, want nothing", got)
	}
}

func TestServeConnAnswersDownloadMediaErrors(t *testing.T) {
	for name, tc := range map[string]struct {
		err  error
		code string
	}{
		"unlinked": {ErrNotLinked, CodeNotLinked},
		"other":    {errors.New("no media message with that id in that chat"), CodeInternal},
	} {
		t.Run(name, func(t *testing.T) {
			h := newFakeHandler(t)
			h.mediaErr = tc.err
			conn, br, _ := serveOne(t, h)
			sendRaw(t, conn, string(readFixture(t, "download_media_request.json")))
			frame := recvRaw(t, br)
			if fieldOf(t, frame, "code") != tc.code || fieldOf(t, frame, "message") != tc.err.Error() {
				t.Errorf("reply %s, want a %s error carrying %q", frame, tc.code, tc.err)
			}
			sendRaw(t, conn, `{"v":1,"op":"status"}`)
			if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
				t.Errorf("after a failed download the connection answered %v, want status", got)
			}
		})
	}
}

func TestClientDownloadMediaReturnsMediaReadyOrTheDaemonError(t *testing.T) {
	h := newFakeHandler(t)
	h.mediaPath, h.mediaMime = fixtureMediaPath, "image/jpeg"
	c, err := Dial(startServer(t, h), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	got, err := c.DownloadMedia("15551230001@s.whatsapp.net", "3EB0AAA0")
	want := MediaReady{V: 1, Op: OpMediaReady, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA0", Path: fixtureMediaPath, Mime: "image/jpeg"}
	if err != nil || got != want {
		t.Errorf("DownloadMedia = %+v, %v; want %+v", got, err, want)
	}

	h2 := newFakeHandler(t)
	h2.mediaErr = ErrNotLinked
	c2, err := Dial(startServer(t, h2), "test/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	t.Cleanup(func() { _ = c2.Close() })
	_, err = c2.DownloadMedia("15551230001@s.whatsapp.net", "3EB0AAA0")
	var perr *ProtocolError
	if !errors.As(err, &perr) || perr.Code != CodeNotLinked {
		t.Errorf("DownloadMedia while unlinked = %v, want a %s error", err, CodeNotLinked)
	}
}

func TestServeConnServesARetryByClientID(t *testing.T) {
	h := newFakeHandler(t)
	h.sent = Sent{State: SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, string(readFixture(t, "retry_request.json")))
	assertSameJSON(t, recvRaw(t, br), []byte(`{"v":1,"op":"sent","client_id":"c-7f3a9e23","state":"sent","id":"3EB0BBB1","timestamp":"2026-10-18T17:56:10Z"}`))
	if got, want := h.retryCalls(), []string{"c-7f3a9e23"}; !slices.Equal(got, want) {
		t.Errorf("handler retries = %q, want %q", got, want)
	}
	if got := h.sendCalls(); len(got) != 0 {
		t.Errorf("handler sends = %q, want none: a retry is not a new send", got)
	}
}

func TestServeConnAnswersARetryLikeASend(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
		want string
	}{
		{"not linked", ErrNotLinked, string(readFixture(t, "error_not_linked.json"))},
		{"not accepted", errors.New("the server did not acknowledge the message"), string(readFixture(t, "send_failed_response.json"))},
		{"not confirmed", ErrNotConfirmed, string(readFixture(t, "error_not_confirmed.json"))},
	} {
		h := newFakeHandler(t)
		h.sendErr = tc.err
		sock := startServer(t, h)
		conn, br := rawConn(t, sock)
		handshake(t, conn, br)
		sendRaw(t, conn, string(readFixture(t, "retry_request.json")))
		if got := recvRaw(t, br); !sameJSON(t, got, []byte(tc.want)) {
			t.Errorf("%s: reply %s, want %s", tc.name, got, tc.want)
		}
	}
}

func TestServeConnRefusesARetryWithoutAClientID(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	for _, frame := range []string{`{"v":1,"op":"retry","client_id":""}`, `{"v":1,"op":"retry"}`, `{"v":1,"op":"retry","client_id":7}`} {
		sendRaw(t, conn, frame)
		if got := fieldOf(t, recvRaw(t, br), "code"); got != CodeBadFrame {
			t.Errorf("%s: code = %v, want %s", frame, got, CodeBadFrame)
		}
	}
	if got := h.retryCalls(); len(got) != 0 {
		t.Errorf("handler retries = %q, want none", got)
	}
}
