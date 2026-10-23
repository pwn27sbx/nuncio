package main

import (
	"errors"
	"io/fs"
	"os"
	"strconv"
	"strings"
	"testing"
)

// procStat answers the state and process group of pid from /proc/<pid>/stat; ok is false when
// the process does not exist.
func procStat(t *testing.T, pid int) (state byte, pgid int, ok bool) {
	t.Helper()
	data, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat")
	if errors.Is(err, fs.ErrNotExist) {
		return 0, 0, false
	}
	if err != nil {
		t.Fatalf("read /proc/%d/stat: %v", pid, err)
	}
	// "pid (comm) state ppid pgrp ...": comm may hold spaces and parentheses, so split after
	// the last ')'.
	i := strings.LastIndexByte(string(data), ')')
	fields := strings.Fields(string(data[i+1:]))
	if i < 0 || len(fields) < 3 {
		t.Fatalf("malformed /proc/%d/stat: %q", pid, data)
	}
	pgid, err = strconv.Atoi(fields[2])
	if err != nil {
		t.Fatalf("pgrp in /proc/%d/stat: %v", pid, err)
	}
	return fields[0][0], pgid, true
}

// processRunning reports whether pid exists and is not a zombie.
func processRunning(t *testing.T, pid int) bool {
	t.Helper()
	state, _, ok := procStat(t, pid)
	return ok && state != 'Z'
}

// runningGroupMembers answers the PIDs in process group pgid that are not zombies.
func runningGroupMembers(t *testing.T, pgid int) []int {
	t.Helper()
	entries, err := os.ReadDir("/proc")
	if err != nil {
		t.Fatalf("read /proc: %v", err)
	}
	var members []int
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil {
			continue
		}
		if state, group, ok := procStat(t, pid); ok && group == pgid && state != 'Z' {
			members = append(members, pid)
		}
	}
	return members
}
