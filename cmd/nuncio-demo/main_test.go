package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// demoBin is the nuncio-demo binary TestMain builds; the tests drive the real process over its
// socket, the seam the TUI uses.
var demoBin string

func TestMain(m *testing.M) {
	os.Exit(runMain(m))
}

func runMain(m *testing.M) int {
	dir, err := os.MkdirTemp("", "nuncio-demo-bin")
	if err != nil {
		fmt.Fprintln(os.Stderr, "mkdtemp:", err)
		return 1
	}
	defer func() { _ = os.RemoveAll(dir) }()
	demoBin = filepath.Join(dir, "nuncio-demo")
	if out, err := exec.Command("go", "build", "-o", demoBin, ".").CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "go build: %v\n%s", err, out)
		return 1
	}
	return m.Run()
}

// shortDir is a fresh runtime dir under /tmp: a unix socket path must stay under ~104 bytes.
func shortDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("/tmp", "nd")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}

func sockPath(runtimeDir string) string { return filepath.Join(runtimeDir, "nuncio", "core.sock") }

// startDemo runs the demo with the given environment and arguments; it is killed at cleanup.
func startDemo(t *testing.T, env []string, args ...string) (*exec.Cmd, *bytes.Buffer) {
	t.Helper()
	cmd := exec.Command(demoBin, args...)
	cmd.Env = env
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Start(); err != nil {
		t.Fatalf("start nuncio-demo: %v", err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	return cmd, &out
}

// runDemo starts the demo in a fresh runtime dir with no incoming replies and waits for it.
func runDemo(t *testing.T, args ...string) (string, *exec.Cmd) {
	t.Helper()
	dir := shortDir(t)
	cmd, out := startDemo(t, []string{"XDG_RUNTIME_DIR=" + dir}, append([]string{"-reply-after=0"}, args...)...)
	sock := sockPath(dir)
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if c, err := net.Dial("unix", sock); err == nil {
			_ = c.Close()
			return sock, cmd
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("the demo never served %s; output: %s", sock, out.String())
	return "", nil
}

// waitExit waits up to 10 s for the demo to exit; one still running is a test failure.
func waitExit(t *testing.T, cmd *exec.Cmd) error {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		return err
	case <-time.After(10 * time.Second):
		_ = cmd.Process.Kill()
		<-done
		t.Fatal("the demo is still running after 10 s")
		return nil
	}
}

func dial(t *testing.T, sock string) *ipc.Client {
	t.Helper()
	c, err := ipc.Dial(sock, "test/0.1.0")
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return c
}

// raw is an NDJSON connection past hello, for the ops the Go client has no method for.
type raw struct {
	conn net.Conn
	rd   *bufio.Reader
}

func dialRaw(t *testing.T, sock string) *raw {
	t.Helper()
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	r := &raw{conn: conn, rd: bufio.NewReader(conn)}
	r.send(t, map[string]any{"v": 1, "op": "hello", "client": "test/0.1.0"})
	if got := r.next(t); got["op"] != "hello_ok" {
		t.Fatalf("hello answered %v", got)
	}
	return r
}

func (r *raw) send(t *testing.T, frame map[string]any) {
	t.Helper()
	data, _ := json.Marshal(frame)
	if _, err := r.conn.Write(append(data, '\n')); err != nil {
		t.Fatalf("write: %v", err)
	}
}

func (r *raw) next(t *testing.T) map[string]any {
	t.Helper()
	_ = r.conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	line, err := r.rd.ReadBytes('\n')
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	var m map[string]any
	if err := json.Unmarshal(line, &m); err != nil {
		t.Fatalf("decode %q: %v", line, err)
	}
	return m
}

// until reads frames until one satisfies ok, failing after 5 s.
func (r *raw) until(t *testing.T, what string, ok func(map[string]any) bool) map[string]any {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if m := r.next(t); ok(m) {
			return m
		}
	}
	t.Fatalf("no %s arrived", what)
	return nil
}

func subscriber(t *testing.T, sock string) *raw {
	t.Helper()
	r := dialRaw(t, sock)
	r.send(t, map[string]any{"v": 1, "op": "subscribe"})
	if got := r.next(t); got["op"] != "subscribed" {
		t.Fatalf("subscribe answered %v", got)
	}
	return r
}

func messageEvent(id string) func(map[string]any) bool {
	return func(m map[string]any) bool {
		msg, _ := m["message"].(map[string]any)
		return m["op"] == "event_message" && msg["id"] == id
	}
}

const (
	ana   = "15551230001@s.whatsapp.net"
	group = "120363000000000001@g.us"
)

func TestRefusesWithoutXDGRuntimeDir(t *testing.T) {
	cmd, out := startDemo(t, []string{"HOME=" + shortDir(t)}, "-reply-after=0")
	err := waitExit(t, cmd)
	if err == nil || cmd.ProcessState.ExitCode() == 0 {
		t.Fatalf("the demo exited %v without XDG_RUNTIME_DIR; want a failure", err)
	}
	if !strings.Contains(out.String(), "XDG_RUNTIME_DIR") {
		t.Errorf("output %q does not name XDG_RUNTIME_DIR", out.String())
	}
}

func TestRefusesWhenTheSocketAnswersAndLeavesItInPlace(t *testing.T) {
	dir := shortDir(t)
	sock := sockPath(dir)
	if err := os.MkdirAll(filepath.Dir(sock), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = ln.Close() }()
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			_ = c.Close()
		}
	}()

	cmd, out := startDemo(t, []string{"XDG_RUNTIME_DIR=" + dir}, "-reply-after=0")
	if err := waitExit(t, cmd); err == nil {
		t.Fatalf("the demo started over a live socket; output: %s", out.String())
	}
	if !strings.Contains(out.String(), "already") {
		t.Errorf("output %q does not say a daemon is already serving", out.String())
	}
	c, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("the live socket is gone or dead after the refusal: %v", err)
	}
	_ = c.Close()
}

func TestPrintsHowToRunTheTUIAndRemovesItsSocketOnSIGTERM(t *testing.T) {
	dir := shortDir(t)
	cmd, out := startDemo(t, []string{"XDG_RUNTIME_DIR=" + dir}, "-reply-after=0")
	sock := sockPath(dir)
	deadline := time.Now().Add(10 * time.Second)
	for !strings.Contains(out.String(), "cargo run -p nuncio-tui") && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if want := "XDG_RUNTIME_DIR=" + dir + " cargo run -p nuncio-tui"; !strings.Contains(out.String(), want) || !strings.Contains(out.String(), sock) {
		t.Fatalf("start line %q does not name %s and %q", out.String(), sock, want)
	}
	if st, err := os.Stat(sock); err != nil || st.Mode().Perm() != 0o600 {
		t.Fatalf("socket stat = %v, %v; want mode 0600", st, err)
	}
	if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
		t.Fatal(err)
	}
	if err := waitExit(t, cmd); err != nil {
		t.Fatalf("exit after SIGTERM: %v; output: %s", err, out.String())
	}
	if _, err := os.Lstat(sock); !os.IsNotExist(err) {
		t.Errorf("socket still there after SIGTERM: %v", err)
	}
}

func TestHelloAndStatusAreLinkedWithUnread(t *testing.T) {
	sock, _ := runDemo(t)
	c := dial(t, sock)
	st, err := c.Status()
	if err != nil || st.Session != ipc.SessionLinked || !st.Linked || st.Unread == 0 {
		t.Errorf("status = %+v, %v; want linked with some unread", st, err)
	}
}

func TestChatsListTheInventedChatsWithPinMuteArchive(t *testing.T) {
	sock, _ := runDemo(t)
	c := dial(t, sock)
	resp, err := c.Chats(200)
	if err != nil {
		t.Fatal(err)
	}
	if len(resp.Chats) < 6 {
		t.Fatalf("got %d chats, want at least 6", len(resp.Chats))
	}
	var pinned, muted, archived, unread, isGroup bool
	for _, ch := range resp.Chats {
		if !ipc.ValidJID(ch.JID) {
			t.Errorf("chat jid %q is not a chat JID", ch.JID)
		}
		pinned = pinned || ch.Pinned
		muted = muted || ch.Muted
		archived = archived || ch.Archived
		unread = unread || ch.Unread > 0
		isGroup = isGroup || ch.JID == group
		if ch.LastMessage == nil {
			t.Errorf("chat %q has no last message", ch.JID)
		}
	}
	if !pinned || !muted || !archived || !unread || !isGroup {
		t.Errorf("pinned %v muted %v archived %v unread %v group %v; want all", pinned, muted, archived, unread, isGroup)
	}
	long := 0
	for _, ch := range resp.Chats {
		msgs, err := c.Messages(ch.JID, 200)
		if err != nil {
			t.Fatal(err)
		}
		if len(msgs.Messages) == 200 {
			long++
		}
	}
	if long == 0 {
		t.Error("no chat holds 200 messages")
	}
}

func TestSendAnswersSentAndPublishesTheMessage(t *testing.T) {
	sock, _ := runDemo(t)
	ui := subscriber(t, sock)
	c := dial(t, sock)
	sent, err := c.Send(ana, "On my way", "c-1")
	if err != nil || sent.State != ipc.SendSent || sent.ID == "" || sent.Timestamp == "" {
		t.Fatalf("send = %+v, %v; want sent with an id", sent, err)
	}
	ev := ui.until(t, "event_message for the send", messageEvent(sent.ID))
	msg := ev["message"].(map[string]any)
	if ev["jid"] != ana || msg["text"] != "On my way" || msg["from_me"] != true || msg["state"] != nil {
		t.Errorf("event = %v; want my sent message in %s", ev, ana)
	}
	msgs, err := c.Messages(ana, 1)
	if err != nil || len(msgs.Messages) != 1 || msgs.Messages[0].ID != sent.ID {
		t.Errorf("newest message = %+v, %v; want the send", msgs, err)
	}
	again, err := c.Send(ana, "something else", "c-1")
	if err != nil || again.ID != sent.ID {
		t.Errorf("repeated client_id = %+v, %v; want the stored send %s", again, err, sent.ID)
	}
}

func TestSendWithReplyToQuotesTheMessage(t *testing.T) {
	sock, _ := runDemo(t)
	c := dial(t, sock)
	msgs, err := c.Messages(ana, 1)
	if err != nil || len(msgs.Messages) == 0 {
		t.Fatal(err)
	}
	target := msgs.Messages[0].ID
	r := dialRaw(t, sock)
	r.send(t, map[string]any{"v": 1, "op": "send", "jid": ana, "text": "yes", "client_id": "c-r", "reply_to": target})
	if got := r.next(t); got["op"] != "sent" {
		t.Fatalf("reply answered %v", got)
	}
	msgs, _ = c.Messages(ana, 1)
	if msgs.Messages[0].ReplyTo != target {
		t.Errorf("reply stored as %+v; want reply_to %s", msgs.Messages[0], target)
	}
	r.send(t, map[string]any{"v": 1, "op": "send", "jid": ana, "text": "yes", "client_id": "c-r2", "reply_to": "NOPE"})
	if got := r.next(t); got["op"] != "send_failed" {
		t.Errorf("reply to an unknown id answered %v; want send_failed", got)
	}
}

func TestFailTextAnswersSendFailedAndRetrySends(t *testing.T) {
	sock, _ := runDemo(t)
	ui := subscriber(t, sock)
	c := dial(t, sock)
	if _, err := c.Send(ana, "fail this one", "c-f"); err == nil {
		t.Fatal("a text starting with fail was sent; want send_failed")
	}
	msgs, err := c.Messages(ana, 1)
	if err != nil || len(msgs.Messages) != 1 || msgs.Messages[0].State != ipc.MessageNotSent || msgs.Messages[0].ClientID != "c-f" {
		t.Fatalf("newest message = %+v, %v; want my not-sent send", msgs, err)
	}
	id := msgs.Messages[0].ID
	ev := ui.until(t, "the not-sent event", messageEvent(id))
	if ev["message"].(map[string]any)["state"] != "not_sent" {
		t.Errorf("first event = %v; want state not_sent", ev)
	}
	sent, err := c.Retry("c-f")
	if err != nil || sent.State != ipc.SendSent || sent.ID != id {
		t.Fatalf("retry = %+v, %v; want sent under id %s", sent, err, id)
	}
	ev = ui.until(t, "the sent event", messageEvent(id))
	if ev["message"].(map[string]any)["state"] != nil {
		t.Errorf("retry event = %v; want no state", ev)
	}
	if _, err := c.Retry("never-stored"); err == nil {
		t.Error("retry of an unknown client_id succeeded")
	}
}

func TestReactAnswersReactedAndUpdatesTheMessage(t *testing.T) {
	sock, _ := runDemo(t)
	ui := subscriber(t, sock)
	c := dial(t, sock)
	msgs, _ := c.Messages(ana, 1)
	target := msgs.Messages[0].ID
	r := dialRaw(t, sock)
	r.send(t, map[string]any{"v": 1, "op": "react", "jid": ana, "id": target, "emoji": "👍"})
	if got := r.next(t); got["op"] != "reacted" || got["emoji"] != "👍" || got["id"] != target {
		t.Fatalf("react answered %v", got)
	}
	ev := ui.until(t, "the reacted message", messageEvent(target))
	reactions, _ := ev["message"].(map[string]any)["reactions"].([]any)
	if len(reactions) == 0 || reactions[0].(map[string]any)["emoji"] != "👍" || reactions[0].(map[string]any)["from_me"] != true {
		t.Errorf("event reactions = %v; want mine first", reactions)
	}
	msgs, _ = c.Messages(ana, 1)
	if len(msgs.Messages[0].Reactions) == 0 || msgs.Messages[0].Reactions[0].Emoji != "👍" {
		t.Errorf("stored reactions = %+v", msgs.Messages[0].Reactions)
	}
	r.send(t, map[string]any{"v": 1, "op": "react", "jid": ana, "id": "NOPE", "emoji": "👍"})
	if got := r.next(t); got["op"] != "error" {
		t.Errorf("react on an unknown id answered %v; want error", got)
	}
}

func TestSearchFindsAKnownMessageNewestFirst(t *testing.T) {
	sock, _ := runDemo(t)
	r := dialRaw(t, sock)
	r.send(t, map[string]any{"v": 1, "op": "search", "jid": ana, "query": "VENUE", "limit": 50})
	got := r.next(t)
	msgs, _ := got["messages"].([]any)
	if got["op"] != "messages" || len(msgs) < 2 {
		t.Fatalf("search answered %v; want at least two matches", got)
	}
	first, second := msgs[0].(map[string]any), msgs[1].(map[string]any)
	if first["timestamp"].(string) < second["timestamp"].(string) {
		t.Errorf("matches are not newest first: %v then %v", first["timestamp"], second["timestamp"])
	}
	for _, m := range msgs {
		if !strings.Contains(strings.ToLower(m.(map[string]any)["text"].(string)), "venue") {
			t.Errorf("match %v does not contain the query", m)
		}
	}
}

func TestDownloadMediaWritesAPrivateFileWithItsMime(t *testing.T) {
	sock, _ := runDemo(t)
	c := dial(t, sock)
	chats, _ := c.Chats(200)
	kinds := map[string]bool{}
	for _, ch := range chats.Chats {
		msgs, _ := c.Messages(ch.JID, 200)
		for _, m := range msgs.Messages {
			if m.Media == nil || kinds[m.Kind] {
				continue
			}
			kinds[m.Kind] = true
			ready, err := c.DownloadMedia(ch.JID, m.ID)
			if err != nil || ready.Mime != m.Media.Mime {
				t.Fatalf("download %s = %+v, %v; want mime %s", m.Kind, ready, err, m.Media.Mime)
			}
			st, err := os.Stat(ready.Path)
			if err != nil || st.Mode().Perm() != 0o600 || st.Size() == 0 {
				t.Fatalf("downloaded file %s: %v, %v; want a non-empty 0600 file", ready.Path, st, err)
			}
			if m.Kind == ipc.KindImage {
				head := make([]byte, 8)
				f, _ := os.Open(ready.Path)
				_, _ = io.ReadFull(f, head)
				_ = f.Close()
				if !bytes.Equal(head, []byte("\x89PNG\r\n\x1a\n")) {
					t.Errorf("image file starts %q; want a PNG", head)
				}
			}
		}
	}
	if !kinds[ipc.KindImage] || !kinds[ipc.KindDocument] {
		t.Errorf("media kinds found %v; want an image and a document", kinds)
	}
	if _, err := c.DownloadMedia(ana, "NOPE"); err == nil {
		t.Error("download of an unknown id succeeded")
	}
}

func TestWipeIsRefusedAndTheDemoKeepsServing(t *testing.T) {
	sock, _ := runDemo(t)
	c := dial(t, sock)
	if _, err := c.Wipe(false); err == nil {
		t.Fatal("wipe succeeded; the demo must refuse it")
	}
	if _, err := dial(t, sock).Status(); err != nil {
		t.Errorf("status after wipe: %v", err)
	}
}

func TestAnIncomingReplyFollowsASend(t *testing.T) {
	sock, _ := runDemo(t, "-reply-after=50ms")
	ui := subscriber(t, sock)
	c := dial(t, sock)
	sent, err := c.Send(group, "hello all", "c-g")
	if err != nil {
		t.Fatal(err)
	}
	ui.until(t, "an incoming message", func(m map[string]any) bool {
		msg, _ := m["message"].(map[string]any)
		return m["op"] == "event_message" && m["jid"] == group && msg["from_me"] == false && msg["id"] != sent.ID
	})
	ev := ui.until(t, "event_unread", func(m map[string]any) bool { return m["op"] == "event_unread" })
	if ev["jid"] != group || ev["chat_unread"].(float64) < 1 || ev["unread"].(float64) < 1 {
		t.Errorf("event_unread = %v", ev)
	}
}
