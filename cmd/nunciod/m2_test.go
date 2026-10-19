package main

import (
	"errors"
	"io"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// The daemon serves the M2 ops: ServeConn finds them through these interfaces.
var (
	_ ipc.Searcher = (*daemon)(nil)
	_ ipc.Replier  = (*daemon)(nil)
	_ ipc.Reactor  = (*daemon)(nil)
	// mark_read is served too (S8).
	_ ipc.MarkReader = (*daemon)(nil)
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

func TestDaemonMarkReadPassesTheSessionsAnswerAndRefusesAfterAWipe(t *testing.T) {
	d, err := newDaemon(fixedSession(wa.StateUnlinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := d.MarkRead("15551230001@s.whatsapp.net"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("MarkRead while unlinked = %v, want ipc.ErrNotLinked", err)
	}
	linked, err := newDaemon(fixedSession(wa.StateLinked), nil)
	if err != nil {
		t.Fatal(err)
	}
	if n, err := linked.MarkRead("15551230001@s.whatsapp.net"); err != nil || n != 7 {
		t.Errorf("MarkRead = %d, %v; want the session's total 7", n, err)
	}
	linked.closed.Store(true)
	if _, err := linked.MarkRead("15551230001@s.whatsapp.net"); !errors.Is(err, errClosed) {
		t.Errorf("MarkRead after a wipe closed the store = %v, want errClosed", err)
	}
}

// Opening a chat in a UI (mark_read over the socket) sends its read receipt, clears the chat in
// the store, publishes event_unread to subscribers and answers marked_read with the new total; a
// second mark_read of the same chat sends nothing.
func TestMarkReadOverTheSocketSendsTheReceiptAndPublishesTheUnread(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeWAClient{}
	session := wa.NewSession(fake, db, nil)
	events := ipc.NewBroker()
	session.SetEvents(events)
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	grace := types.NewJID("15551230002", types.DefaultUserServer)
	t0 := time.Date(2026, 10, 18, 17, 50, 0, 0, time.UTC)
	fake.emit(textMessage(ada, "A1", false, t0, "first"))
	fake.emit(textMessage(grace, "G1", false, t0, "hello"))

	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatal(err)
	}
	sock := serveSocket(t, d, events)
	watcher, wbr := subscriber(t, sock)
	conn, br := subscriber(t, sock)

	for i, want := range []string{
		`{"v":1,"op":"marked_read","jid":"15551230001@s.whatsapp.net","unread":1}`,
		`{"v":1,"op":"marked_read","jid":"15551230001@s.whatsapp.net","unread":1}`,
	} {
		if _, err := io.WriteString(conn, `{"v":1,"op":"mark_read","jid":"15551230001@s.whatsapp.net"}`+"\n"); err != nil {
			t.Fatal(err)
		}
		got := readFrame(t, conn, br, 5*time.Second)
		if i == 0 {
			// The asking connection subscribed too, so it also gets the event, in either order
			// with the reply (events are written by their own goroutine).
			const event = `{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":0}`
			other := readFrame(t, conn, br, 5*time.Second)
			if got == event {
				got, other = other, got
			}
			if other != event {
				t.Errorf("asking UI got %s besides the reply, want the event_unread", other)
			}
		}
		if got != want {
			t.Errorf("mark_read #%d -> %s, want %s", i+1, got, want)
		}
	}
	if n := fake.receipts.Load(); n != 1 {
		t.Errorf("receipts = %d, want 1 (none for the second, already read)", n)
	}
	if got := readFrame(t, watcher, wbr, 5*time.Second); got != `{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":0}` {
		t.Errorf("other UI got %s, want the event_unread", got)
	}
	if got := readFrame(t, watcher, wbr, 200*time.Millisecond); got != "" {
		t.Errorf("other UI then got %s, want nothing more", got)
	}
	if n, _ := store.UnreadCount(db, grace.String()); n != 1 {
		t.Errorf("grace unread = %d, want 1 (untouched)", n)
	}
}
