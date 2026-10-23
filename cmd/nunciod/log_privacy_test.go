package main

import (
	"bytes"
	"errors"
	"io"
	"log"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// daemonLog sends the standard logger to a fresh lockedBuffer until the test ends.
func daemonLog(t *testing.T) *lockedBuffer {
	t.Helper()
	buf := &lockedBuffer{}
	log.SetOutput(buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	return buf
}

// assertNoLeak checks that got holds want and none of leaks.
func assertNoLeak(t *testing.T, got, want string, leaks ...string) {
	t.Helper()
	if !strings.Contains(got, want) {
		t.Errorf("log = %q, want %q", got, want)
	}
	for _, leak := range leaks {
		if strings.Contains(got, leak) {
			t.Errorf("log = %q carries %q", got, leak)
		}
	}
}

// waitForLog waits until the log holds want, for a line logged off the test's goroutine.
func waitForLog(buf *lockedBuffer, want string) string {
	deadline := time.Now().Add(5 * time.Second)
	for !strings.Contains(buf.String(), want) && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	return buf.String()
}

// The client names itself in hello; that name is the client's own text and is not logged.
func TestHelloIsLoggedWithoutTheClientsName(t *testing.T) {
	logs := daemonLog(t)
	d, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	d.Hello("Ada Lovelace\x1b[2J")
	assertNoLeak(t, logs.String(), "a client said hello", "Ada", "\x1b")
}

// A store that cannot count the unread total nor list the chats is logged by its error's type.
func TestFailedStoreReadsOfTheDaemonAreLoggedByTheirTypeOnly(t *testing.T) {
	db := openTestStore(t)
	d, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	logs := daemonLog(t)
	if st := d.Status(); st.Unread != 0 {
		t.Errorf("Status over a closed store = %+v, want 0 unread", st)
	}
	if chats := d.Chats(); chats != nil {
		t.Errorf("Chats over a closed store = %+v, want none", chats)
	}
	got := logs.String()
	assertNoLeak(t, got, "cannot count the unread messages (", "database is closed")
	assertNoLeak(t, got, "cannot list the chats (", "database is closed")
}

// A chat whose last message cannot be read is logged by the error's type, not by SQLite's text.
func TestAChatWhoseLastMessageCannotBeReadIsLoggedByItsTypeOnly(t *testing.T) {
	db := openTestStore(t)
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	if _, err := store.InsertMessage(db, store.Message{ID: "3EB0LOGCHAT", ChatJID: ada.String(),
		Kind: store.KindText, Text: "the secret words of Ada", Timestamp: time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)}); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`DROP TABLE outbox_replies`); err != nil {
		t.Fatal(err)
	}
	d, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	logs := daemonLog(t)
	if chats := d.Chats(); chats != nil {
		t.Errorf("Chats = %+v, want none when a chat cannot be read", chats)
	}
	assertNoLeak(t, logs.String(), "cannot read the last message of a chat (",
		"no such table", "outbox_replies", "15551230001", "3EB0LOGCHAT", "secret words")
}

// A connection refused for a malformed first frame is logged without the refusal's text, which
// quotes what the client sent.
func TestARefusedConnectionIsLoggedByItsTypeOnly(t *testing.T) {
	d, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	logs := daemonLog(t)
	sock := serveSocket(t, d, nil)
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = conn.Close() }()
	if _, err := conn.Write([]byte("Ada Lovelace\n")); err != nil {
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	_, _ = io.ReadAll(conn)
	want := "a connection ended with an error ("
	assertNoLeak(t, waitForLog(logs, want), want, "malformed hello frame", "invalid character", "'A'", "Ada")
}

// A wipe that cannot delete the data dir is logged without the paths the error names.
func TestAFailedWipeIsLoggedWithoutItsPaths(t *testing.T) {
	r := newWipeRig(t, nil)
	if err := os.Chmod(r.dataDir, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(r.dataDir, 0o700) })
	logs := daemonLog(t)
	c := dial(t, r.socket)
	if _, err := c.Wipe(false); err == nil {
		t.Fatal("Wipe(false) into a read-only data dir = nil, want an error")
	}
	assertNoLeak(t, logs.String(), "cannot delete all of the local data (",
		r.dataDir, "permission denied")
}

// A daemon that cannot start logs why by the error's type only: the error names the runtime dir,
// and the missing session bus's error names its address.
func TestADaemonThatCannotStartLogsNoPath(t *testing.T) {
	runtimeDir := shortTempDir(t)
	// A file where the socket's directory goes: creating that directory fails.
	if err := os.WriteFile(filepath.Join(runtimeDir, "nuncio"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	bus := noBus(t)
	cmd := exec.Command(nunciodBin)
	cmd.Env = daemonEnv(runtimeDir, bus)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	cmd.Stdout = io.Discard
	err := cmd.Run()
	var exit *exec.ExitError
	if !errors.As(err, &exit) || exit.ExitCode() != 1 {
		t.Fatalf("nunciod = %v, want exit status 1", err)
	}
	got := stderr.String()
	assertNoLeak(t, got, "nunciod: cannot start (", runtimeDir, "not a directory")
	assertNoLeak(t, got, "nunciod: no session bus (", strings.TrimPrefix(bus, "unix:path="), "no such file")
}
