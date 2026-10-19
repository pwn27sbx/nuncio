package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/godbus/dbus/v5"
	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

var (
	binDir     string
	nunciodBin string
	nuncioBin  string
)

// TestMain builds the two binaries once. The tests drive the real processes,
// because activation and single instance are properties of the shipped
// programs, not of a function.
func TestMain(m *testing.M) {
	code := runMain(m)
	os.Exit(code)
}

func runMain(m *testing.M) int {
	dir, err := os.MkdirTemp("", "nuncio-test-bin")
	if err != nil {
		fmt.Fprintln(os.Stderr, "mkdtemp:", err)
		return 1
	}
	defer func() { _ = os.RemoveAll(dir) }()
	binDir = dir
	nunciodBin = filepath.Join(dir, "nunciod")
	nuncioBin = filepath.Join(dir, "nuncio")

	for _, target := range []struct{ out, pkg string }{{nunciodBin, "."}, {nuncioBin, "../nuncio"}} {
		cmd := exec.Command("go", "build", "-o", target.out, target.pkg)
		if out, err := cmd.CombinedOutput(); err != nil {
			fmt.Fprintf(os.Stderr, "go build %s: %v\n%s", target.pkg, err, out)
			return 1
		}
	}
	return m.Run()
}

func repoRoot(t *testing.T) string {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("go.mod not found above the test working directory")
		}
		dir = parent
	}
}

// daemonEnv is deliberately minimal: a test daemon must never see the user's
// real session bus, runtime directory or data directory (where a linked device
// would make it connect to WhatsApp).
func daemonEnv(runtimeDir, busAddress string) []string {
	return []string{
		"PATH=" + os.Getenv("PATH"),
		"XDG_DATA_HOME=" + dataHome(runtimeDir),
		"XDG_RUNTIME_DIR=" + runtimeDir,
		"DBUS_SESSION_BUS_ADDRESS=" + busAddress,
	}
}

// dataHome is the empty XDG_DATA_HOME a test daemon gets: it holds no
// nuncio.db, so the daemon is unlinked and connects to nothing.
func dataHome(runtimeDir string) string {
	return filepath.Join(runtimeDir, "data")
}

// noBus is an address nothing listens on, so the daemon runs without the
// D-Bus face.
func noBus(t *testing.T) string {
	t.Helper()
	return "unix:path=" + filepath.Join(t.TempDir(), "no-such-bus")
}

func socketPath(runtimeDir string) string {
	return filepath.Join(runtimeDir, "nuncio", "core.sock")
}

func startDaemon(t *testing.T, runtimeDir, busAddress string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command(nunciodBin)
	cmd.Env = daemonEnv(runtimeDir, busAddress)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	cmd.Stdout = io.Discard
	if err := cmd.Start(); err != nil {
		t.Fatalf("start nunciod: %v", err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	t.Cleanup(func() {
		if t.Failed() && stderr.Len() > 0 {
			t.Logf("nunciod stderr: %s", stderr.String())
		}
	})
	return cmd
}

func waitForDaemon(t *testing.T, socket string) *ipc.Client {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	var lastErr error
	for time.Now().Before(deadline) {
		c, err := ipc.Dial(socket, "test/0.1.0")
		if err == nil {
			t.Cleanup(func() { _ = c.Close() })
			return c
		}
		lastErr = err
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("daemon never became reachable on %s: %v", socket, lastErr)
	return nil
}

// makeStaleSocket leaves a real socket inode with no listener behind, the way
// a killed daemon does.
func makeStaleSocket(t *testing.T, socket string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(socket), 0o700); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	ln, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	ln.(*net.UnixListener).SetUnlinkOnClose(false)
	if err := ln.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	st, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stale socket was not left behind: %v", err)
	}
	if st.Mode()&os.ModeSocket == 0 {
		t.Fatalf("%s is not a socket", socket)
	}
}

func assertSocketPermissions(t *testing.T, socket string) {
	t.Helper()
	st, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}
	if st.Mode()&os.ModeSocket == 0 {
		t.Fatalf("%s is not a socket", socket)
	}
	if got := st.Mode().Perm(); got != 0o600 {
		t.Errorf("socket mode = %o, want 600", got)
	}
	dir, err := os.Stat(filepath.Dir(socket))
	if err != nil {
		t.Fatalf("stat socket directory: %v", err)
	}
	if got := dir.Mode().Perm(); got != 0o700 {
		t.Errorf("socket directory mode = %o, want 700", got)
	}
}

func assertFixtureChats(t *testing.T, chats []ipc.Chat) {
	t.Helper()
	if len(chats) != 3 {
		t.Fatalf("got %d chats, want the 3 of the fixture", len(chats))
	}
	first := chats[0]
	if first.JID != "15551230001@s.whatsapp.net" || first.Name != "Ada Lovelace" {
		t.Errorf("first chat = %+v, want Ada Lovelace", first)
	}
	if first.Unread != 2 || !first.Pinned || first.Archived {
		t.Errorf("first chat flags = %+v, want unread 2, pinned, not archived", first)
	}
	if first.LastMessage == nil || first.LastMessage.Text != "See you at 18:00" || first.LastMessage.Timestamp != "2026-10-18T17:55:04Z" {
		t.Errorf("first chat last message = %+v", first.LastMessage)
	}
	if second := chats[1]; second.LastMessage == nil || !second.LastMessage.FromMe {
		t.Errorf("second chat last message = %+v, want a message from me", chats[1].LastMessage)
	}
	third := chats[2]
	if third.JID != "120363000000000000@g.us" || !third.Archived || !third.Muted || third.Unread != 17 || third.LastMessage != nil {
		t.Errorf("group chat = %+v, want an archived, muted group with 17 unread and no last message", third)
	}
}

func TestDaemonServesProtocolOnSocket(t *testing.T) {
	rt := t.TempDir()
	startDaemon(t, rt, noBus(t))
	socket := socketPath(rt)

	c := waitForDaemon(t, socket)
	assertSocketPermissions(t, socket)

	st, err := c.Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	want := ipc.StatusResponse{V: 1, Op: ipc.OpStatus, Daemon: "nunciod/0.1.0", Session: ipc.SessionUnlinked}
	if st != want {
		t.Fatalf("Status() = %+v, want %+v", st, want)
	}

	chats, err := c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	assertFixtureChats(t, chats.Chats)

	// The limit really is applied by the daemon, not just by the fixture.
	limited, err := c.Chats(1)
	if err != nil {
		t.Fatalf("Chats(1): %v", err)
	}
	if len(limited.Chats) != 1 || limited.Chats[0].JID != "15551230001@s.whatsapp.net" {
		t.Fatalf("Chats(1) = %+v, want only the first chat", limited.Chats)
	}
}

func TestDaemonWithoutDatabaseIsUnlinkedAndCreatesNoDataDir(t *testing.T) {
	rt := t.TempDir()
	startDaemon(t, rt, noBus(t))
	st, err := waitForDaemon(t, socketPath(rt)).Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	if st.Linked || st.Session != ipc.SessionUnlinked {
		t.Errorf("Status() = %+v, want unlinked", st)
	}
	dir := filepath.Join(dataHome(rt), "nuncio")
	if _, err := os.Stat(dir); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("stat %s = %v, want it not to exist: starting the daemon must not create the data dir", dir, err)
	}
}

type fixedSession wa.State

func (s fixedSession) State() wa.State { return wa.State(s) }

func (s fixedSession) SendText(string, string) (ipc.Message, error) {
	return ipc.Message{}, errors.New("a fixed session cannot send")
}

func (s fixedSession) Logout(context.Context) error {
	return errors.New("a fixed session cannot log out")
}
func (s fixedSession) Close() {}

func (s fixedSession) DownloadMedia(string, string) (string, string, error) {
	if wa.State(s) == wa.StateUnlinked {
		return "", "", ipc.ErrNotLinked
	}
	return "", "", errors.New("a fixed session has no media")
}

func TestStatusReportsTheSessionState(t *testing.T) {
	for _, tc := range []struct {
		state  wa.State
		linked bool
	}{
		{wa.StateUnlinked, false},
		{wa.StateLinked, true},
		{wa.StateOffline, true},
	} {
		d, err := newDaemon(fixedSession(tc.state), nil)
		if err != nil {
			t.Fatalf("newDaemon: %v", err)
		}
		want := ipc.Status{Daemon: daemonVersion, Linked: tc.linked, Session: string(tc.state)}
		if got := d.Status(); got != want {
			t.Errorf("Status() with a %s session = %+v, want %+v", tc.state, got, want)
		}
		if got := d.Hello("test/0.1.0"); got != want {
			t.Errorf("Hello() with a %s session = %+v, want %+v", tc.state, got, want)
		}
	}
}

func TestDaemonReplacesStaleSocket(t *testing.T) {
	rt := t.TempDir()
	socket := socketPath(rt)
	makeStaleSocket(t, socket)
	before, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat stale socket: %v", err)
	}

	startDaemon(t, rt, noBus(t))
	c := waitForDaemon(t, socket)
	if _, err := c.Status(); err != nil {
		t.Fatalf("Status: %v", err)
	}

	after, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}
	if os.SameFile(before, after) {
		t.Fatalf("daemon served on the stale socket file instead of replacing it")
	}
}

func TestDaemonLeavesLiveSocketAlone(t *testing.T) {
	rt := t.TempDir()
	socket := socketPath(rt)
	bus := noBus(t)

	first := startDaemon(t, rt, bus)
	waitForDaemon(t, socket)
	before, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}

	second := exec.Command(nunciodBin)
	second.Env = daemonEnv(rt, bus)
	var stderr bytes.Buffer
	second.Stderr = &stderr
	start := time.Now()
	if err := second.Run(); err != nil {
		t.Fatalf("second daemon: %v (stderr %q)", err, stderr.String())
	}
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("second daemon took %v to notice the running one", elapsed)
	}
	if !strings.Contains(stderr.String(), "already") {
		t.Errorf("second daemon stderr = %q, want a note that a daemon is already running", stderr.String())
	}
	if err := first.Process.Signal(syscall.Signal(0)); err != nil {
		t.Fatalf("first daemon is gone after the second start: %v", err)
	}

	after, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}
	if !os.SameFile(before, after) {
		t.Fatalf("second daemon replaced the live socket")
	}
	if _, err := waitForDaemon(t, socket).Status(); err != nil {
		t.Fatalf("first daemon stopped serving: %v", err)
	}
}

// privateBus is a dbus-daemon of our own, so the tests never touch the user's
// session bus or their ~/.local/share/dbus-1/services.
type privateBus struct {
	address string
}

func startPrivateBus(t *testing.T, runtimeDir string) *privateBus {
	t.Helper()
	daemon, err := exec.LookPath("dbus-daemon")
	if err != nil {
		t.Skip("dbus-daemon is not installed: skipping the D-Bus spike")
	}
	repo := repoRoot(t)
	dir := t.TempDir()
	socket := filepath.Join(dir, "bus")
	configFile := filepath.Join(dir, "bus.conf")
	dataHome := filepath.Join(dir, "data-home")
	if err := os.MkdirAll(dataHome, 0o700); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	config := `<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:path=` + socket + `</listen>
  <auth>EXTERNAL</auth>
  <standard_session_servicedirs/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
`
	if err := os.WriteFile(configFile, []byte(config), 0o600); err != nil {
		t.Fatalf("write bus config: %v", err)
	}

	cmd := exec.Command(daemon, "--config-file="+configFile, "--nofork", "--print-address=1")
	cmd.Env = []string{
		// The activation file runs `env nunciod`, so the bus and everything
		// it starts must find the freshly built daemon on PATH.
		"PATH=" + binDir + ":" + os.Getenv("PATH"),
		"HOME=" + dir,
		"XDG_DATA_DIRS=" + filepath.Join(repo, "testdata", "dbus"),
		"XDG_DATA_HOME=" + dataHome,
		"XDG_RUNTIME_DIR=" + runtimeDir,
		"DBUS_SESSION_BUS_ADDRESS=unix:path=" + socket,
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatalf("stdout pipe: %v", err)
	}
	// A *os.File keeps os/exec from starting a copier goroutine: an activated
	// daemon inherits this stderr, and a copier would make Wait block until
	// the daemon exits.
	stderrFile, err := os.Create(filepath.Join(dir, "dbus-daemon.log"))
	if err != nil {
		t.Fatalf("create the bus log: %v", err)
	}
	defer func() { _ = stderrFile.Close() }()
	cmd.Stderr = stderrFile
	busLog := func() string {
		data, err := os.ReadFile(stderrFile.Name())
		if err != nil {
			return ""
		}
		return string(data)
	}
	if err := cmd.Start(); err != nil {
		t.Fatalf("start dbus-daemon: %v", err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})

	addrs := make(chan string, 1)
	go func() {
		line, err := bufio.NewReader(stdout).ReadString('\n')
		if err != nil {
			close(addrs)
			return
		}
		addrs <- strings.TrimSpace(line)
	}()
	select {
	case address, ok := <-addrs:
		if !ok || address == "" {
			t.Fatalf("dbus-daemon printed no address; log: %s", busLog())
		}
		return &privateBus{address: address}
	case <-time.After(15 * time.Second):
		t.Fatalf("timed out waiting for dbus-daemon's address; log: %s", busLog())
		return nil
	}
}

// killActivatedDaemon kills whatever owns the daemon name on this private bus,
// so a test never leaks a daemon process behind the bus it started.
func killActivatedDaemon(t *testing.T, conn *dbus.Conn) {
	t.Cleanup(func() {
		call := conn.Object("org.freedesktop.DBus", dbus.ObjectPath("/org/freedesktop/DBus")).
			Call("org.freedesktop.DBus.GetConnectionUnixProcessID", 0, ipc.DaemonName)
		var pid uint32
		if call.Err != nil || call.Store(&pid) != nil || pid == 0 {
			return
		}
		if p, err := os.FindProcess(int(pid)); err == nil {
			_ = p.Kill()
		}
	})
}

func busClient(t *testing.T, address string) *dbus.Conn {
	t.Helper()
	conn, err := dbus.Dial(address)
	if err != nil {
		t.Fatalf("dial %s: %v", address, err)
	}
	if err := conn.Auth(nil); err != nil {
		t.Fatalf("auth on %s: %v", address, err)
	}
	if err := conn.Hello(); err != nil {
		t.Fatalf("hello on %s: %v", address, err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	return conn
}

func TestDBusActivationStartsDaemonInUnderOneSecond(t *testing.T) {
	rt := t.TempDir()
	bus := startPrivateBus(t, rt)
	conn := busClient(t, bus.address)
	killActivatedDaemon(t, conn)
	socket := socketPath(rt)
	if _, err := os.Stat(socket); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("something is already serving %s before the activation test: %v", socket, err)
	}

	start := time.Now()
	var st ipc.Status
	if err := conn.Object(ipc.DaemonName, dbus.ObjectPath(ipc.ObjectPath)).
		Call(ipc.Interface+".Status", 0).Store(&st); err != nil {
		t.Fatalf("activating %s.Status call: %v", ipc.DaemonName, err)
	}
	c := waitForDaemon(t, socket)
	overSocket, err := c.Status()
	if err != nil {
		t.Fatalf("Status after activation: %v", err)
	}
	elapsed := time.Since(start)

	t.Logf("D-Bus activation round trip (call + socket handshake + status): %.0f ms",
		float64(elapsed.Microseconds())/1000)
	if elapsed >= time.Second {
		t.Fatalf("activation round trip took %v, want under 1s", elapsed)
	}
	if st.Linked || st.Session != ipc.SessionUnlinked || st.Unread != 0 || st.Daemon != "nunciod/0.1.0" {
		t.Errorf("D-Bus Status = %+v, want the unlinked M0 state of nunciod/0.1.0", st)
	}
	if !reflect.DeepEqual(st, ipc.Status{
		Daemon: overSocket.Daemon, Linked: overSocket.Linked, Session: overSocket.Session, Unread: overSocket.Unread,
	}) {
		t.Errorf("D-Bus Status %+v and socket Status %+v disagree", st, overSocket)
	}

	var unread dbus.Variant
	if err := conn.Object(ipc.DaemonName, dbus.ObjectPath(ipc.ObjectPath)).
		Call("org.freedesktop.DBus.Properties.Get", 0, ipc.Interface, "UnreadCount").Store(&unread); err != nil {
		t.Fatalf("read UnreadCount property: %v", err)
	}
	if got, ok := unread.Value().(int32); !ok || got != 0 {
		t.Fatalf("UnreadCount = %#v, want int32(0)", unread.Value())
	}
}

func TestDaemonExitsWhenNameIsAlreadyOwned(t *testing.T) {
	rt := t.TempDir()
	bus := startPrivateBus(t, rt)
	conn := busClient(t, bus.address)
	reply, err := conn.RequestName(ipc.DaemonName, dbus.NameFlagDoNotQueue)
	if err != nil {
		t.Fatalf("RequestName: %v", err)
	}
	if reply != dbus.RequestNameReplyPrimaryOwner {
		t.Fatalf("test could not take %s for itself: %v", ipc.DaemonName, reply)
	}

	cmd := exec.Command(nunciodBin)
	cmd.Env = daemonEnv(rt, bus.address)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("nunciod with %s taken: %v (stderr %q)", ipc.DaemonName, err, stderr.String())
	}
	if !strings.Contains(stderr.String(), "already") {
		t.Errorf("stderr = %q, want a note about the taken name", stderr.String())
	}
	if _, err := os.Stat(socketPath(rt)); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("daemon left a socket at %s; it should exit before listening", socketPath(rt))
	}
}

func TestDBusSingleInstanceSecondDaemonExitsZero(t *testing.T) {
	rt := t.TempDir()
	bus := startPrivateBus(t, rt)
	first := startDaemon(t, rt, bus.address)
	socket := socketPath(rt)
	waitForDaemon(t, socket)
	before, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}

	second := exec.Command(nunciodBin)
	second.Env = daemonEnv(rt, bus.address)
	var stderr bytes.Buffer
	second.Stderr = &stderr
	start := time.Now()
	if err := second.Run(); err != nil {
		t.Fatalf("second daemon exited %v, want 0 (stderr %q)", err, stderr.String())
	}
	t.Logf("second daemon exited 0 after %.0f ms", float64(time.Since(start).Microseconds())/1000)

	if err := first.Process.Signal(syscall.Signal(0)); err != nil {
		t.Fatalf("first daemon is gone: %v", err)
	}
	after, err := os.Stat(socket)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}
	if !os.SameFile(before, after) {
		t.Fatalf("second daemon replaced the live socket")
	}
	st, err := waitForDaemon(t, socket).Status()
	if err != nil {
		t.Fatalf("first daemon stopped serving: %v", err)
	}
	if st.Session != ipc.SessionUnlinked {
		t.Errorf("surviving daemon session = %q, want %q", st.Session, ipc.SessionUnlinked)
	}

	conn := busClient(t, bus.address)
	var owner string
	if err := conn.Object("org.freedesktop.DBus", dbus.ObjectPath("/org/freedesktop/DBus")).
		Call("org.freedesktop.DBus.GetNameOwner", 0, ipc.DaemonName).Store(&owner); err != nil {
		t.Fatalf("the name %s is no longer owned after the second start: %v", ipc.DaemonName, err)
	}
}

const wantCLIOutput = "linked=false session=unlinked unread=0\n"

func TestCLIPrintsStatusFromRunningDaemon(t *testing.T) {
	rt := t.TempDir()
	bus := noBus(t)
	startDaemon(t, rt, bus)
	waitForDaemon(t, socketPath(rt))

	out, err := runCLI(t, daemonEnv(rt, bus))
	if err != nil {
		t.Fatalf("nuncio: %v (output %q)", err, out)
	}
	if out != wantCLIOutput {
		t.Fatalf("nuncio printed %q, want %q", out, wantCLIOutput)
	}
}

func TestCLIActivatesDaemonOverDBus(t *testing.T) {
	rt := t.TempDir()
	bus := startPrivateBus(t, rt)
	killActivatedDaemon(t, busClient(t, bus.address))

	env := []string{
		// The activation file runs `env nunciod`, so the built daemon has to
		// be findable on PATH.
		"PATH=" + binDir + ":" + os.Getenv("PATH"),
		"XDG_RUNTIME_DIR=" + rt,
		"DBUS_SESSION_BUS_ADDRESS=" + bus.address,
	}
	if _, err := os.Stat(socketPath(rt)); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("something is already serving %s before the CLI activation test: %v", socketPath(rt), err)
	}

	start := time.Now()
	out, err := runCLI(t, env)
	elapsed := time.Since(start)
	if err != nil {
		t.Fatalf("nuncio: %v (output %q)", err, out)
	}
	t.Logf("CLI activation round trip (activation + handshake + status + print): %.0f ms",
		float64(elapsed.Microseconds())/1000)
	if out != wantCLIOutput {
		t.Fatalf("nuncio printed %q, want %q", out, wantCLIOutput)
	}
	if elapsed >= time.Second {
		t.Fatalf("CLI activation took %v, want under 1s", elapsed)
	}
	if _, err := waitForDaemon(t, socketPath(rt)).Status(); err != nil {
		t.Fatalf("the CLI did not leave a serving daemon behind: %v", err)
	}
}

func runCLI(t *testing.T, env []string) (string, error) {
	t.Helper()
	cmd := exec.Command(nuncioBin)
	cmd.Env = env
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		t.Logf("nuncio stderr: %s", stderr.String())
	}
	return stdout.String(), err
}

func TestEmbeddedChatListMatchesContractFixture(t *testing.T) {
	embedded, err := fixtureChats()
	if err != nil {
		t.Fatalf("fixtureChats: %v", err)
	}
	data, err := os.ReadFile(filepath.Join(repoRoot(t), "proto", "fixtures", "chats_response.json"))
	if err != nil {
		t.Fatalf("read contract fixture: %v", err)
	}
	var resp ipc.ChatsResponse
	if err := json.Unmarshal(data, &resp); err != nil {
		t.Fatalf("unmarshal contract fixture: %v", err)
	}
	if !reflect.DeepEqual(embedded, resp.Chats) {
		t.Fatalf("embedded chat list drifted from proto/fixtures/chats_response.json:\n embedded: %+v\n fixture:  %+v",
			embedded, resp.Chats)
	}
}

// fakeWAClient is a linked whatsmeow stand-in that never opens a socket; emit feeds it events.
// SendText answers sendID and sendTime and counts the sends.
type fakeWAClient struct {
	handlers []func(any)
	sendID   string
	sendTime time.Time
	sends    atomic.Int64
	// logouts counts Logout calls; onLogout, when set, runs inside each one; Logout answers
	// logoutErr.
	logouts   atomic.Int64
	onLogout  func()
	logoutErr error
}

func (f *fakeWAClient) Logout(context.Context) error {
	f.logouts.Add(1)
	if f.onLogout != nil {
		f.onLogout()
	}
	return f.logoutErr
}

func (f *fakeWAClient) SendText(context.Context, string, string) (string, time.Time, error) {
	f.sends.Add(1)
	return f.sendID, f.sendTime, nil
}

func (f *fakeWAClient) HasDevice() bool             { return true }
func (f *fakeWAClient) Connect() error              { f.emit(&waevents.Connected{}); return nil }
func (f *fakeWAClient) Disconnect()                 {}
func (f *fakeWAClient) AddEventHandler(h func(any)) { f.handlers = append(f.handlers, h) }

func (f *fakeWAClient) emit(e any) {
	for _, h := range f.handlers {
		h(e)
	}
}

func textMessage(chat types.JID, id string, fromMe bool, ts time.Time, text string) *waevents.Message {
	return &waevents.Message{
		Info: types.MessageInfo{
			MessageSource: types.MessageSource{Chat: chat, Sender: chat, IsFromMe: fromMe},
			ID:            id,
			Timestamp:     ts,
		},
		Message: &waE2E.Message{Conversation: proto.String(text)},
	}
}

// serveSocket serves h, with events from b, on a private socket and returns its path.
func serveSocket(t *testing.T, h ipc.Handler, b *ipc.Broker) string {
	t.Helper()
	sock := filepath.Join(t.TempDir(), "core.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen %s: %v", sock, err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { _ = serve(ctx, ln, h, b); close(done) }()
	t.Cleanup(func() { cancel(); <-done })
	return sock
}

// serveInProcess serves h on a private socket and returns a connected client.
func serveInProcess(t *testing.T, h ipc.Handler) *ipc.Client {
	t.Helper()
	sock := serveSocket(t, h, nil)
	c, err := ipc.Dial(sock, "test/0.1.0")
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return c
}

func openTestStore(t *testing.T) *sql.DB {
	t.Helper()
	db, err := store.Open(filepath.Join(t.TempDir(), "nuncio.db"))
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	return db
}

func TestDaemonServesChatsMessagesAndUnreadFromTheStore(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeWAClient{}
	session := wa.NewSession(fake, db, nil)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	group := types.NewJID("120363000000000000", types.GroupServer)
	t0 := time.Date(2026, 10, 18, 17, 50, 0, 0, time.UTC)
	fake.emit(textMessage(group, "G1", false, t0, "group hello"))
	fake.emit(textMessage(ada, "A1", false, t0.Add(time.Minute), "first"))
	fake.emit(textMessage(ada, "A2", false, t0.Add(2*time.Minute), "second"))
	fake.emit(textMessage(ada, "A3", true, t0.Add(3*time.Minute), "mine"))

	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	c := serveInProcess(t, d)

	st, err := c.Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	if st.Unread != 3 || st.Session != ipc.SessionLinked {
		t.Errorf("Status() = %+v, want linked with 3 unread", st)
	}

	chats, err := c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	wantChats := []ipc.Chat{
		{JID: ada.String(), Unread: 2, LastMessage: &ipc.Message{
			ID: "A3", Kind: ipc.KindText, Text: "mine", FromMe: true, Timestamp: "2026-10-18T17:53:00Z"}},
		{JID: group.String(), Unread: 1, LastMessage: &ipc.Message{
			ID: "G1", Kind: ipc.KindText, Text: "group hello", Timestamp: "2026-10-18T17:50:00Z"}},
	}
	if !reflect.DeepEqual(chats.Chats, wantChats) {
		t.Errorf("Chats() = %s, want %s", jsonOf(t, chats.Chats), jsonOf(t, wantChats))
	}

	msgs, err := c.Messages(ada.String(), 2)
	if err != nil {
		t.Fatalf("Messages: %v", err)
	}
	wantMsgs := ipc.MessagesResponse{V: 1, Op: ipc.OpMessages, JID: ada.String(), Messages: []ipc.Message{
		{ID: "A3", Kind: ipc.KindText, Text: "mine", FromMe: true, Timestamp: "2026-10-18T17:53:00Z"},
		{ID: "A2", Kind: ipc.KindText, Text: "second", Timestamp: "2026-10-18T17:52:00Z"},
	}}
	if !reflect.DeepEqual(msgs, wantMsgs) {
		t.Errorf("Messages() = %+v, want %+v", msgs, wantMsgs)
	}

	// Marking a chat read clears its unread, and the totals follow.
	if err := session.MarkChatRead(ada.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	if st, err := c.Status(); err != nil || st.Unread != 1 {
		t.Errorf("Status() after marking ada read = %+v, %v; want 1 unread", st, err)
	}
	chats, err = c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	if chats.Chats[0].Unread != 0 || chats.Chats[1].Unread != 1 {
		t.Errorf("unread after marking ada read = %d and %d, want 0 and 1", chats.Chats[0].Unread, chats.Chats[1].Unread)
	}
}

// App-state changes from the phone (pin, archive, mute) show in the next chats reply; a mute
// that has already ended does not.
func TestDaemonChatsReplyCarriesTheAppStateFlags(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeWAClient{}
	session := wa.NewSession(fake, db, nil)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	grace := types.NewJID("15551230002", types.DefaultUserServer)
	group := types.NewJID("120363000000000000", types.GroupServer)
	t0 := time.Date(2026, 10, 18, 17, 50, 0, 0, time.UTC)
	fake.emit(textMessage(ada, "A1", false, t0.Add(2*time.Minute), "first"))
	fake.emit(textMessage(grace, "G1", false, t0.Add(time.Minute), "hello"))
	fake.emit(&waevents.Pin{JID: ada, Action: &waSyncAction.PinAction{Pinned: proto.Bool(true)}})
	fake.emit(&waevents.Mute{JID: ada, Action: &waSyncAction.MuteAction{
		Muted: proto.Bool(true), MuteEndTimestamp: proto.Int64(time.Now().Add(time.Hour).UnixMilli())}})
	fake.emit(&waevents.Archive{JID: grace, Action: &waSyncAction.ArchiveChatAction{Archived: proto.Bool(true)}})
	fake.emit(&waevents.Mute{JID: grace, Action: &waSyncAction.MuteAction{
		Muted: proto.Bool(true), MuteEndTimestamp: proto.Int64(time.Now().Add(-time.Hour).UnixMilli())}})
	fake.emit(&waevents.Mute{JID: group, Action: &waSyncAction.MuteAction{
		Muted: proto.Bool(true), MuteEndTimestamp: proto.Int64(-1)}})

	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	chats, err := serveInProcess(t, d).Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	got := map[string][3]bool{}
	for _, c := range chats.Chats {
		got[c.JID] = [3]bool{c.Pinned, c.Archived, c.Muted}
	}
	want := map[string][3]bool{
		ada.String():   {true, false, true},
		grace.String(): {false, true, false},
		group.String(): {false, false, true},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("chat flags [pinned archived muted] = %v, want %v", got, want)
	}
}

func TestDaemonWithoutStoreServesTheFixtureChatsAndNoMessages(t *testing.T) {
	d, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	c := serveInProcess(t, d)
	chats, err := c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	assertFixtureChats(t, chats.Chats)
	msgs, err := c.Messages("15551230001@s.whatsapp.net", 50)
	if err != nil {
		t.Fatalf("Messages: %v", err)
	}
	if len(msgs.Messages) != 0 || msgs.Messages == nil {
		t.Errorf("Messages() while unlinked = %#v, want an empty list", msgs.Messages)
	}
}

func TestDaemonAnswersEmptyMessagesForAChatNotInTheStore(t *testing.T) {
	d, err := newDaemon(fixedSession(wa.StateLinked), openTestStore(t))
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	c := serveInProcess(t, d)
	msgs, err := c.Messages("15551239999@s.whatsapp.net", 50)
	if err != nil {
		t.Fatalf("Messages: %v", err)
	}
	if msgs.JID != "15551239999@s.whatsapp.net" || len(msgs.Messages) != 0 {
		t.Errorf("Messages() = %+v, want an empty list for that jid", msgs)
	}
	chats, err := c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	if len(chats.Chats) != 0 {
		t.Errorf("Chats() over an empty store = %+v, want none (not the fixture)", chats.Chats)
	}
}

func jsonOf(t *testing.T, v any) string {
	t.Helper()
	data, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(data)
}

// subscriber dials sock, handshakes and subscribes, and returns a reader of the frames that
// follow.
func subscriber(t *testing.T, sock string) (net.Conn, *bufio.Reader) {
	t.Helper()
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("dial %s: %v", sock, err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	br := bufio.NewReader(conn)
	for _, step := range []struct{ send, reply string }{
		{`{"v":1,"op":"hello","client":"test/0.1.0"}`, `"op":"hello_ok"`},
		{`{"v":1,"op":"subscribe"}`, `{"v":1,"op":"subscribed"}`},
	} {
		if _, err := io.WriteString(conn, step.send+"\n"); err != nil {
			t.Fatalf("write %s: %v", step.send, err)
		}
		if got := readFrame(t, conn, br, 5*time.Second); !strings.Contains(got, step.reply) {
			t.Fatalf("reply to %s = %s, want %s", step.send, got, step.reply)
		}
	}
	return conn, br
}

// readFrame reads one frame within wait; it returns "" when none arrives in time.
func readFrame(t *testing.T, conn net.Conn, br *bufio.Reader, wait time.Duration) string {
	t.Helper()
	if err := conn.SetReadDeadline(time.Now().Add(wait)); err != nil {
		t.Fatalf("set deadline: %v", err)
	}
	line, err := br.ReadString('\n')
	if errors.Is(err, os.ErrDeadlineExceeded) {
		return ""
	}
	if err != nil {
		t.Fatalf("read frame: %v", err)
	}
	return strings.TrimSuffix(line, "\n")
}

func TestTwoSubscribedClientsBothReceiveAStoredMessage(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeWAClient{}
	session := wa.NewSession(fake, db, nil)
	events := ipc.NewBroker()
	session.SetEvents(events)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	sock := serveSocket(t, d, events)
	first, firstR := subscriber(t, sock)
	second, secondR := subscriber(t, sock)

	ada := types.NewJID("15551230001", types.DefaultUserServer)
	msg := textMessage(ada, "3EB0AAA3", false, time.Date(2026, 10, 18, 17, 58, 0, 0, time.UTC), "Running late")
	fake.emit(msg)
	want := []string{
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"3EB0AAA3","kind":"text","text":"Running late","from_me":false,"timestamp":"2026-10-18T17:58:00Z"}}`,
		`{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":1}`,
	}
	for i, r := range []struct {
		conn net.Conn
		br   *bufio.Reader
	}{{first, firstR}, {second, secondR}} {
		for _, w := range want {
			if got := readFrame(t, r.conn, r.br, 5*time.Second); got != w {
				t.Errorf("subscriber %d got %s, want %s", i+1, got, w)
			}
		}
	}

	// The same message again is a duplicate: stored once, published once.
	fake.emit(msg)
	if got := readFrame(t, first, firstR, 200*time.Millisecond); got != "" {
		t.Errorf("a duplicate message published %s, want nothing", got)
	}
}

func TestDaemonSendWhileUnlinkedAnswersNotLinked(t *testing.T) {
	session := wa.NewSession(nil, nil, nil)
	d, err := newDaemon(session, nil)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	c := serveInProcess(t, d)
	_, err = c.Send("15551230001@s.whatsapp.net", "On my way", "c-1")
	var perr *ipc.ProtocolError
	if !errors.As(err, &perr) || perr.Code != ipc.CodeNotLinked {
		t.Errorf("Send while unlinked = %v, want a %s error", err, ipc.CodeNotLinked)
	}
}

// linkedDaemon is a daemon over a started session with a fake linked client and a real store,
// served with events on a private socket.
func linkedDaemon(t *testing.T) (*wa.Session, *fakeWAClient, string) {
	t.Helper()
	db := openTestStore(t)
	fake := &fakeWAClient{sendID: "3EB0BBB1", sendTime: time.Date(2026, 10, 18, 17, 56, 10, 0, time.UTC)}
	session := wa.NewSession(fake, db, nil)
	t.Cleanup(session.Close)
	events := ipc.NewBroker()
	session.SetEvents(events)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	return session, fake, serveSocket(t, d, events)
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

func TestDaemonSendWhileLinkedAnswersSentStoresAndPublishes(t *testing.T) {
	_, fake, sock := linkedDaemon(t)
	ui, uiR := subscriber(t, sock)
	c := dial(t, sock)

	sent, err := c.Send("15551230001@s.whatsapp.net", "On my way", "c-7f3a9e21")
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	want := ipc.Sent{V: 1, Op: ipc.OpSent, ClientID: "c-7f3a9e21", State: ipc.SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	if sent != want {
		t.Errorf("Send = %+v, want %+v", sent, want)
	}
	if n := fake.sends.Load(); n != 1 {
		t.Errorf("client sends = %d, want 1", n)
	}
	msgs, err := c.Messages("15551230001@s.whatsapp.net", 10)
	if err != nil {
		t.Fatalf("Messages: %v", err)
	}
	wantMsg := ipc.Message{ID: "3EB0BBB1", Kind: ipc.KindText, Text: "On my way", FromMe: true, Timestamp: "2026-10-18T17:56:10Z"}
	if len(msgs.Messages) != 1 || msgs.Messages[0] != wantMsg {
		t.Errorf("Messages after a send = %+v, want only %+v", msgs.Messages, wantMsg)
	}
	wantEvent := `{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"3EB0BBB1","kind":"text","text":"On my way","from_me":true,"timestamp":"2026-10-18T17:56:10Z"}}`
	if got := readFrame(t, ui, uiR, 5*time.Second); got != wantEvent {
		t.Errorf("subscriber got %s, want %s", got, wantEvent)
	}
}

// Until the outbox (T11) an offline send answers send_failed, never sent/queued.
func TestDaemonSendWhileOfflineAnswersNotSent(t *testing.T) {
	_, fake, sock := linkedDaemon(t)
	fake.emit(&waevents.Disconnected{})
	c := dial(t, sock)

	_, err := c.Send("15551230001@s.whatsapp.net", "On my way", "c-1")
	var notSent *ipc.NotSentError
	if !errors.As(err, &notSent) {
		t.Fatalf("Send while offline = %v, want a send_failed reply", err)
	}
	if notSent.ClientID != "c-1" || !strings.Contains(notSent.Reason, "offline") {
		t.Errorf("send_failed = %+v, want client_id c-1 and an offline reason", notSent)
	}
	if n := fake.sends.Load(); n != 0 {
		t.Errorf("client sends = %d, want 0", n)
	}
	if msgs, err := c.Messages("15551230001@s.whatsapp.net", 10); err != nil || len(msgs.Messages) != 0 {
		t.Errorf("Messages after an offline send = %+v, %v; want none stored", msgs.Messages, err)
	}
}

func TestDaemonFocusReachesASubscribedUI(t *testing.T) {
	_, _, sock := linkedDaemon(t)
	ui, uiR := subscriber(t, sock)
	c := dial(t, sock)
	focused, err := c.Focus("15551230002@s.whatsapp.net")
	if err != nil {
		t.Fatalf("Focus: %v", err)
	}
	if focused.Delivered != 1 || focused.JID != "15551230002@s.whatsapp.net" {
		t.Errorf("Focus = %+v, want delivered 1", focused)
	}
	if got, want := readFrame(t, ui, uiR, 5*time.Second), `{"v":1,"op":"event_focus","jid":"15551230002@s.whatsapp.net"}`; got != want {
		t.Errorf("subscriber got %s, want %s", got, want)
	}
}

// fakeKeyring is an in-memory keyring; tests never touch the real one.
type fakeKeyring struct {
	mu    sync.Mutex
	items map[string]string
}

func (k *fakeKeyring) Set(service, user, secret string) error {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.items[service+"/"+user] = secret
	return nil
}

func (k *fakeKeyring) Get(service, user string) (string, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	secret, ok := k.items[service+"/"+user]
	if !ok {
		return "", store.ErrKeyNotFound
	}
	return secret, nil
}

func (k *fakeKeyring) Delete(service, user string) error {
	k.mu.Lock()
	defer k.mu.Unlock()
	if _, ok := k.items[service+"/"+user]; !ok {
		return store.ErrKeyNotFound
	}
	delete(k.items, service+"/"+user)
	return nil
}

func (k *fakeKeyring) count() int {
	k.mu.Lock()
	defer k.mu.Unlock()
	return len(k.items)
}

// deviceKeyItems are the keyring items the device store writes under the nuncio service.
var deviceKeyItems = []string{"device.noise_key", "device.identity_key", "device.signed_pre_key", "device.adv_secret_key"}

// wipeRig is a daemon over a started session with a fake linked client, a real store in a temp
// data dir that also holds media and exports, a fake keyring full of device items, served on
// $XDG_RUNTIME_DIR/nuncio/core.sock of a temp runtime dir.
type wipeRig struct {
	fake    *fakeWAClient
	keyring *fakeKeyring
	dataDir string
	runtime string
	socket  string
	// served is closed when serve returns.
	served chan struct{}
}

// shortTempDir is a temp dir with a short path: a unix socket path must fit in 104 bytes on macOS,
// and t.TempDir carries the long test name.
func shortTempDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "nw")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}

// newWipeRig builds the rig; configure, when not nil, sets up the rig's fake client before anything
// serves, so the daemon's goroutines only read it.
func newWipeRig(t *testing.T, configure func(*wipeRig)) *wipeRig {
	t.Helper()
	root := t.TempDir()
	r := &wipeRig{
		fake:    &fakeWAClient{sendID: "3EB0BBB1"},
		keyring: &fakeKeyring{items: map[string]string{}},
		dataDir: filepath.Join(root, "data", "nuncio"),
		runtime: shortTempDir(t),
		served:  make(chan struct{}),
	}
	r.socket = socketPath(r.runtime)
	db, err := store.Open(filepath.Join(r.dataDir, store.DBFile))
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	for _, p := range []string{filepath.Join(r.dataDir, store.MediaDir, "0f3a.jpg"), filepath.Join(r.dataDir, store.ExportsDir, "chat.txt")} {
		if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("synthetic"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	for _, name := range deviceKeyItems {
		_ = r.keyring.Set(store.KeyringService, name, "synthetic")
	}
	if configure != nil {
		configure(r)
	}
	session := wa.NewSession(r.fake, db, db.Close)
	t.Cleanup(session.Close)
	events := ipc.NewBroker()
	session.SetEvents(events)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	d.dataDir, d.keyring, d.socket = r.dataDir, r.keyring, r.socket
	if err := os.MkdirAll(filepath.Dir(r.socket), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", r.socket)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	// Closing the listener must not unlink the socket: only the daemon's own removal may, so a
	// test can tell whether it happened.
	ln.(*net.UnixListener).SetUnlinkOnClose(false)
	ctx, cancel := context.WithCancel(context.Background())
	go func() { _ = serve(ctx, ln, d, events); close(r.served) }()
	t.Cleanup(func() { cancel(); <-r.served })
	return r
}

// assertNothingLeft checks the S12 outcome: no data dir (so no DB, WAL, SHM, media or exports),
// an empty keyring, no socket, and a daemon that stopped serving.
func (r *wipeRig) assertNothingLeft(t *testing.T) {
	t.Helper()
	if _, err := os.Lstat(r.dataDir); !errors.Is(err, fs.ErrNotExist) {
		entries, _ := os.ReadDir(r.dataDir)
		t.Errorf("data dir %s still exists (%v), holding %v", r.dataDir, err, entries)
	}
	if n := r.keyring.count(); n != 0 {
		t.Errorf("keyring holds %d items, want 0", n)
	}
	select {
	case <-r.served:
	case <-time.After(5 * time.Second):
		t.Error("the daemon kept serving after the wipe")
	}
	// Checked after the listener closed: it does not unlink the socket (see newWipeRig), so only
	// the daemon's removal makes it go.
	if _, err := os.Lstat(r.socket); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("socket %s still exists: %v", r.socket, err)
	}
}

// assertNothingDeleted checks that a refused wipe left the data, the keys and the socket alone.
func (r *wipeRig) assertNothingDeleted(t *testing.T) {
	t.Helper()
	for _, p := range []string{filepath.Join(r.dataDir, store.DBFile), filepath.Join(r.dataDir, store.MediaDir, "0f3a.jpg"), filepath.Join(r.dataDir, store.ExportsDir, "chat.txt"), r.socket} {
		if _, err := os.Lstat(p); err != nil {
			t.Errorf("%s is gone after a refused wipe: %v", p, err)
		}
	}
	if n := r.keyring.count(); n != len(deviceKeyItems) {
		t.Errorf("keyring holds %d items after a refused wipe, want %d", n, len(deviceKeyItems))
	}
}

// TestLogoutUnlinksFirstThenLeavesNothingBehind drives the real CLI binary against the daemon:
// `nuncio logout --confirm` makes the daemon call the client's Logout exactly once, while the
// data is still there, and then leaves nothing behind.
func TestLogoutUnlinksFirstThenLeavesNothingBehind(t *testing.T) {
	// Set in the daemon's connection goroutine, read here after the CLI exits.
	var dbAtLogout, keysAtLogout atomic.Bool
	r := newWipeRig(t, func(r *wipeRig) {
		r.fake.onLogout = func() {
			_, err := os.Stat(filepath.Join(r.dataDir, store.DBFile))
			dbAtLogout.Store(err == nil)
			keysAtLogout.Store(r.keyring.count() == len(deviceKeyItems))
		}
	})
	cmd := exec.Command(nuncioBin, "logout", "--confirm")
	cmd.Env = []string{
		"PATH=" + os.Getenv("PATH"),
		"XDG_RUNTIME_DIR=" + r.runtime,
		"XDG_DATA_HOME=" + filepath.Dir(r.dataDir),
		"DBUS_SESSION_BUS_ADDRESS=" + noBus(t),
	}
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("nuncio logout --confirm: %v\n%s", err, out)
	}
	if !strings.Contains(string(out), "logged out") {
		t.Errorf("nuncio logout printed %q, want it to say logged out", out)
	}
	if n := r.fake.logouts.Load(); n != 1 {
		t.Errorf("client Logout called %d times, want 1", n)
	}
	if !dbAtLogout.Load() || !keysAtLogout.Load() {
		t.Errorf("at Logout the database existed=%t and the keys were all there=%t; the network step must come before any deletion", dbAtLogout.Load(), keysAtLogout.Load())
	}
	r.assertNothingLeft(t)
}

func TestPurgeThroughTheDaemonNeverCallsTheNetworkAndLeavesNothingBehind(t *testing.T) {
	r := newWipeRig(t, nil)
	c := dial(t, r.socket)
	got, err := c.Wipe(false)
	if err != nil {
		t.Fatalf("Wipe(false): %v", err)
	}
	if got.Unlinked {
		t.Errorf("Wipe(false) = %+v, want unlinked false", got)
	}
	if n := r.fake.logouts.Load(); n != 0 {
		t.Errorf("client Logout called %d times by a purge, want 0", n)
	}
	r.assertNothingLeft(t)
}

func TestLogoutThatWhatsAppDoesNotConfirmDeletesNothing(t *testing.T) {
	r := newWipeRig(t, func(r *wipeRig) { r.fake.logoutErr = errors.New("error sending logout request: timed out") })
	c := dial(t, r.socket)
	_, err := c.Wipe(true)
	var perr *ipc.ProtocolError
	if !errors.As(err, &perr) || perr.Code != ipc.CodeInternal || !strings.Contains(perr.Message, "nothing was deleted") {
		t.Errorf("Wipe(true) with a refused logout = %v, want an internal error saying nothing was deleted", err)
	}
	r.assertNothingDeleted(t)
	if _, err := c.Status(); err != nil {
		t.Errorf("the daemon stopped serving after a refused logout: %v", err)
	}
}

func TestLogoutWhileUnlinkedAnswersNotLinkedAndDeletesNothing(t *testing.T) {
	r := newWipeRig(t, nil)
	// The device is gone (an external logout): the session is unlinked, the local data is still
	// there and only purge may delete it.
	r.fake.emit(&waevents.LoggedOut{})
	c := dial(t, r.socket)
	_, err := c.Wipe(true)
	var perr *ipc.ProtocolError
	if !errors.As(err, &perr) || perr.Code != ipc.CodeNotLinked {
		t.Errorf("Wipe(true) while unlinked = %v, want a %s error", err, ipc.CodeNotLinked)
	}
	if n := r.fake.logouts.Load(); n != 0 {
		t.Errorf("client Logout called %d times while unlinked, want 0", n)
	}
	r.assertNothingDeleted(t)
}

// A wipe that fails after the session was closed must not leave the daemon claiming a live,
// linked session or serving the closed store; a retry once the cause is gone finishes the job.
func TestFailedLocalWipeDoesNotReportLinkedAndARetryFinishes(t *testing.T) {
	r := newWipeRig(t, nil)
	if err := os.Chmod(r.dataDir, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(r.dataDir, 0o700) })
	c := dial(t, r.socket)
	_, err := c.Wipe(false)
	var perr *ipc.ProtocolError
	if !errors.As(err, &perr) || !strings.Contains(perr.Message, "nuncio purge --confirm") {
		t.Fatalf("Wipe(false) into a read-only data dir = %v, want an error with the purge hint", err)
	}
	st, err := c.Status()
	if err != nil {
		t.Fatalf("Status after a failed wipe: %v", err)
	}
	if st.Session == ipc.SessionLinked {
		t.Errorf("status after a failed wipe = %+v; the session is closed", st)
	}
	_, err = c.Messages("15551230001@s.whatsapp.net", 10)
	if !errors.As(err, &perr) || perr.Code != ipc.CodeInternal || !strings.Contains(perr.Message, "nuncio purge --confirm") {
		t.Errorf("Messages after a failed wipe = %v, want an internal error with the purge hint", err)
	}

	if err := os.Chmod(r.dataDir, 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Wipe(false); err != nil {
		t.Fatalf("retried Wipe(false): %v", err)
	}
	r.assertNothingLeft(t)
}

// fakeDownloader stands in for whatsmeow's Client.Download (network fetch plus decryption with
// the media key, both delegated to whatsmeow), so it returns the known decrypted bytes and
// counts its calls. It never touches the network.
type fakeDownloader struct {
	plain []byte
	calls atomic.Int64
}

func (f *fakeDownloader) Download(context.Context, whatsmeow.DownloadableMessage) ([]byte, error) {
	f.calls.Add(1)
	return f.plain, nil
}

var photo = []byte("fake decrypted photo bytes")

func sha(b []byte) []byte { s := sha256.Sum256(b); return s[:] }

func imageEvent(chat types.JID, id string, ts time.Time, fileSHA []byte) *waevents.Message {
	return &waevents.Message{
		Info: types.MessageInfo{MessageSource: types.MessageSource{Chat: chat, Sender: chat}, ID: id, Timestamp: ts},
		Message: &waE2E.Message{ImageMessage: &waE2E.ImageMessage{
			Caption: proto.String("the venue"), Mimetype: proto.String("image/jpeg"),
			FileLength: proto.Uint64(48213), DirectPath: proto.String("/v/t62.fake/0001"),
			MediaKey: bytes.Repeat([]byte{7}, 32), FileSHA256: fileSHA, FileEncSHA256: bytes.Repeat([]byte{9}, 32),
		}},
	}
}

// mediaDaemon is linkedDaemon with a media cache in a temp dir served by a fake downloader.
func mediaDaemon(t *testing.T) (*fakeWAClient, *fakeDownloader, string, string) {
	t.Helper()
	session, fake, sock := linkedDaemon(t)
	dl := &fakeDownloader{plain: photo}
	dir := filepath.Join(t.TempDir(), "media")
	session.SetMedia(dir, dl)
	return fake, dl, dir, sock
}

func permOf(t *testing.T, p string) os.FileMode {
	t.Helper()
	fi, err := os.Stat(p)
	if err != nil {
		t.Fatal(err)
	}
	return fi.Mode().Perm()
}

func TestDaemonStoresMediaWithoutFetchingAndDownloadsItOnRequestOnce(t *testing.T) {
	fake, dl, dir, sock := mediaDaemon(t)
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	fake.emit(imageEvent(ada, "3EB0AAA0", time.Date(2026, 10, 18, 17, 50, 30, 0, time.UTC), sha(photo)))
	c := dial(t, sock)

	// messages and chats describe the attachment; nothing has been fetched.
	want := ipc.Message{ID: "3EB0AAA0", Kind: ipc.KindImage, Text: "the venue", Timestamp: "2026-10-18T17:50:30Z",
		Media: &ipc.Media{Mime: "image/jpeg", Size: 48213}}
	msgs, err := c.Messages(ada.String(), 10)
	if err != nil || len(msgs.Messages) != 1 || !reflect.DeepEqual(msgs.Messages[0], want) {
		t.Fatalf("Messages = %s, %v; want only %s", jsonOf(t, msgs.Messages), err, jsonOf(t, want))
	}
	chats, err := c.Chats(40)
	if err != nil || len(chats.Chats) != 1 || !reflect.DeepEqual(chats.Chats[0].LastMessage, &want) {
		t.Fatalf("Chats = %s, %v; want the image as last_message", jsonOf(t, chats.Chats), err)
	}
	if n := dl.calls.Load(); n != 0 {
		t.Fatalf("downloader called %d times before any request, want 0", n)
	}
	if _, err := os.Stat(dir); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("the media cache exists before any request: %v", err)
	}

	ready, err := c.DownloadMedia(ada.String(), "3EB0AAA0")
	if err != nil {
		t.Fatalf("DownloadMedia: %v", err)
	}
	if ready.Op != ipc.OpMediaReady || ready.JID != ada.String() || ready.ID != "3EB0AAA0" ||
		ready.Mime != "image/jpeg" || filepath.Dir(ready.Path) != dir {
		t.Errorf("DownloadMedia = %+v, want media_ready with a file in %s", ready, dir)
	}
	if got, err := os.ReadFile(ready.Path); err != nil || !bytes.Equal(got, photo) {
		t.Errorf("cached bytes = %q, %v; want %q", got, err, photo)
	}
	if m := permOf(t, dir); m != 0o700 {
		t.Errorf("cache dir mode %o, want 700", m)
	}
	if m := permOf(t, ready.Path); m != 0o600 {
		t.Errorf("cached file mode %o, want 600", m)
	}

	again, err := c.DownloadMedia(ada.String(), "3EB0AAA0")
	if err != nil || again != ready {
		t.Errorf("second DownloadMedia = %+v, %v; want %+v", again, err, ready)
	}
	if n := dl.calls.Load(); n != 1 {
		t.Errorf("downloader called %d times for two requests, want 1 (the second is a cache hit)", n)
	}
}

func TestDaemonDownloadMediaRejectsASHA256MismatchAndCachesNothing(t *testing.T) {
	fake, dl, dir, sock := mediaDaemon(t)
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	fake.emit(imageEvent(ada, "3EB0AAA0", time.Date(2026, 10, 18, 17, 50, 30, 0, time.UTC), sha([]byte("other"))))
	c := dial(t, sock)

	_, err := c.DownloadMedia(ada.String(), "3EB0AAA0")
	var perr *ipc.ProtocolError
	if !errors.As(err, &perr) || perr.Code != ipc.CodeInternal || perr.Message != wa.ErrMediaCorrupt.Error() {
		t.Fatalf("DownloadMedia of a mismatch = %v, want an internal error saying %q", err, wa.ErrMediaCorrupt)
	}
	des, err := os.ReadDir(dir)
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		t.Fatal(err)
	}
	if len(des) != 0 {
		t.Errorf("cache dir holds %d entries after a mismatch, want none", len(des))
	}
	if n := dl.calls.Load(); n != 1 {
		t.Errorf("downloader called %d times, want 1", n)
	}
}

func TestDaemonDownloadMediaCorners(t *testing.T) {
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	codeOf := func(err error) (string, string) {
		var perr *ipc.ProtocolError
		if !errors.As(err, &perr) {
			return "", fmt.Sprint(err)
		}
		return perr.Code, perr.Message
	}
	t.Run("unknown id, a text message and a bad jid", func(t *testing.T) {
		fake, dl, _, sock := mediaDaemon(t)
		fake.emit(textMessage(ada, "TXT1", false, time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC), "plain"))
		c := dial(t, sock)
		for _, id := range []string{"NOPE", "TXT1"} {
			if code, msg := codeOf(downloadErr(c, ada.String(), id)); code != ipc.CodeInternal || msg != wa.ErrNoMedia.Error() {
				t.Errorf("DownloadMedia(%s) = %s %q, want internal %q", id, code, msg, wa.ErrNoMedia)
			}
		}
		if code, _ := codeOf(downloadErr(c, "status@broadcast", "TXT1")); code != ipc.CodeBadFrame {
			t.Errorf("DownloadMedia with a bad jid = %s, want %s", code, ipc.CodeBadFrame)
		}
		if n := dl.calls.Load(); n != 0 {
			t.Errorf("downloader called %d times, want 0", n)
		}
	})
	t.Run("unlinked", func(t *testing.T) {
		d, err := newDaemon(wa.NewSession(nil, nil, nil), nil)
		if err != nil {
			t.Fatalf("newDaemon: %v", err)
		}
		c := serveInProcess(t, d)
		if code, _ := codeOf(downloadErr(c, ada.String(), "3EB0AAA0")); code != ipc.CodeNotLinked {
			t.Errorf("DownloadMedia while unlinked = %s, want %s", code, ipc.CodeNotLinked)
		}
	})
	t.Run("offline", func(t *testing.T) {
		fake, dl, _, sock := mediaDaemon(t)
		fake.emit(imageEvent(ada, "3EB0AAA0", time.Date(2026, 10, 18, 17, 50, 30, 0, time.UTC), sha(photo)))
		fake.emit(&waevents.Disconnected{})
		c := dial(t, sock)
		if code, msg := codeOf(downloadErr(c, ada.String(), "3EB0AAA0")); code != ipc.CodeInternal || msg != wa.ErrMediaOffline.Error() {
			t.Errorf("DownloadMedia while offline = %s %q, want internal %q", code, msg, wa.ErrMediaOffline)
		}
		if n := dl.calls.Load(); n != 0 {
			t.Errorf("downloader called %d times while offline, want 0", n)
		}
	})
}

func downloadErr(c *ipc.Client, jid, id string) error {
	_, err := c.DownloadMedia(jid, id)
	return err
}
