package main

import (
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// senderSession is fixedSession whose Senders names two fake people and counts how many resolvers
// the daemon asked for.
type senderSession struct {
	fixedSession
	asked *int
}

func (s senderSession) Senders() func(string) string {
	*s.asked++
	return func(sender string) string {
		return map[string]string{"15551230001@s.whatsapp.net": "Ana", "100000000000002@lid": "…0002"}[sender]
	}
}

// messages and search serve each incoming group message with its sender, as the session names it,
// with one resolver per reply; a direct chat's messages and mine carry none.
func TestDaemonServesGroupMessagesWithTheirSender(t *testing.T) {
	const group, direct = "120363000000000001@g.us", "15551230001@s.whatsapp.net"
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	for _, m := range []store.Message{
		{ID: "G1", ChatJID: group, Kind: store.KindText, Text: "rope at the crag", Timestamp: t0, Sender: "15551230001@s.whatsapp.net"},
		{ID: "G2", ChatJID: group, Kind: store.KindText, Text: "rope is here", Timestamp: t0.Add(time.Second), Sender: "100000000000002@lid"},
		{ID: "G3", ChatJID: group, Kind: store.KindText, Text: "my rope", Timestamp: t0.Add(2 * time.Second), FromMe: true},
		{ID: "D1", ChatJID: direct, Kind: store.KindText, Text: "rope?", Timestamp: t0, Sender: direct},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	asked := 0
	d, err := newDaemon(senderSession{fixedSession(wa.StateLinked), &asked}, db)
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]string{"G1": "Ana", "G2": "…0002", "G3": "", "D1": ""}
	serve := map[string]func(string) ([]ipc.Message, error){
		"messages": func(jid string) ([]ipc.Message, error) { return d.Messages(jid, 10) },
		"search":   func(jid string) ([]ipc.Message, error) { return d.Search(jid, "rope", 10) },
	}
	for op, f := range serve {
		before := asked
		for _, jid := range []string{group, direct} {
			msgs, err := f(jid)
			if err != nil {
				t.Fatal(err)
			}
			for _, m := range msgs {
				if m.Sender != want[m.ID] {
					t.Errorf("%s: %s sender = %q, want %q", op, m.ID, m.Sender, want[m.ID])
				}
			}
		}
		if asked-before != 2 {
			t.Errorf("%s: %d resolvers for 2 replies, want one per reply", op, asked-before)
		}
	}
}

// The real session names senders for the daemon.
var _ interface{ Senders() func(string) string } = (*wa.Session)(nil)
