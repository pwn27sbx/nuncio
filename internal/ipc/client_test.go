package ipc

import (
	"net"
	"path/filepath"
	"testing"
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
