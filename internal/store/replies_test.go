package store

import (
	"database/sql"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

const adaJID = "15551230001@s.whatsapp.net"
const graceJID = "15551230002@s.whatsapp.net"

func openStore(t *testing.T) *sql.DB {
	t.Helper()
	db, err := Open(filepath.Join(t.TempDir(), "nuncio.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	return db
}

func insert(t *testing.T, db *sql.DB, m Message) {
	t.Helper()
	if _, err := InsertMessage(db, m); err != nil {
		t.Fatalf("InsertMessage: %v", err)
	}
}

var base = time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)

func TestSearchChatFindsOnlyThatChatsMessagesNewestFirst(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "lunch at noon", Timestamp: base})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "no lunch today", Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "a3", ChatJID: adaJID, Kind: KindText, Text: "dinner", Timestamp: base.Add(2 * time.Minute)})
	insert(t, db, Message{ID: "g1", ChatJID: graceJID, Kind: KindText, Text: "lunch?", Timestamp: base.Add(3 * time.Minute)})

	got, err := SearchChat(db, adaJID, "lunch", 10)
	if err != nil {
		t.Fatalf("SearchChat: %v", err)
	}
	var ids []string
	for _, m := range got {
		ids = append(ids, m.ID)
	}
	if !reflect.DeepEqual(ids, []string{"a2", "a1"}) {
		t.Errorf("SearchChat ids = %v, want [a2 a1] (that chat only, newest first)", ids)
	}
	if got, _ := SearchChat(db, adaJID, "lunch", 1); len(got) != 1 || got[0].ID != "a2" {
		t.Errorf("SearchChat limit 1 = %+v, want only a2", got)
	}
}

func TestSearchChatTreatsFTSSyntaxAsLiteralText(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: `say "hi" NEAR(me) OR text:x`, Timestamp: base})
	for _, q := range []string{`"`, `NEAR(`, `text:x`, `OR`, `*`, `"hi"`} {
		if _, err := SearchChat(db, adaJID, q, 10); err != nil {
			t.Errorf("SearchChat(%q) = %v, want no error", q, err)
		}
	}
}

func TestAReplyKeepsWhatItQuotes(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "question", Timestamp: base, Sender: adaJID})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "answer", Timestamp: base.Add(time.Minute), FromMe: true, ReplyTo: "a1"})

	msgs, err := ListMessages(db, adaJID, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if msgs[0].ReplyTo != "a1" || msgs[1].ReplyTo != "" {
		t.Errorf("reply_to = %q, %q; want a1 on the reply only", msgs[0].ReplyTo, msgs[1].ReplyTo)
	}
	got, err := GetMessage(db, adaJID, "a1")
	if err != nil || got.Sender != adaJID {
		t.Errorf("GetMessage sender = %q, %v; want %s", got.Sender, err, adaJID)
	}
}

func TestAReactionIsKeptOnItsMessagePerSenderAndCanBeRemoved(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "hello", Timestamp: base})
	mustReact := func(sender string, fromMe bool, emoji string) {
		t.Helper()
		if err := SetReaction(db, adaJID, "a1", sender, fromMe, emoji); err != nil {
			t.Fatalf("SetReaction: %v", err)
		}
	}
	mustReact(adaJID, false, "👍")
	mustReact("", true, "❤️")
	mustReact(adaJID, false, "😂") // replaces Ada's first reaction

	msgs, err := ListMessages(db, adaJID, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 1 {
		t.Fatalf("messages = %+v, want one: a reaction is not a new message", msgs)
	}
	want := []Reaction{{Emoji: "❤️", FromMe: true}, {Emoji: "😂", FromMe: false}}
	if !reflect.DeepEqual(msgs[0].Reactions, want) {
		t.Errorf("reactions = %+v, want %+v", msgs[0].Reactions, want)
	}
	mustReact("", true, "")
	got, _ := GetMessage(db, adaJID, "a1")
	if !reflect.DeepEqual(got.Reactions, []Reaction{{Emoji: "😂"}}) {
		t.Errorf("after removing mine, reactions = %+v, want only Ada's", got.Reactions)
	}
}

func TestAQueuedReplyKeepsItsReplyTo(t *testing.T) {
	db := openStore(t)
	e, _, err := Enqueue(db, OutboxEntry{ClientID: "c-1", ChatJID: adaJID, Text: "yes", MessageID: "3EB0A", Created: base, ReplyTo: "a1"})
	if err != nil || e.ReplyTo != "a1" {
		t.Fatalf("Enqueue = %+v, %v; want reply_to a1", e, err)
	}
	queued, err := QueuedOutbox(db)
	if err != nil || len(queued) != 1 || queued[0].ReplyTo != "a1" {
		t.Errorf("QueuedOutbox = %+v, %v; want the reply_to kept", queued, err)
	}
	msgs, _ := ListMessages(db, adaJID, 10)
	if len(msgs) != 1 || msgs[0].ReplyTo != "a1" {
		t.Errorf("listed queued send = %+v, want reply_to a1", msgs)
	}
	plain, _, _ := Enqueue(db, OutboxEntry{ClientID: "c-2", ChatJID: adaJID, Text: "plain", MessageID: "3EB0B", Created: base})
	if plain.ReplyTo != "" {
		t.Errorf("plain send reply_to = %q, want empty", plain.ReplyTo)
	}
}

// The send and its quoted id are written together: when the quote cannot be saved, the send is not
// queued either.
func TestAQueuedReplyIsNotKeptWithoutItsQuote(t *testing.T) {
	db := openStore(t)
	mustExec(t, db, `DROP TABLE outbox_replies`)
	if _, _, err := Enqueue(db, OutboxEntry{ClientID: "c-1", ChatJID: adaJID, Text: "yes", MessageID: "3EB0A", Created: base, ReplyTo: "a1"}); err == nil {
		t.Fatal("Enqueue without a place for the quote succeeded; want an error")
	}
	var n int
	if err := db.QueryRow(`SELECT COUNT(*) FROM outbox`).Scan(&n); err != nil || n != 0 {
		t.Errorf("outbox rows = %d, %v; want none: the send must not be kept without its quote", n, err)
	}
}
