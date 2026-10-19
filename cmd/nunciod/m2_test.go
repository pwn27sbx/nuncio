package main

import (
	"errors"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// The daemon serves the M2 ops: ServeConn finds them through these interfaces.
var (
	_ ipc.Searcher = (*daemon)(nil)
	_ ipc.Replier  = (*daemon)(nil)
	_ ipc.Reactor  = (*daemon)(nil)
)

func TestDaemonSearchesOneChatOfTheStoreNewestFirst(t *testing.T) {
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	for _, m := range []store.Message{
		{ID: "A1", ChatJID: "15551230001@s.whatsapp.net", Kind: store.KindText, Text: "the venue is open", Timestamp: t0},
		{ID: "A2", ChatJID: "15551230001@s.whatsapp.net", Kind: store.KindText, Text: "venue?", Timestamp: t0.Add(time.Minute), ReplyTo: "A1"},
		{ID: "G1", ChatJID: "15551230002@s.whatsapp.net", Kind: store.KindText, Text: "venue", Timestamp: t0},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	d, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	got, err := d.Search("15551230001@s.whatsapp.net", "venue", 50)
	if err != nil || len(got) != 2 || got[0].ID != "A2" || got[0].ReplyTo != "A1" || got[1].ID != "A1" {
		t.Errorf("Search = %+v, %v; want A2 (quoting A1) then A1", got, err)
	}
	none, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := none.Search("15551230001@s.whatsapp.net", "venue", 50); err != nil || len(got) != 0 {
		t.Errorf("Search without a store = %+v, %v; want nothing", got, err)
	}
}

func TestDaemonReactPassesTheSessionsAnswer(t *testing.T) {
	d, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := d.React("15551230001@s.whatsapp.net", "A1", "x"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("React while unlinked = %v, want ipc.ErrNotLinked", err)
	}
}
