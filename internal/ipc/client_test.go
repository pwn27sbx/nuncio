package ipc

import (
	"errors"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestDefaultSocketPathUsesXDGRuntimeDir(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", "/run/user/1234")
	got, err := DefaultSocketPath()
	if err != nil {
		t.Fatalf("DefaultSocketPath: %v", err)
	}
	const want = "/run/user/1234/nuncio/core.sock"
	if got != want {
		t.Fatalf("DefaultSocketPath() = %q, want %q", got, want)
	}
}

func TestDefaultSocketPathWithoutXDGRuntimeDir(t *testing.T) {
	t.Setenv("XDG_RUNTIME_DIR", "")
	if _, err := DefaultSocketPath(); err == nil {
		t.Fatal("DefaultSocketPath() with no XDG_RUNTIME_DIR: got nil error")
	}
}

func TestDialHandshakesAndCarriesRequests(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)

	c, err := Dial(sock, "nuncio/0.1.0")
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = c.Close() }()

	if got := h.clientNames(); len(got) != 1 || got[0] != "nuncio/0.1.0" {
		t.Fatalf("handler saw handshakes %q, want [nuncio/0.1.0]", got)
	}

	st, err := c.Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	want := StatusResponse{V: 1, Op: OpStatus, Daemon: "nunciod/0.1.0", Session: SessionUnlinked}
	if st != want {
		t.Fatalf("Status() = %+v, want %+v", st, want)
	}

	chats, err := c.Chats(40)
	if err != nil {
		t.Fatalf("Chats: %v", err)
	}
	if len(chats.Chats) != 3 {
		t.Fatalf("got %d chats, want 3", len(chats.Chats))
	}
	if chats.Chats[0].Name != "Ada Lovelace" || chats.Chats[2].JID != "120363000000000000@g.us" {
		t.Fatalf("chats = %+v, want the fixture list", chats.Chats)
	}
}

func TestDialReportsMissingSocket(t *testing.T) {
	if _, err := Dial(filepath.Join(t.TempDir(), "nope.sock"), "nuncio/0.1.0"); err == nil {
		t.Fatal("Dial on a missing socket: got nil error")
	}
}

// TestDialReportsVersionMismatch covers a client talking to a daemon that
// refuses the version: the handshake must fail loudly, not silently.
func TestDialReportsVersionMismatch(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "core.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer func() { _ = ln.Close() }()
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer func() { _ = conn.Close() }()
		_ = WriteFrame(conn, Error{V: 1, Op: OpError, Code: CodeVersionMismatch, Message: "unsupported protocol version"})
	}()

	if _, err := Dial(sock, "nuncio/0.1.0"); err == nil {
		t.Fatal("Dial against a version-refusing daemon: got nil error")
	}
}

// startSilentServer accepts connections and answers the hello, if answerHello is set, and then
// never answers anything again: a wedged daemon.
func startSilentServer(t *testing.T, answerHello bool) string {
	t.Helper()
	sock := filepath.Join(t.TempDir(), "core.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			t.Cleanup(func() { _ = conn.Close() })
			if answerHello {
				go func() {
					rd := NewReader(conn)
					if _, err := rd.Next(); err != nil {
						return
					}
					_ = WriteFrame(conn, HelloOK{V: Version, Op: OpHelloOK, Daemon: "nunciod/0.1.0"})
				}()
			}
		}
	}()
	return sock
}

const testReadTimeout = 100 * time.Millisecond

// returnsWithin runs call and returns its error, failing the test when call has not returned after
// bound. A client that never times out then fails the test, which runs its cleanups; reaching go
// test's -timeout instead panics and exits without them, leaving the test's temp dir and its
// core.sock behind in $TMPDIR.
func returnsWithin(t *testing.T, bound time.Duration, call func() error) error {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- call() }()
	select {
	case err := <-done:
		return err
	case <-time.After(bound):
		t.Fatalf("still blocked after %v: the client read is not bounded", bound)
		return nil
	}
}

func TestStatusTimesOutOnAWedgedDaemon(t *testing.T) {
	sock := startSilentServer(t, true)
	c, err := DialTimeout(sock, "nuncio/0.1.0", testReadTimeout)
	if err != nil {
		t.Fatalf("DialTimeout: %v", err)
	}
	defer func() { _ = c.Close() }()

	started := time.Now()
	err = returnsWithin(t, 10*testReadTimeout, func() error { _, err := c.Status(); return err })
	if !errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("Status() error = %v, want a deadline-exceeded timeout", err)
	}
	if elapsed := time.Since(started); elapsed > 10*testReadTimeout {
		t.Fatalf("Status() took %v, want about %v", elapsed, testReadTimeout)
	}
}

func TestDialTimesOutWhenTheHandshakeNeverAnswers(t *testing.T) {
	sock := startSilentServer(t, false)
	started := time.Now()
	err := returnsWithin(t, 10*testReadTimeout, func() error {
		c, err := DialTimeout(sock, "nuncio/0.1.0", testReadTimeout)
		if err == nil {
			_ = c.Close()
		}
		return err
	})
	if !errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("DialTimeout() error = %v, want a deadline-exceeded timeout", err)
	}
	if elapsed := time.Since(started); elapsed > 10*testReadTimeout {
		t.Fatalf("DialTimeout() took %v, want about %v", elapsed, testReadTimeout)
	}
}

// TestShortTimeoutDoesNotBreakANormalExchange proves the bound only cuts a silent daemon: each
// read gets its own deadline, so a slow sequence of prompt replies is fine.
func TestShortTimeoutDoesNotBreakANormalExchange(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	c, err := DialTimeout(sock, "nuncio/0.1.0", testReadTimeout)
	if err != nil {
		t.Fatalf("DialTimeout: %v", err)
	}
	defer func() { _ = c.Close() }()
	for i := 0; i < 3; i++ {
		time.Sleep(testReadTimeout) // idle time between requests must not count against a read
		if _, err := c.Status(); err != nil {
			t.Fatalf("Status #%d: %v", i, err)
		}
	}
}
