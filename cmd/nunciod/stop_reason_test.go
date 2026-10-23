package main

import (
	"testing"
	"time"

	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// Q6: after WhatsApp stops the session, status says why, as a fixed stop_reason, and a subscribed
// client got it with the event_connection of the stop.
func TestStatusReportsTheStopReasonAfterAStop(t *testing.T) {
	_, fake, sock := linkedDaemon(t)
	ui, uiR := subscriber(t, sock)
	c := dial(t, sock)

	before, err := c.Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	if before.StopReason != "" {
		t.Errorf("status before a stop has stop_reason %q, want none", before.StopReason)
	}

	fake.emit(&waevents.TemporaryBan{Code: waevents.TempBanBlockedByUsers})
	want := `{"v":1,"op":"event_connection","session":"offline","stop_reason":"temporary_ban"}`
	if got := readFrame(t, ui, uiR, 5*time.Second); got != want {
		t.Errorf("event after the ban = %s, want %s", got, want)
	}
	st, err := c.Status()
	if err != nil {
		t.Fatalf("Status: %v", err)
	}
	if st.Session != ipc.SessionOffline || st.StopReason != ipc.StopTemporaryBan {
		t.Errorf("status after the ban = %+v, want offline with stop_reason temporary_ban", st)
	}
}
