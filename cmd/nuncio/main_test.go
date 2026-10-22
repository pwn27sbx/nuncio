package main

import (
	"bufio"
	"bytes"
	"context"
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
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// fakeDaemon answers the wire protocol from fixed values: unlinked with no chats unless a test
// sets more. Send answers sent, or sendErr, and records every call.
type fakeDaemon struct {
	session string
	chats   []ipc.Chat
	sent    ipc.Sent
	sendErr error

	// wipeErr answers every Wipe; wipes records each call's unlink.
	wipeErr error

	mu      sync.Mutex
	sends   []ipc.SendRequest
	retries []string
	wipes   []bool
}

// Retry records the client_id and answers like Send.
func (d *fakeDaemon) Retry(clientID string) (ipc.Sent, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.retries = append(d.retries, clientID)
	return d.sent, d.sendErr
}

func (d *fakeDaemon) retryCalls() []string {
	d.mu.Lock()
	defer d.mu.Unlock()
	return append([]string(nil), d.retries...)
}

// Wipe records the request only: the fake daemon deletes nothing, so a test can tell what the
// CLI deleted by itself.
func (d *fakeDaemon) Wipe(unlink bool) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.wipes = append(d.wipes, unlink)
	return d.wipeErr
}

func (d *fakeDaemon) wipeCalls() []bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	return append([]bool(nil), d.wipes...)
}

func (d *fakeDaemon) Hello(string) ipc.Status { return d.Status() }
func (d *fakeDaemon) Status() ipc.Status {
	return ipc.Status{Daemon: "nunciod/0.1.0", Linked: d.session != ipc.SessionUnlinked, Session: d.session}
}
func (d *fakeDaemon) Chats() []ipc.Chat { return d.chats }
func (d *fakeDaemon) Messages(string, int) ([]ipc.Message, error) {
	return nil, nil
}
func (d *fakeDaemon) Send(jid, text, clientID string) (ipc.Sent, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.sends = append(d.sends, ipc.SendRequest{JID: jid, Text: text, ClientID: clientID})
	return d.sent, d.sendErr
}

// DownloadMedia is not used by the CLI; the fake daemon has no media.
func (d *fakeDaemon) DownloadMedia(string, string) (string, string, error) {
	return "", "", errors.New("the fake daemon has no media")
}

func (d *fakeDaemon) sendCalls() []ipc.SendRequest {
	d.mu.Lock()
	defer d.mu.Unlock()
	return append([]ipc.SendRequest(nil), d.sends...)
}

// serveFakeDaemon serves d, with events from a new broker, at $XDG_RUNTIME_DIR/nuncio/core.sock,
// where the CLI looks, and returns the socket path.
func serveFakeDaemon(t *testing.T, d *fakeDaemon) string {
	t.Helper()
	if d.session == "" {
		d.session = ipc.SessionUnlinked
	}
	runtime := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", runtime)
	sock := filepath.Join(runtime, "nuncio", "core.sock")
	if err := os.MkdirAll(filepath.Dir(sock), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen %s: %v", sock, err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	events := ipc.NewBroker()
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() { _ = ipc.ServeConnEvents(conn, d, events) }()
		}
	}()
	return sock
}

func runCLI(args ...string) (code int, stdout, stderr string) {
	var out, errOut bytes.Buffer
	code = cli(args, &out, &errOut)
	return code, out.String(), errOut.String()
}

func TestStatusPrintsTheDaemonState(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{})
	for _, args := range [][]string{nil, {"status"}} {
		code, out, errOut := runCLI(args...)
		if code != 0 {
			t.Fatalf("nuncio %v: exit %d, stderr %q", args, code, errOut)
		}
		if want := "linked=false session=unlinked unread=0\n"; out != want {
			t.Errorf("nuncio %v: stdout %q, want %q", args, out, want)
		}
	}
}

func TestUnknownCommandIsAUsageError(t *testing.T) {
	code, _, errOut := runCLI("frobnicate")
	if code != 2 {
		t.Errorf("exit %d, want 2", code)
	}
	if !strings.Contains(errOut, `unknown command "frobnicate"`) || !strings.Contains(errOut, usage) {
		t.Errorf("stderr %q, want the unknown-command line and the usage text", errOut)
	}
}

func TestHelpPrintsUsage(t *testing.T) {
	for _, arg := range []string{"help", "-h", "--help"} {
		code, out, _ := runCLI(arg)
		if code != 0 || !strings.Contains(out, usage) {
			t.Errorf("nuncio %s: exit %d, stdout %q, want 0 and the usage text", arg, code, out)
		}
	}
}

// isolateData points every XDG data and state directory at an empty temp dir, so a test can
// prove that nothing was written there.
func isolateData(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	t.Setenv("XDG_DATA_HOME", filepath.Join(root, "data"))
	t.Setenv("XDG_STATE_HOME", filepath.Join(root, "state"))
	t.Setenv("HOME", filepath.Join(root, "home"))
	return root
}

func assertEmptyDir(t *testing.T, dir string) {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		var names []string
		for _, e := range entries {
			names = append(names, e.Name())
		}
		t.Errorf("%s holds %v, want nothing", dir, names)
	}
}

func TestLinkWithoutFlagsIsAUsageError(t *testing.T) {
	root := isolateData(t)
	code, _, errOut := runCLI("link")
	if code != 2 {
		t.Errorf("exit %d, want 2", code)
	}
	if !strings.Contains(errOut, "pass exactly one of --qr or --phone") {
		t.Errorf("stderr %q, want the flag error", errOut)
	}
	assertEmptyDir(t, root)
}

// TestLinkWithoutConsentChangesNothing is the consent gate: no data dir, no nuncio.db. No
// keyring item can exist either, because nothing before the gate builds a keyring or a device
// store (the real keyring is never touched by tests).
func TestLinkWithoutConsentChangesNothing(t *testing.T) {
	root := isolateData(t)
	code, out, errOut := runCLI("link", "--phone", "+10000000000")
	if code != 2 {
		t.Errorf("exit %d, want 2", code)
	}
	if !strings.Contains(errOut, wa.LinkWarning) {
		t.Errorf("stderr %q, want the ban warning", errOut)
	}
	if !strings.Contains(errOut, "nothing was linked") {
		t.Errorf("stderr %q, want the nothing-was-linked line", errOut)
	}
	if out != "" {
		t.Errorf("stdout %q, want nothing", out)
	}
	assertEmptyDir(t, root)
}

func TestMaskPhoneKeepsOnlyTheLastFourDigits(t *testing.T) {
	for _, tc := range []struct{ in, want string }{
		{"15551230001", "…0001"}, // a normal number
		{"123", "…"},             // short: revealing the tail would reveal it all
		{"", "…"},                // empty
	} {
		if got := maskPhone(tc.in); got != tc.want {
			t.Errorf("maskPhone(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

// inUTC makes the CLI print local times as UTC for the duration of a test.
func inUTC(t *testing.T) {
	t.Helper()
	local := time.Local
	time.Local = time.UTC
	t.Cleanup(func() { time.Local = local })
}

func fixtureChats(t *testing.T) []ipc.Chat {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "proto", "fixtures", "chats_response.json"))
	if err != nil {
		t.Fatal(err)
	}
	var resp ipc.ChatsResponse
	if err := json.Unmarshal(data, &resp); err != nil {
		t.Fatal(err)
	}
	return resp.Chats
}

func TestChatsPrintsOneLinePerChatWithoutMessageText(t *testing.T) {
	inUTC(t)
	serveFakeDaemon(t, &fakeDaemon{chats: fixtureChats(t)})
	code, out, errOut := runCLI("chats")
	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, errOut)
	}
	want := "Ada Lovelace  2 unread   pinned    2026-10-18 17:55\n" +
		"Grace Hopper  0 unread   -         2026-10-18 16:02\n" +
		"Omarchy devs  17 unread  archived  -\n"
	if out != want {
		t.Errorf("stdout =\n%s\nwant\n%s", out, want)
	}
	for _, body := range []string{"See you at 18:00", "The compiler is done"} {
		if strings.Contains(out, body) {
			t.Errorf("stdout carries the message text %q", body)
		}
	}
}

func TestChatsMasksTheNumberOfAChatWithoutAName(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{chats: []ipc.Chat{
		{JID: "15551230009@s.whatsapp.net", Unread: 1},
		{JID: "15550001234-1600000000@g.us", Pinned: true, Archived: true},
	}})
	code, out, errOut := runCLI("chats")
	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, errOut)
	}
	want := "…0009  1 unread  -                -\n" +
		"…0000  0 unread  pinned,archived  -\n"
	if out != want {
		t.Errorf("stdout =\n%s\nwant\n%s", out, want)
	}
	if strings.Contains(out, "1555") {
		t.Errorf("stdout %q carries a full number", out)
	}
}

// A chat without a name prints the masked phone the daemon serves (a LID chat's number comes from
// WhatsApp's LID map), and a LID chat whose number is unknown never prints its LID digits.
func TestChatsPrintsTheServedPhoneAndNeverTheLIDDigits(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{chats: []ipc.Chat{
		{JID: "100000000000001@lid", Unread: 1, Phone: "…0001"},
		{JID: "100000000000009@lid"},
		{JID: "100000000000002@lid", Name: "Grace Hopper", Phone: "…0002"},
	}})
	code, out, errOut := runCLI("chats")
	if code != 0 {
		t.Fatalf("exit %d, stderr %q", code, errOut)
	}
	want := "…0001          1 unread  -  -\n" +
		"hidden number  0 unread  -  -\n" +
		"Grace Hopper   0 unread  -  -\n"
	if out != want {
		t.Errorf("stdout =\n%s\nwant\n%s", out, want)
	}
	if strings.Contains(out, "0009") || strings.Contains(out, "1000") {
		t.Errorf("stdout %q carries LID digits", out)
	}
}

func TestChatsWithNoChatsSaysSo(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{})
	if code, out, _ := runCLI("chats"); code != 0 || out != "no chats\n" {
		t.Errorf("exit %d, stdout %q; want 0 and %q", code, out, "no chats\n")
	}
}

const adaJID = "15551230001@s.whatsapp.net"

func TestSendRefusesAnEmptyMessage(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked}
	serveFakeDaemon(t, d)
	for _, text := range []string{"", "   ", "\t\n"} {
		code, out, errOut := runCLI("send", adaJID, text)
		if code != 2 || out != "" || !strings.Contains(errOut, "the message is empty") {
			t.Errorf("send %q: exit %d, stdout %q, stderr %q; want 2 and the empty-message error", text, code, out, errOut)
		}
	}
	if got := d.sendCalls(); len(got) != 0 {
		t.Errorf("daemon got %+v, want no send", got)
	}
}

func TestSendRefusesMoreThanOneRecipient(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked}
	serveFakeDaemon(t, d)
	for _, args := range [][]string{
		{"send", adaJID, "15551230002@s.whatsapp.net", "hello"},
		{"send", adaJID + "," + "15551230002@s.whatsapp.net", "hello"},
		{"send", adaJID, "two", "words"}, // an unquoted message could hide a second recipient
		{"send", adaJID},
		{"send"},
	} {
		code, out, errOut := runCLI(args...)
		if code != 2 || out != "" {
			t.Errorf("nuncio %q: exit %d, stdout %q, stderr %q; want 2", args, code, out, errOut)
		}
	}
	if got := d.sendCalls(); len(got) != 0 {
		t.Errorf("daemon got %+v, want no send", got)
	}
}

func TestSendPrintsSentOnlyWhenTheDaemonSentIt(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked, sent: ipc.Sent{State: ipc.SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}}
	serveFakeDaemon(t, d)
	code, out, errOut := runCLI("send", adaJID, "On my way")
	if code != 0 || out != "sent\n" || errOut != "" {
		t.Errorf("exit %d, stdout %q, stderr %q; want 0 and %q", code, out, errOut, "sent\n")
	}
	got := d.sendCalls()
	if len(got) != 1 || got[0].JID != adaJID || got[0].Text != "On my way" || got[0].ClientID == "" {
		t.Errorf("daemon got %+v, want one send of the message with a client_id", got)
	}
}

func TestSendPrintsQueuedWhenTheDaemonQueuedIt(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionOffline, sent: ipc.Sent{State: ipc.SendQueued}})
	code, out, _ := runCLI("send", adaJID, "On my way")
	if code != 0 || !strings.HasPrefix(out, "queued") {
		t.Errorf("exit %d, stdout %q; want 0 and a queued line", code, out)
	}
}

func TestSendPrintsNotSentWithTheReasonAndFails(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionOffline, sendErr: errors.New("the WhatsApp session is offline")})
	code, out, errOut := runCLI("send", adaJID, "On my way")
	if code != 1 || out != "" || !strings.HasPrefix(errOut, "nuncio: not sent: the WhatsApp session is offline\n") {
		t.Errorf("exit %d, stdout %q, stderr %q; want 1 and the not-sent line", code, out, errOut)
	}
}

// A send that ends queued, not sent or not confirmed prints its client_id, which is what
// `nuncio retry` takes.
func TestSendPrintsTheClientIDUnlessItWasSent(t *testing.T) {
	for _, tc := range []struct {
		name string
		d    *fakeDaemon
		code int
		out  func(stdout, stderr string) string
	}{
		{"queued", &fakeDaemon{session: ipc.SessionOffline, sent: ipc.Sent{State: ipc.SendQueued}}, 0, func(o, _ string) string { return o }},
		{"not sent", &fakeDaemon{session: ipc.SessionLinked, sendErr: errors.New("WhatsApp did not accept the message")}, 1, func(_, e string) string { return e }},
		{"not confirmed", &fakeDaemon{session: ipc.SessionLinked, sendErr: ipc.ErrNotConfirmed}, 1, func(_, e string) string { return e }},
	} {
		serveFakeDaemon(t, tc.d)
		code, out, errOut := runCLI("send", adaJID, "On my way")
		sends := tc.d.sendCalls()
		if code != tc.code || len(sends) != 1 {
			t.Fatalf("%s: exit %d after %d sends, want %d after 1", tc.name, code, len(sends), tc.code)
		}
		if got := tc.out(out, errOut); !strings.Contains(got, sends[0].ClientID) {
			t.Errorf("%s: output %q, want the client_id %s", tc.name, got, sends[0].ClientID)
		}
		if tc.name != "queued" && !strings.Contains(errOut, "nuncio retry "+sends[0].ClientID) {
			t.Errorf("%s: stderr %q, want the retry command", tc.name, errOut)
		}
	}
	d := &fakeDaemon{session: ipc.SessionLinked, sent: ipc.Sent{State: ipc.SendSent, ID: "3EB0BBB1"}}
	serveFakeDaemon(t, d)
	if _, out, errOut := runCLI("send", adaJID, "On my way"); out != "sent\n" || errOut != "" {
		t.Errorf("sent: stdout %q, stderr %q; want only %q", out, errOut, "sent\n")
	}
}

func TestRetrySendsOneRetryByClientID(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked, sent: ipc.Sent{State: ipc.SendSent, ID: "3EB0BBB1"}}
	serveFakeDaemon(t, d)
	code, out, errOut := runCLI("retry", "c-7f3a9e23")
	if code != 0 || out != "sent\n" || errOut != "" {
		t.Errorf("exit %d, stdout %q, stderr %q; want 0 and %q", code, out, errOut, "sent\n")
	}
	if got := d.retryCalls(); len(got) != 1 || got[0] != "c-7f3a9e23" {
		t.Errorf("daemon retries = %q, want exactly [c-7f3a9e23]", got)
	}
	if got := d.sendCalls(); len(got) != 0 {
		t.Errorf("daemon sends = %+v, want none: a retry is not a new send", got)
	}
}

func TestRetryThatFailsAgainSaysNotSent(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionLinked, sendErr: errors.New("WhatsApp did not accept the message")})
	code, out, errOut := runCLI("retry", "c-1")
	if code != 1 || out != "" || !strings.Contains(errOut, "not sent: WhatsApp did not accept the message") || !strings.Contains(errOut, "nuncio retry c-1") {
		t.Errorf("exit %d, stdout %q, stderr %q; want 1 and the not-sent line with the retry command", code, out, errOut)
	}
}

// retry takes exactly one client_id: no list, no stdin, nothing to script a batch with.
func TestRetryRefusesAnythingButOneClientID(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked, sent: ipc.Sent{State: ipc.SendSent}}
	serveFakeDaemon(t, d)
	for _, args := range [][]string{{"retry"}, {"retry", "c-1", "c-2"}, {"retry", ""}, {"retry", " "}} {
		code, out, errOut := runCLI(args...)
		if code != 2 || out != "" || !strings.Contains(errOut, "nuncio retry <client_id>") {
			t.Errorf("nuncio %q: exit %d, stdout %q, stderr %q; want 2 and the usage", args, code, out, errOut)
		}
	}
	if got := d.retryCalls(); len(got) != 0 {
		t.Errorf("daemon retries = %q, want none", got)
	}
}

func TestSendWhileUnlinkedFailsWithTheNotLinkedMessage(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{sendErr: ipc.ErrNotLinked})
	code, out, errOut := runCLI("send", adaJID, "On my way")
	if code != 1 || out != "" {
		t.Errorf("exit %d, stdout %q; want 1 and nothing on stdout", code, out)
	}
	if !strings.Contains(errOut, "not sent: no WhatsApp device is linked") || !strings.Contains(errOut, "nuncio link") {
		t.Errorf("stderr %q, want the not_linked message and the way to link", errOut)
	}
}

// The CLI never reads stdin: a message piped in is not sent, and the pipe is left unread.
func TestSendNeverReadsStdin(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked, sent: ipc.Sent{State: ipc.SendSent}}
	serveFakeDaemon(t, d)
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.WriteString(w, "piped text\n"); err != nil {
		t.Fatal(err)
	}
	_ = w.Close()
	stdin := os.Stdin
	os.Stdin = r
	t.Cleanup(func() { os.Stdin = stdin; _ = r.Close() })

	for _, args := range [][]string{{"send", adaJID}, {"send", adaJID, "-"}} {
		runCLI(args...)
	}
	left, err := io.ReadAll(r)
	if err != nil || string(left) != "piped text\n" {
		t.Errorf("stdin left %q, %v; want it untouched", left, err)
	}
	for _, s := range d.sendCalls() {
		if strings.Contains(s.Text, "piped") {
			t.Errorf("daemon got %+v, the piped text", s)
		}
	}
}

// subscribeUI connects to sock as a UI that subscribed to events.
func subscribeUI(t *testing.T, sock string) *bufio.Reader {
	t.Helper()
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	br := bufio.NewReader(conn)
	for _, frame := range []string{`{"v":1,"op":"hello","client":"test-ui/0.1.0"}`, `{"v":1,"op":"subscribe"}`} {
		if _, err := io.WriteString(conn, frame+"\n"); err != nil {
			t.Fatal(err)
		}
		if _, err := br.ReadString('\n'); err != nil {
			t.Fatal(err)
		}
	}
	return br
}

func TestFocusPrintsHowManyUIsGotIt(t *testing.T) {
	sock := serveFakeDaemon(t, &fakeDaemon{})
	code, out, errOut := runCLI("focus", adaJID)
	if code != 0 || out != "focus delivered to 0 UIs: no UI is running\n" {
		t.Errorf("with no UI: exit %d, stdout %q, stderr %q", code, out, errOut)
	}

	first, second := subscribeUI(t, sock), subscribeUI(t, sock)
	code, out, errOut = runCLI("focus", adaJID)
	if code != 0 || out != "focus delivered to 2 UIs\n" {
		t.Errorf("with two UIs: exit %d, stdout %q, stderr %q", code, out, errOut)
	}
	for i, br := range []*bufio.Reader{first, second} {
		line, err := br.ReadString('\n')
		if want := `{"v":1,"op":"event_focus","jid":"15551230001@s.whatsapp.net"}` + "\n"; err != nil || line != want {
			t.Errorf("UI %d got %q, %v; want %q", i+1, line, err, want)
		}
	}
}

func TestFocusRefusesAJIDThatIsNotAChat(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{})
	for _, args := range [][]string{{"focus", "status@broadcast"}, {"focus"}, {"focus", adaJID, adaJID}} {
		if code, _, _ := runCLI(args...); code != 2 {
			t.Errorf("nuncio %q: exit %d, want 2", args, code)
		}
	}
}

func TestHelpDocumentsEveryCommand(t *testing.T) {
	for _, cmd := range []string{"nuncio status", "nuncio chats", "nuncio send <jid> <message>", "nuncio focus <jid>", "nuncio link --qr", "nuncio link --phone", "not confirmed", "nuncio logout --confirm", "nuncio purge --confirm", "nuncio retry <client_id>"} {
		if !strings.Contains(usage, cmd) {
			t.Errorf("usage does not document %q:\n%s", cmd, usage)
		}
	}
}

// The help names the silent window of a dead connection: up to 3 minutes (whatsmeow's
// KeepAliveMaxFailTime) in which a send still ends not confirmed instead of queued.
func TestHelpNamesTheSilentConnectionWindow(t *testing.T) {
	const want = "up to 3 minutes"
	if !strings.Contains(usage, want) {
		t.Errorf("usage does not mention %q:\n%s", want, usage)
	}
}

// linkedDevice stores a whatsmeow device with an ID in the isolated data dir, sealed under a data
// key in localKeyring (a fake: call fakeKeys first), as a finished link would. Nothing here
// touches the network.
func linkedDevice(t *testing.T) {
	t.Helper()
	path, err := wa.DBPath()
	if err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	device, _, err := wa.OpenDevice(context.Background(), db, path, localKeyring, true)
	if err != nil {
		t.Fatal(err)
	}
	device.ID = &types.JID{User: "15550009999", Device: 1, Server: types.DefaultUserServer}
	// Placeholder bytes of the lengths the device table requires.
	device.Account = &waAdv.ADVSignedDeviceIdentity{Details: []byte{1}, AccountSignature: make([]byte, 64), AccountSignatureKey: make([]byte, 32), DeviceSignature: make([]byte, 64)}
	if err := device.Save(context.Background()); err != nil {
		t.Fatal(err)
	}
}

// fakeConnect replaces the connect of an already linked device, so no test can reach WhatsApp,
// and counts the attempts.
func fakeConnect(t *testing.T) *int {
	t.Helper()
	calls := 0
	connect := connectLinked
	connectLinked = func(*whatsmeow.Client) error {
		calls++
		return errors.New("fake connect")
	}
	t.Cleanup(func() { connectLinked = connect })
	return &calls
}

func TestLinkRefusesASecondSessionWhileTheDaemonAnswers(t *testing.T) {
	isolateData(t)
	fakeKeys(t)
	linkedDevice(t)
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionLinked})
	calls := fakeConnect(t)

	code, _, errOut := runCLI("link", "--qr", "--consent")
	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	if !strings.Contains(errOut, "nunciod is running and owns this device's session") {
		t.Errorf("stderr %q, want the daemon-owns-the-session line", errOut)
	}
	if *calls != 0 {
		t.Errorf("connected %d times, want never", *calls)
	}
}

func TestLinkConnectsTheLinkedDeviceWhenNoDaemonAnswers(t *testing.T) {
	isolateData(t)
	fakeKeys(t)
	linkedDevice(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir()) // no core.sock there
	calls := fakeConnect(t)

	code, _, _ := runCLI("link", "--qr", "--consent")
	if code != 1 || *calls != 1 {
		t.Errorf("exit %d after %d connects, want 1 (the fake connect fails) after exactly 1", code, *calls)
	}
}

// A send whose ack timed out may have been delivered: the CLI says not confirmed, never not sent.
func TestSendNotConfirmedNeverSaysNotSent(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionLinked, sendErr: ipc.ErrNotConfirmed})
	code, out, errOut := runCLI("send", adaJID, "On my way")
	if code != 1 || out != "" {
		t.Errorf("exit %d, stdout %q; want 1 and nothing on stdout", code, out)
	}
	if strings.Contains(errOut, "not sent") || !strings.Contains(errOut, "not confirmed") || !strings.Contains(errOut, "may have been delivered") {
		t.Errorf("stderr %q, want the not-confirmed line and no not-sent claim", errOut)
	}
	// The daemon answers it with the not_confirmed code; the CLI prints the same honest text as
	// before the code existed, and the retry hint that reuses the same WhatsApp id.
	lines := strings.Split(strings.TrimSuffix(errOut, "\n"), "\n")
	if len(lines) != 2 || lines[0] != "nuncio: "+ipc.ErrNotConfirmed.Error() ||
		!strings.HasPrefix(lines[1], "nuncio: if it did not arrive, try again with the same message id: nuncio retry ") {
		t.Errorf("stderr %q, want exactly the not-confirmed line and the same-id retry hint", errOut)
	}
}

// TestMain points the purge keyring at one that refuses every call, so no test can reach the
// owner's real keyring even if it forgets fakeKeys.
func TestMain(m *testing.M) {
	localKeyring = refusingKeyring{}
	os.Exit(m.Run())
}

type refusingKeyring struct{}

var errRealKeyring = errors.New("test: the real keyring is off limits; call fakeKeys")

func (refusingKeyring) Set(string, string, string) error   { return errRealKeyring }
func (refusingKeyring) Get(string, string) (string, error) { return "", errRealKeyring }
func (refusingKeyring) Delete(string, string) error        { return errRealKeyring }

// memKeyring is an in-memory keyring.
type memKeyring struct {
	mu    sync.Mutex
	items map[string]string
}

func (k *memKeyring) Set(service, user, secret string) error {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.items[service+"/"+user] = secret
	return nil
}

func (k *memKeyring) Get(service, user string) (string, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	if v, ok := k.items[service+"/"+user]; ok {
		return v, nil
	}
	return "", store.ErrKeyNotFound
}

func (k *memKeyring) Delete(service, user string) error {
	k.mu.Lock()
	defer k.mu.Unlock()
	if _, ok := k.items[service+"/"+user]; !ok {
		return store.ErrKeyNotFound
	}
	delete(k.items, service+"/"+user)
	return nil
}

func (k *memKeyring) count() int {
	k.mu.Lock()
	defer k.mu.Unlock()
	return len(k.items)
}

// fakeKeys makes localKeyring a fake holding every device item under the nuncio service.
func fakeKeys(t *testing.T) *memKeyring {
	t.Helper()
	k := &memKeyring{items: map[string]string{}}
	for _, name := range []string{"device.noise_key", "device.identity_key", "device.signed_pre_key", "device.adv_secret_key"} {
		_ = k.Set(store.KeyringService, name, "synthetic")
	}
	prev := localKeyring
	localKeyring = k
	t.Cleanup(func() { localKeyring = prev })
	return k
}

// populatedData isolates the data dirs and fills the nuncio data dir with a database, a media
// file and an export, as a used install leaves it. It returns the isolated root and the data dir.
func populatedData(t *testing.T) (root, dataDir string) {
	t.Helper()
	root = isolateData(t)
	path, err := wa.DBPath()
	if err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := store.EnsureSchema(db); err != nil {
		t.Fatal(err)
	}
	_ = db.Close()
	dataDir = filepath.Dir(path)
	for _, p := range []string{filepath.Join(dataDir, store.MediaDir, "0f3a.jpg"), filepath.Join(dataDir, store.ExportsDir, "chat.txt")} {
		if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("synthetic"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root, dataDir
}

// listing is every path under root with its mode and size, to prove nothing changed.
func listing(t *testing.T, root string) []string {
	t.Helper()
	var out []string
	err := filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		fi, err := d.Info()
		if err != nil {
			return err
		}
		out = append(out, fmt.Sprintf("%s %v %d", p, fi.Mode(), fi.Size()))
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func TestLogoutAndPurgeWithoutConfirmChangeNothing(t *testing.T) {
	for _, cmd := range []string{"logout", "purge"} {
		t.Run(cmd, func(t *testing.T) {
			root, dataDir := populatedData(t)
			keys := fakeKeys(t)
			d := &fakeDaemon{session: ipc.SessionLinked}
			sock := serveFakeDaemon(t, d)
			before := listing(t, root)
			calls := fakeConnect(t)

			code, out, errOut := runCLI(cmd)
			if code != 2 {
				t.Errorf("exit %d, want 2", code)
			}
			if out != "" {
				t.Errorf("stdout %q, want nothing", out)
			}
			for _, want := range []string{filepath.Join(dataDir, store.DBFile), filepath.Join(dataDir, store.MediaDir), "--confirm", "Nothing was deleted"} {
				if !strings.Contains(errOut, want) {
					t.Errorf("stderr %q, want it to name %q", errOut, want)
				}
			}
			if after := listing(t, root); !slices.Equal(before, after) {
				t.Errorf("the data dirs changed:\n before %v\n after  %v", before, after)
			}
			if n := keys.count(); n != 4 {
				t.Errorf("keyring holds %d items, want the 4 it had", n)
			}
			if got := d.wipeCalls(); len(got) != 0 {
				t.Errorf("the daemon was asked to wipe %v, want never", got)
			}
			if _, err := os.Stat(sock); err != nil {
				t.Errorf("socket: %v", err)
			}
			if *calls != 0 {
				t.Errorf("connected %d times, want never", *calls)
			}
		})
	}
}

func TestLogoutAsksTheRunningDaemonToUnlinkAndWipe(t *testing.T) {
	d := &fakeDaemon{session: ipc.SessionLinked}
	serveFakeDaemon(t, d)
	code, out, errOut := runCLI("logout", "--confirm")
	if code != 0 || !strings.Contains(out, "logged out") {
		t.Errorf("exit %d, stdout %q, stderr %q; want 0 and logged out", code, out, errOut)
	}
	if got := d.wipeCalls(); !slices.Equal(got, []bool{true}) {
		t.Errorf("wipe requests %v, want exactly one with unlink", got)
	}
}

func TestLogoutWithoutADaemonRefusesAndConnectsNothing(t *testing.T) {
	root, _ := populatedData(t)
	keys := fakeKeys(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir()) // no core.sock there
	linkedDevice(t)
	before := listing(t, root)
	items := keys.count()
	calls := fakeConnect(t)

	code, out, errOut := runCLI("logout", "--confirm")
	if code != 1 || out != "" {
		t.Errorf("exit %d, stdout %q; want 1 and nothing on stdout", code, out)
	}
	for _, want := range []string{"nunciod is not running", "nuncio purge --confirm", "Nothing was deleted"} {
		if !strings.Contains(errOut, want) {
			t.Errorf("stderr %q, want %q", errOut, want)
		}
	}
	if *calls != 0 {
		t.Errorf("connected %d times, want never: only the daemon's session may unlink", *calls)
	}
	if after := listing(t, root); !slices.Equal(before, after) {
		t.Errorf("the data dirs changed:\n before %v\n after  %v", before, after)
	}
	if n := keys.count(); n != items {
		t.Errorf("keyring holds %d items, want the %d it had", n, items)
	}
}

func TestLogoutWhileUnlinkedPointsAtPurge(t *testing.T) {
	serveFakeDaemon(t, &fakeDaemon{wipeErr: ipc.ErrNotLinked})
	code, _, errOut := runCLI("logout", "--confirm")
	if code != 1 || !strings.Contains(errOut, "no WhatsApp device is linked") || !strings.Contains(errOut, "nuncio purge --confirm") {
		t.Errorf("exit %d, stderr %q; want 1, the not-linked line and the purge hint", code, errOut)
	}
}

func TestPurgeAsksTheRunningDaemonWithoutTheNetwork(t *testing.T) {
	root, _ := populatedData(t)
	keys := fakeKeys(t)
	d := &fakeDaemon{session: ipc.SessionLinked}
	serveFakeDaemon(t, d)
	before := listing(t, root)

	code, out, errOut := runCLI("purge", "--confirm")
	if code != 0 || !strings.Contains(out, "purged") || !strings.Contains(out, "WhatsApp was not contacted") {
		t.Errorf("exit %d, stdout %q, stderr %q; want 0 and purged", code, out, errOut)
	}
	if got := d.wipeCalls(); !slices.Equal(got, []bool{false}) {
		t.Errorf("wipe requests %v, want exactly one without unlink", got)
	}
	// The daemon does the deleting: the CLI touches nothing under a running daemon.
	if after := listing(t, root); !slices.Equal(before, after) {
		t.Errorf("the CLI deleted files under a running daemon:\n before %v\n after  %v", before, after)
	}
	if n := keys.count(); n != 4 {
		t.Errorf("the CLI emptied the keyring under a running daemon: %d items left", n)
	}
}

func TestPurgeWithoutADaemonLeavesNothingBehind(t *testing.T) {
	_, dataDir := populatedData(t)
	keys := fakeKeys(t)
	runtime := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", runtime)
	// A socket left behind by a dead daemon.
	sock := filepath.Join(runtime, "nuncio", "core.sock")
	if err := os.MkdirAll(filepath.Dir(sock), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	ln.(*net.UnixListener).SetUnlinkOnClose(false)
	_ = ln.Close()
	calls := fakeConnect(t)

	code, out, errOut := runCLI("purge", "--confirm")
	if code != 0 || !strings.Contains(out, "purged") {
		t.Errorf("exit %d, stdout %q, stderr %q; want 0 and purged", code, out, errOut)
	}
	if _, err := os.Lstat(dataDir); !os.IsNotExist(err) {
		entries, _ := os.ReadDir(dataDir)
		t.Errorf("data dir still exists (%v), holding %v", err, entries)
	}
	if n := keys.count(); n != 0 {
		t.Errorf("keyring holds %d items, want 0", n)
	}
	if _, err := os.Lstat(sock); !os.IsNotExist(err) {
		t.Errorf("stale socket still exists: %v", err)
	}
	if *calls != 0 {
		t.Errorf("connected %d times, want never", *calls)
	}
}

// Something that accepts on core.sock but never answers may be a wedged daemon: purge deletes
// nothing under it.
func TestPurgeRefusesWhileSomethingListensButDoesNotAnswer(t *testing.T) {
	root, _ := populatedData(t)
	keys := fakeKeys(t)
	runtime := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", runtime)
	sock := filepath.Join(runtime, "nuncio", "core.sock")
	if err := os.MkdirAll(filepath.Dir(sock), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			t.Cleanup(func() { _ = conn.Close() })
		}
	}()
	before := listing(t, root)

	code, _, errOut := runCLI("purge", "--confirm")
	if code != 1 || !strings.Contains(errOut, "does not answer") || !strings.Contains(errOut, "Nothing was deleted") {
		t.Errorf("exit %d, stderr %q; want 1 and the does-not-answer refusal", code, errOut)
	}
	if after := listing(t, root); !slices.Equal(before, after) {
		t.Errorf("the data dirs changed:\n before %v\n after  %v", before, after)
	}
	if n := keys.count(); n != 4 {
		t.Errorf("keyring holds %d items, want the 4 it had", n)
	}
}

// qrLinker is a Linker whose QR flow reports success at once, as a scanned code would; it never
// touches the network.
type qrLinker struct{}

func (qrLinker) QR(context.Context) (<-chan wa.Code, error) {
	codes := make(chan wa.Code, 1)
	codes <- wa.Code{Event: "success"}
	close(codes)
	return codes, nil
}

func (qrLinker) Phone(context.Context, string) (string, error) {
	return "", errors.New("qrLinker: no phone flow")
}

// After pairing, link reports the device as linked, says how to start the daemon and exits: it
// never waits for a message and never sends one.
func TestLinkExitsAfterReportingLinkedAndSendsNothing(t *testing.T) {
	isolateData(t)
	// A link needs the keyring for the data key, even one that never pairs.
	fakeKeys(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir()) // no core.sock there
	prepare := prepareLink
	prepareLink = func(c wa.Consent, _ *whatsmeow.Client) (wa.Prepared, error) {
		return wa.Prepared{Linker: qrLinker{}, Consent: c}, nil
	}
	t.Cleanup(func() { prepareLink = prepare })

	type result struct {
		code     int
		out, err string
	}
	done := make(chan result, 1)
	go func() {
		code, out, errOut := runCLI("link", "--qr", "--consent")
		done <- result{code, out, errOut}
	}()
	var r result
	select {
	case r = <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("nuncio link still running 5 s after the device was linked; it must exit")
	}
	if r.code != 0 {
		t.Errorf("exit %d, stderr %q; want 0", r.code, r.err)
	}
	if !strings.Contains(r.out, "nuncio: linked") || !strings.HasSuffix(r.out, "systemctl --user start nunciod\n") {
		t.Errorf("stdout %q, want the linked line and, last, how to start the daemon", r.out)
	}
	for _, s := range []string{r.out, r.err} {
		if strings.Contains(s, "send one message") || strings.Contains(s, "reply") || strings.Contains(s, "mensaje recibido") {
			t.Errorf("output %q mentions waiting for a message or a reply; link must send nothing", s)
		}
	}
}
