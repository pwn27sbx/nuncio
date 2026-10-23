package main

import (
	"os/exec"
	"slices"
	"syscall"
	"testing"
	"time"
)

// A process that has exited but nobody reaped (a zombie) counts as gone, and a running one does
// not: in a container whose PID 1 never reaps, a killed bus and its activated daemon stay
// zombies, and kill(pid, 0) still succeeds on them.
func TestAZombieCountsAsGoneAndARunningProcessDoesNot(t *testing.T) {
	running := exec.Command("/bin/sleep", "30")
	running.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := running.Start(); err != nil {
		t.Fatalf("start sleep: %v", err)
	}
	t.Cleanup(func() { _ = running.Process.Kill(); _ = running.Wait() })

	zombie := exec.Command("/bin/sh", "-c", "exit 0")
	zombie.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := zombie.Start(); err != nil {
		t.Fatalf("start sh: %v", err)
	}
	// Reaped only at the end: until then it is this test's zombie.
	t.Cleanup(func() { _ = zombie.Wait() })

	pid := zombie.Process.Pid
	deadline := time.Now().Add(5 * time.Second)
	for processRunning(t, pid) {
		if time.Now().After(deadline) {
			t.Fatalf("pid %d still counts as running 5s after it exited", pid)
		}
		time.Sleep(10 * time.Millisecond)
	}
	if err := syscall.Kill(pid, 0); err != nil {
		t.Fatalf("kill(%d, 0) = %v, want nil: the unreaped child should be a zombie", pid, err)
	}
	if members := runningGroupMembers(t, pid); len(members) != 0 {
		t.Errorf("group %d of a zombie has running members %v, want none", pid, members)
	}

	if !processRunning(t, running.Process.Pid) {
		t.Errorf("running sleep %d counts as gone", running.Process.Pid)
	}
	if members := runningGroupMembers(t, running.Process.Pid); !slices.Equal(members, []int{running.Process.Pid}) {
		t.Errorf("group of the running sleep = %v, want [%d]", members, running.Process.Pid)
	}
}
