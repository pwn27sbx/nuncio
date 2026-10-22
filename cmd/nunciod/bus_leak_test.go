package main

import (
	"errors"
	"net"
	"syscall"
	"testing"

	"github.com/godbus/dbus/v5"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// socketPeerPID answers the PID of the daemon serving socket, read from the socket itself, so it
// works where the bus cannot tell (macOS's dbus-daemon answers no GetConnectionUnixProcessID).
func socketPeerPID(t *testing.T, socket string) int {
	t.Helper()
	conn, err := net.Dial("unix", socket)
	if err != nil {
		t.Fatalf("dial %s: %v", socket, err)
	}
	defer func() { _ = conn.Close() }()
	raw, err := conn.(*net.UnixConn).SyscallConn()
	if err != nil {
		t.Fatalf("raw conn: %v", err)
	}
	var pid int
	var pidErr error
	if err := raw.Control(func(fd uintptr) { pid, pidErr = peerPIDOf(fd) }); err != nil {
		t.Fatalf("control: %v", err)
	}
	if pidErr != nil || pid <= 0 {
		t.Fatalf("peer PID of %s = %d, %v", socket, pid, pidErr)
	}
	return pid
}

// A daemon the private bus starts by activation dies with the bus when the test ends: the leak
// this guards left one orphaned nunciod per activation test per run.
func TestPrivateBusLeavesNoActivatedDaemonBehind(t *testing.T) {
	var pid int
	t.Run("activate", func(t *testing.T) {
		rt := shortTempDir(t)
		bus := startPrivateBus(t, rt)
		conn := busClient(t, bus.address)
		var st ipc.Status
		if err := conn.Object(ipc.DaemonName, dbus.ObjectPath(ipc.ObjectPath)).
			Call(ipc.Interface+".Status", 0).Store(&st); err != nil {
			t.Fatalf("activating %s.Status call: %v", ipc.DaemonName, err)
		}
		waitForDaemon(t, socketPath(rt))
		pid = socketPeerPID(t, socketPath(rt))
	})
	if pid == 0 {
		t.Fatal("the activation subtest learned no daemon PID")
	}
	if err := syscall.Kill(pid, 0); !errors.Is(err, syscall.ESRCH) {
		_ = syscall.Kill(pid, syscall.SIGKILL) // ours: started by this test's bus
		t.Fatalf("activated nunciod %d is still alive after its test ended (kill 0 = %v)", pid, err)
	}
}
