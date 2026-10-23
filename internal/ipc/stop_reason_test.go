package ipc

import (
	"bytes"
	"encoding/json"
	"testing"

	"github.com/godbus/dbus/v5"
)

// Q6: status names why WhatsApp stopped the session, as a fixed stop_reason.
func TestStatusCarriesTheStopReason(t *testing.T) {
	h := newFakeHandler(t)
	h.status = Status{Daemon: "nunciod/0.1.0", Linked: true, Session: SessionOffline, StopReason: StopTemporaryBan}
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)

	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	want := `{"v":1,"op":"status","daemon":"nunciod/0.1.0","linked":true,"session":"offline","unread":0,"stop_reason":"temporary_ban"}`
	if got := string(recvRaw(t, br)); got != want {
		t.Errorf("status = %s, want %s", got, want)
	}
}

// Without a stop, status and event_connection are the same bytes as before stop_reason existed.
func TestFramesWithoutAStopAreByteIdentical(t *testing.T) {
	h := newFakeHandler(t)
	sock := startServer(t, h)
	conn, br := rawConn(t, sock)
	handshake(t, conn, br)
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got, want := recvRaw(t, br), readFixture(t, "status_response.json"); !bytes.Equal(got, want) {
		t.Errorf("status without a stop = %s, want the fixture's bytes %s", got, want)
	}

	got, err := json.Marshal(EventConnection{V: Version, Op: OpEventConnection, Session: SessionOffline})
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if want := readFixture(t, "event_connection.json"); !bytes.Equal(got, want) {
		t.Errorf("event_connection without a stop = %s, want the fixture's bytes %s", got, want)
	}
}

// The D-Bus Status reply keeps its signature: stop_reason is only on the socket.
func TestDBusStatusSignatureIsUnchanged(t *testing.T) {
	if got := dbus.SignatureOf(Status{StopReason: StopLoggedOut}).String(); got != "(sbsi)" {
		t.Errorf("D-Bus signature of Status = %s, want (sbsi)", got)
	}
}
