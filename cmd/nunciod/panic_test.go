package main

import (
	"bufio"
	"encoding/json"
	"io"
	"net"
	"strings"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// panicOnMarkRead is the daemon with a bug in one op: building the mark_read reply panics with a
// value that quotes a number.
type panicOnMarkRead struct{ *daemon }

func (panicOnMarkRead) MarkRead(string) (int, error) {
	panic("cannot build the reply for 15551230001")
}

// A panic while one request is answered ends only that connection: the client reads the internal
// error, then the close; another connection still gets its status, and the log names the panic by
// its type only.
func TestAPanicInOneRequestClosesOnlyItsConnection(t *testing.T) {
	logs := daemonLog(t)
	d, err := newDaemon(fixedSession(wa.StateLinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	sock := serveSocket(t, panicOnMarkRead{d}, ipc.NewBroker())

	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	if err := conn.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatal(err)
	}
	br := bufio.NewReader(conn)
	if _, err := io.WriteString(conn, `{"v":1,"op":"hello","client":"test/0.1.0"}`+"\n"); err != nil {
		t.Fatal(err)
	}
	if _, err := br.ReadString('\n'); err != nil {
		t.Fatalf("hello_ok: %v", err)
	}
	if _, err := io.WriteString(conn, `{"v":1,"op":"mark_read","jid":"15551230001@s.whatsapp.net"}`+"\n"); err != nil {
		t.Fatal(err)
	}
	line, err := br.ReadString('\n')
	if err != nil {
		t.Fatalf("mark_read reply: %v", err)
	}
	var reply ipc.Error
	if err := json.Unmarshal([]byte(line), &reply); err != nil {
		t.Fatalf("reply %q: %v", line, err)
	}
	if reply.Op != ipc.OpError || reply.Code != ipc.CodeInternal || reply.Message != ipc.PanicMessage {
		t.Errorf("reply = %+v, want error/internal %q", reply, ipc.PanicMessage)
	}
	if rest, err := br.ReadString('\n'); err != io.EOF {
		t.Errorf("after the reply read %q, %v; want the connection closed", rest, err)
	}

	other, err := ipc.Dial(sock, "test/0.1.0")
	if err != nil {
		t.Fatalf("second dial: %v", err)
	}
	t.Cleanup(func() { _ = other.Close() })
	st, err := other.Status()
	if err != nil || st.Session != string(wa.StateLinked) {
		t.Errorf("second connection status = %+v, %v; want linked", st, err)
	}

	got := waitForLog(logs, "ipc: a request panicked (string); its connection was closed")
	assertNoLeak(t, got, "ipc: a request panicked (string); its connection was closed", "15551230001", "cannot build", "a connection ended")
	if n := strings.Count(got, "panicked"); n != 1 {
		t.Errorf("log = %q, want one panic line, got %d", got, n)
	}
}
