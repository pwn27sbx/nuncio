package main

import "syscall"

// peerPIDOf answers the PID of the process on the other end of the Unix socket fd: macOS's
// LOCAL_PEERPID option at level SOL_LOCAL (both from <sys/un.h>).
func peerPIDOf(fd uintptr) (int, error) {
	const solLocal, localPeerPID = 0, 2
	return syscall.GetsockoptInt(int(fd), solLocal, localPeerPID)
}
