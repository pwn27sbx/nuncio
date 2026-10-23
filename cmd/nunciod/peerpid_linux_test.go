package main

import "syscall"

// peerPIDOf answers the PID of the process on the other end of the Unix socket fd.
func peerPIDOf(fd uintptr) (int, error) {
	cred, err := syscall.GetsockoptUcred(int(fd), syscall.SOL_SOCKET, syscall.SO_PEERCRED)
	if err != nil {
		return 0, err
	}
	return int(cred.Pid), nil
}
