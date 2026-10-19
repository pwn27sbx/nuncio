package main

import (
	"bytes"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/wa"
	"go.mau.fi/whatsmeow/types"
)

// fakeDaemon answers the wire protocol with a fixed unlinked state.
type fakeDaemon struct{}

func (fakeDaemon) Hello(string) ipc.Status { return fakeDaemon{}.Status() }
func (fakeDaemon) Status() ipc.Status {
	return ipc.Status{Daemon: "nunciod/0.1.0", Session: ipc.SessionUnlinked}
}
func (fakeDaemon) Chats() []ipc.Chat { return nil }

// serveFakeDaemon listens at $XDG_RUNTIME_DIR/nuncio/core.sock, where ipc.Connect looks.
func serveFakeDaemon(t *testing.T) {
	t.Helper()
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
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() { _ = ipc.ServeConn(conn, fakeDaemon{}) }()
		}
	}()
}

func runCLI(args ...string) (code int, stdout, stderr string) {
	var out, errOut bytes.Buffer
	code = cli(args, &out, &errOut)
	return code, out.String(), errOut.String()
}

func TestStatusPrintsTheDaemonState(t *testing.T) {
	serveFakeDaemon(t)
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
