package store

import (
	"database/sql"
	"errors"
	"fmt"
	"path/filepath"
	"testing"
	"time"
)

// Fake chat ids: never real phone numbers.
const (
	chatAlice = "15550000001@s.whatsapp.net"
	chatBob   = "15550000002@s.whatsapp.net"
	chatGroup = "120363000000000001@g.us"
)

// t0 is a fixed base time so ordering is deterministic.
var t0 = time.Date(2026, 10, 19, 12, 0, 0, 0, time.UTC)

// openMessagesDB opens nuncio.db through store.Open with the chat schema in place.
func openMessagesDB(t *testing.T, path string) *sql.DB {
	t.Helper()
	db, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	return db
}

func newMessagesDB(t *testing.T) *sql.DB {
	t.Helper()
	return openMessagesDB(t, filepath.Join(t.TempDir(), "nuncio.db"))
}

func mustInsert(t *testing.T, db *sql.DB, m Message) {
	t.Helper()
	inserted, err := InsertMessage(db, m)
	if err != nil {
		t.Fatalf("InsertMessage %s: %v", m.ID, err)
	}
	if !inserted {
		t.Fatalf("InsertMessage %s reported a duplicate", m.ID)
	}
}

func text(id, chat, body string, fromMe bool, at time.Time) Message {
	return Message{ID: id, ChatJID: chat, Kind: KindText, Text: body, FromMe: fromMe, Timestamp: at}
}

func messageIDs(ms []Message) []string {
	ids := make([]string, len(ms))
	for i, m := range ms {
		ids[i] = m.ID
	}
	return ids
}

func chatJIDs(cs []Chat) []string {
	jids := make([]string, len(cs))
	for i, c := range cs {
		jids[i] = c.JID
	}
	return jids
}

func hitIDs(hs []SearchHit) []string {
	ids := make([]string, len(hs))
	for i, h := range hs {
		ids[i] = h.MessageID
	}
	return ids
}

func TestInsertChatAndMessageRoundTrip(t *testing.T) {
	db := newMessagesDB(t)
	if err := UpsertChat(db, Chat{JID: chatAlice, Name: "Alice", Pinned: true}); err != nil {
		t.Fatalf("UpsertChat: %v", err)
	}
	media := Message{
		ID: "M1", ChatJID: chatAlice, Kind: KindImage, Text: "a caption", FromMe: true,
		Timestamp: t0, Mime: "image/jpeg", Size: 1234, FileName: "cat.jpg",
	}
	mustInsert(t, db, media)

	msgs, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 1 {
		t.Fatalf("ListMessages = %v, want one message", messageIDs(msgs))
	}
	got := msgs[0]
	if got.ID != "M1" || got.ChatJID != chatAlice || got.Kind != KindImage || got.Text != "a caption" ||
		!got.FromMe || !got.Timestamp.Equal(t0) || got.Mime != "image/jpeg" || got.Size != 1234 ||
		got.FileName != "cat.jpg" {
		t.Fatalf("round trip = %+v, want %+v", got, media)
	}

	chats, err := ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 1 || chats[0].JID != chatAlice || chats[0].Name != "Alice" || !chats[0].Pinned ||
		chats[0].Archived || !chats[0].LastMessageAt.Equal(t0) {
		t.Fatalf("ListChats = %+v, want Alice pinned with last message at %v", chats, t0)
	}

	// Updating a chat changes its fields instead of adding a second row.
	if err := UpsertChat(db, Chat{JID: chatAlice, Name: "Alice B", Archived: true}); err != nil {
		t.Fatalf("UpsertChat update: %v", err)
	}
	chats, err = ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 1 || chats[0].Name != "Alice B" || !chats[0].Archived || chats[0].Pinned {
		t.Fatalf("after update ListChats = %+v, want one archived unpinned 'Alice B'", chats)
	}
}

func TestInsertMessageCreatesItsChat(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("M1", chatBob, "hi", false, t0))
	chats, err := ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if fmt.Sprint(chatJIDs(chats)) != fmt.Sprint([]string{chatBob}) {
		t.Fatalf("ListChats = %v, want [%s]", chatJIDs(chats), chatBob)
	}
}

func TestListMessagesNewestFirstWithLimit(t *testing.T) {
	db := newMessagesDB(t)
	// Inserted out of order so the result order comes from the timestamp.
	mustInsert(t, db, text("M2", chatAlice, "second", false, t0.Add(2*time.Minute)))
	mustInsert(t, db, text("M1", chatAlice, "first", false, t0.Add(1*time.Minute)))
	mustInsert(t, db, text("M3", chatAlice, "third", false, t0.Add(3*time.Minute)))
	mustInsert(t, db, text("B1", chatBob, "other chat", false, t0.Add(4*time.Minute)))

	all, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if want := []string{"M3", "M2", "M1"}; fmt.Sprint(messageIDs(all)) != fmt.Sprint(want) {
		t.Fatalf("ListMessages = %v, want %v", messageIDs(all), want)
	}

	two, err := ListMessages(db, chatAlice, 2)
	if err != nil {
		t.Fatalf("ListMessages limit 2: %v", err)
	}
	if want := []string{"M3", "M2"}; fmt.Sprint(messageIDs(two)) != fmt.Sprint(want) {
		t.Fatalf("ListMessages limit 2 = %v, want %v", messageIDs(two), want)
	}
}

func TestListChatsNewestFirstWithLimit(t *testing.T) {
	db := newMessagesDB(t)
	// The time order differs from the jid order in both directions.
	mustInsert(t, db, text("A1", chatAlice, "old", false, t0))
	mustInsert(t, db, text("G1", chatGroup, "older", false, t0.Add(2*time.Minute)))
	mustInsert(t, db, text("B1", chatBob, "newer", false, t0.Add(3*time.Minute)))
	// A newer message moves Alice to the top.
	mustInsert(t, db, text("A2", chatAlice, "newest", false, t0.Add(4*time.Minute)))

	all, err := ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if want := []string{chatAlice, chatBob, chatGroup}; fmt.Sprint(chatJIDs(all)) != fmt.Sprint(want) {
		t.Fatalf("ListChats = %v, want %v", chatJIDs(all), want)
	}

	two, err := ListChats(db, 2)
	if err != nil {
		t.Fatalf("ListChats limit 2: %v", err)
	}
	if want := []string{chatAlice, chatBob}; fmt.Sprint(chatJIDs(two)) != fmt.Sprint(want) {
		t.Fatalf("ListChats limit 2 = %v, want %v", chatJIDs(two), want)
	}
}

func TestSearchMatch(t *testing.T) {
	db := newMessagesDB(t)
	// The better match (M2) is the older one, so the order comes from bm25, not time.
	mustInsert(t, db, text("M1", chatAlice, "the compiler is done", false, t0.Add(time.Minute)))
	mustInsert(t, db, text("M2", chatGroup, "compiler compiler compiler", false, t0))
	mustInsert(t, db, text("M3", chatBob, "see you at six", false, t0.Add(2*time.Minute)))

	t.Run("matches with chat jid ranked by bm25", func(t *testing.T) {
		hits, err := Search(db, "compiler", 10)
		if err != nil {
			t.Fatalf("Search: %v", err)
		}
		// M2 repeats the term in a shorter text, so bm25 ranks it first.
		if want := []string{"M2", "M1"}; fmt.Sprint(hitIDs(hits)) != fmt.Sprint(want) {
			t.Fatalf("Search compiler = %v, want %v", hitIDs(hits), want)
		}
		if hits[0].ChatJID != chatGroup || hits[0].Text != "compiler compiler compiler" ||
			!hits[0].Timestamp.Equal(t0) {
			t.Fatalf("first hit = %+v, want M2 in %s", hits[0], chatGroup)
		}
	})

	t.Run("all terms must match", func(t *testing.T) {
		hits, err := Search(db, "compiler done", 10)
		if err != nil {
			t.Fatalf("Search: %v", err)
		}
		if want := []string{"M1"}; fmt.Sprint(hitIDs(hits)) != fmt.Sprint(want) {
			t.Fatalf("Search 'compiler done' = %v, want %v", hitIDs(hits), want)
		}
	})

	t.Run("limit", func(t *testing.T) {
		hits, err := Search(db, "compiler", 1)
		if err != nil {
			t.Fatalf("Search: %v", err)
		}
		if want := []string{"M2"}; fmt.Sprint(hitIDs(hits)) != fmt.Sprint(want) {
			t.Fatalf("Search limit 1 = %v, want %v", hitIDs(hits), want)
		}
	})

	t.Run("no match", func(t *testing.T) {
		hits, err := Search(db, "whatsapp", 10)
		if err != nil {
			t.Fatalf("Search: %v", err)
		}
		if len(hits) != 0 {
			t.Fatalf("Search whatsapp = %v, want none", hitIDs(hits))
		}
	})

	t.Run("fts syntax in the query is not an error", func(t *testing.T) {
		for _, q := range []string{`"`, `compiler"`, `AND`, `compiler OR`, `(`, `text:`, `*`, `-x`, `NEAR(`, `^`, `   `} {
			if _, err := Search(db, q, 10); err != nil {
				t.Errorf("Search(%q): %v", q, err)
			}
		}
		hits, err := Search(db, `compiler"`, 10)
		if err != nil {
			t.Fatalf("Search: %v", err)
		}
		if want := []string{"M2", "M1"}; fmt.Sprint(hitIDs(hits)) != fmt.Sprint(want) {
			t.Fatalf(`Search compiler" = %v, want %v`, hitIDs(hits), want)
		}
	})
}

func TestUnreadCounts(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("A1", chatAlice, "one", false, t0))
	mustInsert(t, db, text("A2", chatAlice, "two", false, t0.Add(time.Minute)))
	mustInsert(t, db, text("A3", chatAlice, "mine", true, t0.Add(2*time.Minute)))
	mustInsert(t, db, text("B1", chatBob, "hey", false, t0.Add(3*time.Minute)))
	mustInsert(t, db, text("G1", chatGroup, "mine too", true, t0.Add(4*time.Minute)))

	assertUnread := func(t *testing.T, alice, bob, group, total int) {
		t.Helper()
		for jid, want := range map[string]int{chatAlice: alice, chatBob: bob, chatGroup: group} {
			got, err := UnreadCount(db, jid)
			if err != nil {
				t.Fatalf("UnreadCount %s: %v", jid, err)
			}
			if got != want {
				t.Errorf("UnreadCount %s = %d, want %d", jid, got, want)
			}
		}
		got, err := TotalUnread(db)
		if err != nil {
			t.Fatalf("TotalUnread: %v", err)
		}
		if got != total {
			t.Errorf("TotalUnread = %d, want %d", got, total)
		}
		chats, err := ListChats(db, 10)
		if err != nil {
			t.Fatalf("ListChats: %v", err)
		}
		for _, c := range chats {
			want := map[string]int{chatAlice: alice, chatBob: bob, chatGroup: group}[c.JID]
			if c.Unread != want {
				t.Errorf("ListChats %s Unread = %d, want %d", c.JID, c.Unread, want)
			}
		}
	}

	t.Run("own messages are never unread", func(t *testing.T) {
		assertUnread(t, 2, 1, 0, 3)
	})

	t.Run("marking a chat read clears only that chat", func(t *testing.T) {
		if err := MarkChatRead(db, chatAlice); err != nil {
			t.Fatalf("MarkChatRead: %v", err)
		}
		assertUnread(t, 0, 1, 0, 1)
	})

	t.Run("a new incoming message is unread again", func(t *testing.T) {
		mustInsert(t, db, text("A4", chatAlice, "again", false, t0.Add(5*time.Minute)))
		assertUnread(t, 1, 1, 0, 2)
	})
}

func TestReopenKeepsEverything(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	db, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	if err := UpsertChat(db, Chat{JID: chatAlice, Name: "Alice", Pinned: true}); err != nil {
		t.Fatalf("UpsertChat: %v", err)
	}
	mustInsert(t, db, text("A1", chatAlice, "persisted compiler", false, t0))
	mustInsert(t, db, text("A2", chatAlice, "read one", false, t0.Add(time.Minute)))
	if err := MarkChatRead(db, chatAlice); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	mustInsert(t, db, text("A3", chatAlice, "unread one", false, t0.Add(2*time.Minute)))
	if err := db.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	// Reopening runs the schema again on an existing database.
	db = openMessagesDB(t, path)
	msgs, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if want := []string{"A3", "A2", "A1"}; fmt.Sprint(messageIDs(msgs)) != fmt.Sprint(want) {
		t.Fatalf("after reopen ListMessages = %v, want %v", messageIDs(msgs), want)
	}
	chats, err := ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 1 || chats[0].Name != "Alice" || !chats[0].Pinned {
		t.Fatalf("after reopen ListChats = %+v, want pinned Alice", chats)
	}
	hits, err := Search(db, "compiler", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if want := []string{"A1"}; fmt.Sprint(hitIDs(hits)) != fmt.Sprint(want) {
		t.Fatalf("after reopen Search = %v, want %v", hitIDs(hits), want)
	}
	if n, err := TotalUnread(db); err != nil || n != 1 {
		t.Fatalf("after reopen TotalUnread = %d, %v; want 1", n, err)
	}
	// A duplicate after reopen is still recognised.
	if inserted, err := InsertMessage(db, text("A1", chatAlice, "persisted compiler", false, t0)); err != nil || inserted {
		t.Fatalf("after reopen duplicate InsertMessage = %v, %v; want false, nil", inserted, err)
	}
}

func TestDuplicateMessageIsIgnored(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("M1", chatAlice, "original compiler", false, t0))

	inserted, err := InsertMessage(db, text("M1", chatAlice, "replayed compiler", false, t0.Add(time.Hour)))
	if err != nil {
		t.Fatalf("duplicate InsertMessage: %v", err)
	}
	if inserted {
		t.Fatal("duplicate InsertMessage reported inserted=true, want false")
	}

	msgs, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 1 || msgs[0].Text != "original compiler" || !msgs[0].Timestamp.Equal(t0) {
		t.Fatalf("ListMessages = %+v, want only the original M1", msgs)
	}
	hits, err := Search(db, "compiler", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(hits) != 1 || hits[0].Text != "original compiler" {
		t.Fatalf("Search = %+v, want one hit for the original", hits)
	}
	if n, err := UnreadCount(db, chatAlice); err != nil || n != 1 {
		t.Fatalf("UnreadCount = %d, %v; want 1", n, err)
	}

	// The same id in another chat is a different message.
	mustInsert(t, db, text("M1", chatBob, "same id, other chat", false, t0))
}

func TestMediaMessageKeepsItsDescriptionAndDownloadReference(t *testing.T) {
	db := newMessagesDB(t)
	img := Message{ID: "IMG1", ChatJID: chatAlice, Kind: KindImage, Text: "the venue", Timestamp: t0,
		Mime: "image/jpeg", Size: 48213, MediaRef: []byte{0x0a, 0x03, 'r', 'e', 'f'}}
	doc := Message{ID: "DOC1", ChatJID: chatAlice, Kind: KindDocument, Timestamp: t0.Add(time.Minute),
		Mime: "application/pdf", Size: 1200, FileName: "menu.pdf", MediaRef: []byte{0x01}}
	mustInsert(t, db, img)
	mustInsert(t, db, doc)
	mustInsert(t, db, text("TXT1", chatAlice, "plain", false, t0.Add(2*time.Minute)))

	msgs, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 3 {
		t.Fatalf("ListMessages = %+v, want 3 messages", msgs)
	}
	if got := msgs[1]; got.Kind != KindDocument || got.Mime != "application/pdf" || got.Size != 1200 || got.FileName != "menu.pdf" {
		t.Errorf("listed document = %+v, want its mime, size and file name", got)
	}
	if got := msgs[2]; got.Kind != KindImage || got.Text != "the venue" || got.Mime != "image/jpeg" || got.Size != 48213 {
		t.Errorf("listed image = %+v, want kind, caption, mime and size", got)
	}

	got, err := GetMessage(db, chatAlice, "IMG1")
	if err != nil {
		t.Fatalf("GetMessage IMG1: %v", err)
	}
	if string(got.MediaRef) != string(img.MediaRef) || got.Kind != KindImage || got.Mime != "image/jpeg" {
		t.Errorf("GetMessage IMG1 = %+v, want the stored image with its reference %v", got, img.MediaRef)
	}
	plain, err := GetMessage(db, chatAlice, "TXT1")
	if err != nil {
		t.Fatalf("GetMessage TXT1: %v", err)
	}
	if plain.Kind != KindText || plain.MediaRef != nil {
		t.Errorf("GetMessage TXT1 = %+v, want a text message with no reference", plain)
	}
	for _, key := range [][2]string{{chatAlice, "NOPE"}, {chatBob, "IMG1"}} {
		if _, err := GetMessage(db, key[0], key[1]); !errors.Is(err, ErrNoMessage) {
			t.Errorf("GetMessage(%s, %s) = %v, want ErrNoMessage", key[0], key[1], err)
		}
	}
	// A duplicate keeps the first reference.
	dup := img
	dup.MediaRef = []byte{0xff}
	if inserted, err := InsertMessage(db, dup); err != nil || inserted {
		t.Fatalf("duplicate InsertMessage = %v, %v; want false, nil", inserted, err)
	}
	if got, err := GetMessage(db, chatAlice, "IMG1"); err != nil || string(got.MediaRef) != string(img.MediaRef) {
		t.Errorf("after a duplicate GetMessage = %+v, %v; want the first reference", got, err)
	}
}

// t5Schema is the message schema as T5 shipped it, before the media references: a nuncio.db
// created by that build must keep working.
const t5Schema = `
CREATE TABLE chats (
	jid      TEXT PRIMARY KEY,
	name     TEXT NOT NULL DEFAULT '',
	archived INTEGER NOT NULL DEFAULT 0,
	pinned   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE messages (
	seq       INTEGER PRIMARY KEY,
	chat_jid  TEXT NOT NULL,
	id        TEXT NOT NULL,
	kind      TEXT NOT NULL,
	text      TEXT NOT NULL DEFAULT '',
	from_me   INTEGER NOT NULL,
	ts        INTEGER NOT NULL,
	read      INTEGER NOT NULL DEFAULT 0,
	mime      TEXT,
	size      INTEGER,
	file_name TEXT,
	UNIQUE (chat_jid, id)
);
CREATE INDEX messages_chat_ts ON messages (chat_jid, ts);
CREATE VIRTUAL TABLE messages_fts USING fts5(text);
INSERT INTO chats (jid, name, pinned) VALUES ('15550000001@s.whatsapp.net', 'Alice', 1);
INSERT INTO messages (seq, chat_jid, id, kind, text, from_me, ts) VALUES
	(1, '15550000001@s.whatsapp.net', 'OLD1', 'text', 'kept compiler', 0, 1760875200000000000);
INSERT INTO messages_fts (rowid, text) VALUES (1, 'kept compiler');
`

func TestADatabaseWithTheT5SchemaOpensAndKeepsItsRows(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	old, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if _, err := old.Exec(t5Schema); err != nil {
		t.Fatalf("create the T5 schema: %v", err)
	}
	if err := old.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db := openMessagesDB(t, path)
	msgs, err := ListMessages(db, chatAlice, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 1 || msgs[0].ID != "OLD1" || msgs[0].Text != "kept compiler" {
		t.Fatalf("ListMessages after the upgrade = %+v, want the old row", msgs)
	}
	if hits, err := Search(db, "compiler", 10); err != nil || len(hits) != 1 {
		t.Fatalf("Search after the upgrade = %+v, %v; want the old row", hits, err)
	}
	if chats, err := ListChats(db, 10); err != nil || len(chats) != 1 || chats[0].Name != "Alice" {
		t.Fatalf("ListChats after the upgrade = %+v, %v; want Alice", chats, err)
	}
	mustInsert(t, db, Message{ID: "NEW1", ChatJID: chatAlice, Kind: KindImage, Timestamp: t0,
		Mime: "image/jpeg", Size: 10, MediaRef: []byte{0x01, 0x02}})
	if got, err := GetMessage(db, chatAlice, "NEW1"); err != nil || string(got.MediaRef) != "\x01\x02" {
		t.Fatalf("GetMessage after the upgrade = %+v, %v; want the new reference", got, err)
	}
	if got, err := GetMessage(db, chatAlice, "OLD1"); err != nil || got.MediaRef != nil {
		t.Fatalf("GetMessage of the old row = %+v, %v; want it with no reference", got, err)
	}
}

func chatOf(t *testing.T, db *sql.DB, jid string) Chat {
	t.Helper()
	chats, err := ListChats(db, 200)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	for _, c := range chats {
		if c.JID == jid {
			return c
		}
	}
	t.Fatalf("chat %s not stored: %+v", jid, chats)
	return Chat{}
}

func TestSetChatPinnedAndArchivedChangeOnlyTheirFlag(t *testing.T) {
	db := newMessagesDB(t)
	if err := UpsertChat(db, Chat{JID: chatAlice, Name: "Alice"}); err != nil {
		t.Fatalf("UpsertChat: %v", err)
	}
	if err := SetChatPinned(db, chatAlice, true); err != nil {
		t.Fatalf("SetChatPinned: %v", err)
	}
	if err := SetChatArchived(db, chatAlice, true); err != nil {
		t.Fatalf("SetChatArchived: %v", err)
	}
	if c := chatOf(t, db, chatAlice); !c.Pinned || !c.Archived || c.Name != "Alice" {
		t.Fatalf("after pin and archive = %+v, want pinned, archived, named Alice", c)
	}
	if err := SetChatPinned(db, chatAlice, false); err != nil {
		t.Fatalf("SetChatPinned: %v", err)
	}
	if c := chatOf(t, db, chatAlice); c.Pinned || !c.Archived {
		t.Fatalf("after unpin = %+v, want unpinned and still archived", c)
	}
	if err := SetChatArchived(db, chatAlice, false); err != nil {
		t.Fatalf("SetChatArchived: %v", err)
	}
	if c := chatOf(t, db, chatAlice); c.Pinned || c.Archived || c.Name != "Alice" {
		t.Fatalf("after unarchive = %+v, want no flags, named Alice", c)
	}
}

func TestSetChatMutedUntilKeepsForeverAnEndTimeAndUnmute(t *testing.T) {
	db := newMessagesDB(t)
	if err := SetChatMutedUntil(db, chatBob, MutedForever); err != nil {
		t.Fatalf("SetChatMutedUntil forever: %v", err)
	}
	if c := chatOf(t, db, chatBob); !c.MutedUntil.Equal(MutedForever) || !c.Muted(t0) {
		t.Fatalf("muted forever = %+v, want MutedForever", c)
	}
	end := t0.Add(8 * time.Hour)
	if err := SetChatMutedUntil(db, chatBob, end); err != nil {
		t.Fatalf("SetChatMutedUntil: %v", err)
	}
	c := chatOf(t, db, chatBob)
	if !c.MutedUntil.Equal(end) || !c.Muted(t0) || c.Muted(end) {
		t.Fatalf("muted until %v = %+v, want muted before and not at the end", end, c)
	}
	if err := SetChatMutedUntil(db, chatBob, time.Time{}); err != nil {
		t.Fatalf("SetChatMutedUntil unmute: %v", err)
	}
	if c := chatOf(t, db, chatBob); !c.MutedUntil.IsZero() || c.Muted(t0) {
		t.Fatalf("after unmute = %+v, want not muted", c)
	}
}

// t13Schema is the message schema as T13 shipped it, with media references but before chat mutes.
const t13Schema = t5Schema + `
CREATE TABLE media_refs (
	chat_jid TEXT NOT NULL,
	id       TEXT NOT NULL,
	ref      BLOB NOT NULL,
	PRIMARY KEY (chat_jid, id)
);
`

func TestADatabaseWithTheT13SchemaOpensKeepsItsFlagsAndLearnsMutes(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	old, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if _, err := old.Exec(t13Schema); err != nil {
		t.Fatalf("create the T13 schema: %v", err)
	}
	if err := old.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db := openMessagesDB(t, path)
	c := chatOf(t, db, chatAlice)
	if c.Name != "Alice" || !c.Pinned || !c.MutedUntil.IsZero() || c.Unread != 1 {
		t.Fatalf("old chat after the upgrade = %+v, want Alice pinned, unmuted, 1 unread", c)
	}
	if err := SetChatMutedUntil(db, chatAlice, MutedForever); err != nil {
		t.Fatalf("SetChatMutedUntil after the upgrade: %v", err)
	}
	if c := chatOf(t, db, chatAlice); !c.Muted(t0) || !c.Pinned || c.Name != "Alice" {
		t.Fatalf("muted old chat = %+v, want muted, still pinned and named", c)
	}
}

func TestChatFlagsOfAnUnknownChatCreateIt(t *testing.T) {
	db := newMessagesDB(t)
	if err := SetChatPinned(db, chatAlice, true); err != nil {
		t.Fatalf("SetChatPinned: %v", err)
	}
	if err := SetChatArchived(db, chatBob, true); err != nil {
		t.Fatalf("SetChatArchived: %v", err)
	}
	if err := SetChatMutedUntil(db, chatGroup, MutedForever); err != nil {
		t.Fatalf("SetChatMutedUntil: %v", err)
	}
	if c := chatOf(t, db, chatAlice); !c.Pinned {
		t.Errorf("new pinned chat = %+v", c)
	}
	if c := chatOf(t, db, chatBob); !c.Archived {
		t.Errorf("new archived chat = %+v", c)
	}
	if c := chatOf(t, db, chatGroup); !c.Muted(t0) {
		t.Errorf("new muted chat = %+v", c)
	}
}
