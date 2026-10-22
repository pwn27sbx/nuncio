package main

import (
	"os/exec"
	"strconv"
	"strings"
	"testing"
)

// processRunning reports whether pid exists and is not a zombie (ps state Z).
func processRunning(t *testing.T, pid int) bool {
	t.Helper()
	for _, p := range psTable(t) {
		if p.pid == pid {
			return p.state != 'Z'
		}
	}
	return false
}

// runningGroupMembers answers the PIDs in process group pgid that are not zombies.
func runningGroupMembers(t *testing.T, pgid int) []int {
	t.Helper()
	var members []int
	for _, p := range psTable(t) {
		if p.pgid == pgid && p.state != 'Z' {
			members = append(members, p.pid)
		}
	}
	return members
}

type psRow struct {
	pid, pgid int
	state     byte
}

// psTable lists every process with its group and state; macOS has no /proc.
func psTable(t *testing.T) []psRow {
	t.Helper()
	out, err := exec.Command("/bin/ps", "-A", "-o", "pid=,pgid=,stat=").Output()
	if err != nil {
		t.Fatalf("ps: %v", err)
	}
	var rows []psRow
	for _, line := range strings.Split(string(out), "\n") {
		f := strings.Fields(line)
		if len(f) < 3 {
			continue
		}
		pid, err1 := strconv.Atoi(f[0])
		pgid, err2 := strconv.Atoi(f[1])
		if err1 != nil || err2 != nil {
			t.Fatalf("unexpected ps line %q", line)
		}
		rows = append(rows, psRow{pid: pid, pgid: pgid, state: f[2][0]})
	}
	return rows
}
