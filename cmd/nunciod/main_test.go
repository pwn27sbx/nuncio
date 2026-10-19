package main

import (
	"bufio"
	"bytes"
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
	"syscall"
	"testing"
	"time"

	"github.com/godbus/dbus/v5"

	"github.com/pwn27sbx/nuncio/internal/ipc"
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
	if third.JID != "120363000000000000@g.us" || !third.Archived || third.Unread != 17 || third.LastMessage != nil {
		t.Errorf("group chat = %+v, want an archived group with 17 unread and no last message", third)
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

func TestStatusReportsTheSessionState(t *testing.T) {
	for _, tc := range []struct {
		state  wa.State
		linked bool
	}{
		{wa.StateUnlinked, false},
		{wa.StateLinked, true},
		{wa.StateOffline, true},
	} {
		d, err := newDaemon(fixedSession(tc.state))
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
