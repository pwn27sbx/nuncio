package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
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

	mu    sync.Mutex
	sends []ipc.SendRequest
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

func TestReplyableOnlyAllowsDirectUserChats(t *testing.T) {
	user := types.NewJID("15550001234", types.DefaultUserServer)
	cases := []struct {
		name string
		info types.MessageInfo
		want bool
	}{
		{"direct user chat", msgInfo(user, user, false, false), true},
		{"direct chat addressed by lid", msgInfo(types.NewJID("123456789", types.HiddenUserServer), user, false, false), true},
		{"group", msgInfo(types.NewJID("15550001234-1600000000", types.GroupServer), user, false, true), false},
		{"group without the IsGroup flag", msgInfo(types.NewJID("15550001234-1600000000", types.GroupServer), user, false, false), false},
		{"status update", msgInfo(types.StatusBroadcastJID, user, false, false), false},
		{"broadcast list", msgInfo(types.NewJID("1600000000", types.BroadcastServer), user, false, true), false},
		{"newsletter", msgInfo(types.NewJID("120363000000000000", types.NewsletterServer), user, false, false), false},
		{"from me", msgInfo(user, user, true, false), false},
	}
	for _, c := range cases {
		if got := replyable(c.info); got != c.want {
			t.Errorf("%s: replyable(%s) = %v, want %v", c.name, c.info.Chat, got, c.want)
		}
	}
}

func msgInfo(chat, sender types.JID, fromMe, group bool) types.MessageInfo {
	return types.MessageInfo{MessageSource: types.MessageSource{Chat: chat, Sender: sender, IsFromMe: fromMe, IsGroup: group}}
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
	if code != 1 || out != "" || errOut != "nuncio: not sent: the WhatsApp session is offline\n" {
		t.Errorf("exit %d, stdout %q, stderr %q; want 1 and the not-sent line", code, out, errOut)
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
	for _, cmd := range []string{"nuncio status", "nuncio chats", "nuncio send <jid> <message>", "nuncio focus <jid>", "nuncio link --qr", "nuncio link --phone", "not confirmed"} {
		if !strings.Contains(usage, cmd) {
			t.Errorf("usage does not document %q:\n%s", cmd, usage)
		}
	}
}

// linkedDevice stores a whatsmeow device with an ID in the isolated data dir, as a finished
// link would. Nothing here touches the network.
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
	device, err := wa.OpenDevice(context.Background(), db)
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
}
