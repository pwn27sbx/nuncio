package store

import (
	"errors"
	"fmt"
	"path/filepath"
	"testing"
	"time"
)

func outboxEntry(clientID, chat, body, waID string, at time.Time) OutboxEntry {
	return OutboxEntry{ClientID: clientID, ChatJID: chat, Text: body, MessageID: waID, Created: at}
}

func clientIDs(es []OutboxEntry) []string {
	ids := make([]string, len(es))
	for i, e := range es {
		ids[i] = e.ClientID
	}
	return ids
}

func TestEnqueueStoresAQueuedSendOnceByClientID(t *testing.T) {
	db := newMessagesDB(t)
	got, inserted, err := Enqueue(db, outboxEntry("c-1", chatAlice, "On my way", "3EB0Q1", t0))
	if err != nil || !inserted {
		t.Fatalf("Enqueue = %+v, %t, %v; want a new entry", got, inserted, err)
	}
	if got.State != OutboxQueued || got.MessageID != "3EB0Q1" || got.Text != "On my way" || !got.Created.Equal(t0) {
		t.Errorf("Enqueue = %+v, want it queued with its WhatsApp id, text and time", got)
	}
	// A second enqueue of the same client_id keeps the first entry: never a second send.
	again, inserted, err := Enqueue(db, outboxEntry("c-1", chatBob, "something else", "3EB0Q2", t0.Add(time.Minute)))
	if err != nil || inserted {
		t.Fatalf("second Enqueue = %t, %v; want the stored entry, not a new one", inserted, err)
	}
	if again.ChatJID != chatAlice || again.MessageID != "3EB0Q1" || again.Text != "On my way" {
		t.Errorf("second Enqueue = %+v, want the first entry", again)
	}
	if _, err := GetOutbox(db, "c-unknown"); !errors.Is(err, ErrNoOutboxEntry) {
		t.Errorf("GetOutbox(unknown) = %v, want ErrNoOutboxEntry", err)
	}
}

func TestQueuedOutboxIsOldestFirstAndOnlyQueued(t *testing.T) {
	db := newMessagesDB(t)
	for i, id := range []string{"c-1", "c-2", "c-3", "c-4"} {
		if _, _, err := Enqueue(db, outboxEntry(id, chatAlice, "m", fmt.Sprintf("3EB0Q%d", i), t0)); err != nil {
			t.Fatalf("Enqueue %s: %v", id, err)
		}
	}
	if err := SetOutboxState(db, "c-2", OutboxNotSent, time.Time{}); err != nil {
		t.Fatalf("SetOutboxState: %v", err)
	}
	if err := SetOutboxState(db, "c-3", OutboxSent, t0.Add(time.Minute)); err != nil {
		t.Fatalf("SetOutboxState: %v", err)
	}
	q, err := QueuedOutbox(db)
	if err != nil {
		t.Fatalf("QueuedOutbox: %v", err)
	}
	if want := []string{"c-1", "c-4"}; fmt.Sprint(clientIDs(q)) != fmt.Sprint(want) {
		t.Errorf("QueuedOutbox = %v, want %v (creation order, queued only)", clientIDs(q), want)
	}
	sent, err := GetOutbox(db, "c-3")
	if err != nil || sent.State != OutboxSent || !sent.SentAt.Equal(t0.Add(time.Minute)) {
		t.Errorf("GetOutbox(c-3) = %+v, %v; want sent at its ack time", sent, err)
	}
	notSent, err := GetOutbox(db, "c-2")
	if err != nil || notSent.State != OutboxNotSent {
		t.Errorf("GetOutbox(c-2) = %+v, %v; want not_sent", notSent, err)
	}
}

// A queued or not-sent message is listed with its chat's messages, with its state and client_id;
// a sent one is listed once, from the messages table.
func TestListMessagesShowsQueuedAndNotSentSends(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("M1", chatAlice, "hello", false, t0))
	for _, e := range []OutboxEntry{
		outboxEntry("c-1", chatAlice, "queued one", "3EB0Q1", t0.Add(1*time.Minute)),
		outboxEntry("c-2", chatAlice, "failed one", "3EB0Q2", t0.Add(2*time.Minute)),
		outboxEntry("c-3", chatAlice, "sent one", "3EB0Q3", t0.Add(3*time.Minute)),
		outboxEntry("c-4", chatBob, "other chat", "3EB0Q4", t0.Add(4*time.Minute)),
	} {
		if _, _, err := Enqueue(db, e); err != nil {
			t.Fatalf("Enqueue: %v", err)
		}
	}
	if err := SetOutboxState(db, "c-2", OutboxNotSent, time.Time{}); err != nil {
		t.Fatalf("SetOutboxState: %v", err)
	}
	if err := SetOutboxState(db, "c-3", OutboxSent, t0.Add(3*time.Minute)); err != nil {
		t.Fatalf("SetOutboxState: %v", err)
	}
	mustInsert(t, db, Message{ID: "3EB0Q3", ChatJID: chatAlice, Kind: KindText, Text: "sent one", FromMe: true, Timestamp: t0.Add(3 * time.Minute)})

	got, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if want := []string{"3EB0Q3", "3EB0Q2", "3EB0Q1", "M1"}; fmt.Sprint(messageIDs(got)) != fmt.Sprint(want) {
		t.Fatalf("ListMessages = %v, want %v", messageIDs(got), want)
	}
	if got[0].State != "" || got[0].ClientID != "" {
		t.Errorf("sent message = %+v, want no outbox state", got[0])
	}
	if m := got[1]; m.State != OutboxNotSent || m.ClientID != "c-2" || !m.FromMe || m.Kind != KindText || m.Text != "failed one" {
		t.Errorf("not-sent message = %+v, want it from me, not_sent, with its client_id", m)
	}
	if m := got[2]; m.State != OutboxQueued || m.ClientID != "c-1" || !m.Timestamp.Equal(t0.Add(time.Minute)) {
		t.Errorf("queued message = %+v, want it queued at its creation time", m)
	}
	if n, err := UnreadCount(db, chatAlice); err != nil || n != 1 {
		t.Errorf("UnreadCount = %d, %v; want 1: my own sends are never unread", n, err)
	}
	limited, err := ListMessages(db, chatAlice, 2)
	if err != nil || len(limited) != 2 {
		t.Errorf("ListMessages limit 2 = %v, %v; want 2", messageIDs(limited), err)
	}
}

// A send queued to a chat that has no messages yet still makes the chat known.
func TestEnqueueCreatesItsChat(t *testing.T) {
	db := newMessagesDB(t)
	if _, _, err := Enqueue(db, outboxEntry("c-1", chatGroup, "hi", "3EB0Q1", t0)); err != nil {
		t.Fatalf("Enqueue: %v", err)
	}
	chats, err := ListChats(db, 10)
	if err != nil || len(chats) != 1 || chats[0].JID != chatGroup {
		t.Errorf("ListChats = %v, %v; want the chat of the queued send", chatJIDs(chats), err)
	}
}

// t14Schema is the message schema as T14.1 shipped it, with chat mutes but before the outbox.
const t14Schema = t13Schema + `
CREATE TABLE chat_mutes (
	jid   TEXT PRIMARY KEY,
	until INTEGER NOT NULL
);
`

func TestADatabaseWithTheT14SchemaOpensKeepsItsRowsAndLearnsTheOutbox(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	old, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if _, err := old.Exec(t14Schema); err != nil {
		t.Fatalf("create the T14 schema: %v", err)
	}
	if err := old.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db := openMessagesDB(t, path)
	if got, err := ListMessages(db, chatAlice, 10); err != nil || fmt.Sprint(messageIDs(got)) != "[OLD1]" {
		t.Fatalf("old messages after the upgrade = %v, %v; want [OLD1]", messageIDs(got), err)
	}
	if _, _, err := Enqueue(db, outboxEntry("c-1", chatAlice, "new", "3EB0Q1", t0)); err != nil {
		t.Fatalf("Enqueue after the upgrade: %v", err)
	}
	if q, err := QueuedOutbox(db); err != nil || len(q) != 1 {
		t.Errorf("QueuedOutbox after the upgrade = %v, %v; want the new send", clientIDs(q), err)
	}
}

// RequeueFailed moves only a failed send (not_sent or not_confirmed) back to queued, once.
func TestRequeueFailedQueuesOnlyAFailedSendOnce(t *testing.T) {
	db := newMessagesDB(t)
	states := map[string]string{"c-q": OutboxQueued, "c-s": OutboxSent, "c-n": OutboxNotSent, "c-u": OutboxNotConfirmed}
	for id, st := range states {
		if _, _, err := Enqueue(db, outboxEntry(id, chatAlice, "m", "3EB0"+id, t0)); err != nil {
			t.Fatalf("Enqueue: %v", err)
		}
		if err := SetOutboxState(db, id, st, t0); err != nil {
			t.Fatalf("SetOutboxState: %v", err)
		}
	}
	for id, want := range map[string]bool{"c-q": false, "c-s": false, "c-n": true, "c-u": true} {
		got, err := RequeueFailed(db, id)
		if err != nil || got != want {
			t.Errorf("RequeueFailed(%s in %s) = %t, %v; want %t", id, states[id], got, err, want)
		}
		if again, err := RequeueFailed(db, id); err != nil || again {
			t.Errorf("second RequeueFailed(%s) = %t, %v; want false", id, again, err)
		}
	}
	if e, err := GetOutbox(db, "c-s"); err != nil || e.State != OutboxSent {
		t.Errorf("sent entry after RequeueFailed = %+v, %v; want it still sent", e, err)
	}
	// A not_confirmed send is listed with its chat like a not_sent one.
	if err := SetOutboxState(db, "c-u", OutboxNotConfirmed, time.Time{}); err != nil {
		t.Fatalf("SetOutboxState: %v", err)
	}
	listed, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	var found bool
	for _, m := range listed {
		if m.ClientID == "c-u" && m.State == OutboxNotConfirmed {
			found = true
		}
	}
	if !found {
		t.Errorf("listed %+v, want the not-confirmed send", listed)
	}
}
