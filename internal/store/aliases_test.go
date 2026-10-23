package store

import (
	"crypto/sha256"
	"database/sql"
	"errors"
	"fmt"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
	"time"
)

// A contact as WhatsApp addresses it twice: by phone number (fake) and by hidden id (LID, fake).
const (
	adaPN  = "15551230001@s.whatsapp.net"
	adaLID = "100000000000001@lid"
)

// dbHash is a hash of every row of every table and of the schema, so a test can prove a database
// did not change at all.
func dbHash(t *testing.T, db *sql.DB) string {
	t.Helper()
	rows, err := db.Query(`SELECT name, COALESCE(sql, '') FROM sqlite_master ORDER BY name`)
	if err != nil {
		t.Fatalf("read the schema: %v", err)
	}
	var tables []string
	h := sha256.New()
	for rows.Next() {
		var name, ddl string
		if err := rows.Scan(&name, &ddl); err != nil {
			t.Fatalf("read the schema: %v", err)
		}
		fmt.Fprintln(h, name, ddl)
		if strings.HasPrefix(ddl, "CREATE TABLE") {
			tables = append(tables, name)
		}
	}
	_ = rows.Close()
	sort.Strings(tables)
	for _, table := range tables {
		r, err := db.Query(`SELECT * FROM "` + table + `" ORDER BY 1, 2`)
		if err != nil {
			// FTS shadow tables hold blobs; ordering by rowid works for every table.
			r, err = db.Query(`SELECT * FROM "` + table + `"`)
			if err != nil {
				t.Fatalf("read %s: %v", table, err)
			}
		}
		cols, _ := r.Columns()
		for r.Next() {
			vals := make([]any, len(cols))
			ptrs := make([]any, len(cols))
			for i := range vals {
				ptrs[i] = &vals[i]
			}
			if err := r.Scan(ptrs...); err != nil {
				t.Fatalf("read %s: %v", table, err)
			}
			fmt.Fprintln(h, table, vals)
		}
		_ = r.Close()
	}
	return fmt.Sprintf("%x", h.Sum(nil))
}

// seedSplit stores the home scenario of T15: a send to Ada's phone-number JID made a second chat
// next to her LID chat, and her reply in the LID chat quotes the message stored in the other one.
func seedSplit(t *testing.T, db *sql.DB) {
	t.Helper()
	// The phone-number chat: named, pinned, muted for 8 hours.
	if err := UpsertChat(db, Chat{JID: adaPN, Name: "Ada", Pinned: true}); err != nil {
		t.Fatal(err)
	}
	if err := SetChatMutedUntil(db, adaPN, t0.Add(8*time.Hour)); err != nil {
		t.Fatal(err)
	}
	mustInsert(t, db, text("P1", adaPN, "lunch at noon", true, t0))
	p2 := text("P2", adaPN, "pn only incoming", false, t0.Add(time.Minute))
	p2.Sender = adaPN
	mustInsert(t, db, p2)
	mustInsert(t, db, Message{ID: "P3", ChatJID: adaPN, Kind: KindImage, Text: "a photo", Timestamp: t0.Add(2 * time.Minute),
		Mime: "image/jpeg", Size: 10, MediaRef: []byte{0x07}, Sender: adaPN})
	d1 := text("D1", adaPN, "both copies", false, t0.Add(3*time.Minute))
	d1.Sender = adaPN
	mustInsert(t, db, d1)
	if err := MarkMessagesRead(db, adaPN, []string{"D1"}); err != nil {
		t.Fatal(err)
	}
	if err := SetReaction(db, adaPN, "P1", adaPN, false, "👍"); err != nil {
		t.Fatal(err)
	}
	if err := SetReaction(db, adaPN, "D1", "", true, "🎉"); err != nil {
		t.Fatal(err)
	}
	// A send to the phone-number JID that failed, quoting P1, and one that was sent (P1).
	if _, _, err := Enqueue(db, OutboxEntry{ClientID: "c-1", ChatJID: adaPN, Text: "still there?", MessageID: "P9",
		Created: t0.Add(4 * time.Minute), ReplyTo: "P1"}); err != nil {
		t.Fatal(err)
	}
	if err := SetOutboxState(db, "c-1", OutboxNotSent, time.Time{}); err != nil {
		t.Fatal(err)
	}
	if _, _, err := Enqueue(db, OutboxEntry{ClientID: "c-0", ChatJID: adaPN, Text: "lunch at noon", MessageID: "P1",
		Created: t0}); err != nil {
		t.Fatal(err)
	}
	if err := SetOutboxState(db, "c-0", OutboxSent, t0); err != nil {
		t.Fatal(err)
	}

	// The LID chat: archived, muted with no end, her reply quoting P1 and her copy of D1, unread.
	if err := SetChatArchived(db, adaLID, true); err != nil {
		t.Fatal(err)
	}
	if err := SetChatMutedUntil(db, adaLID, MutedForever); err != nil {
		t.Fatal(err)
	}
	l1 := text("L1", adaLID, "yes, see you", false, t0.Add(5*time.Minute))
	l1.Sender, l1.ReplyTo = adaLID, "P1"
	mustInsert(t, db, l1)
	d1lid := text("D1", adaLID, "both copies", false, t0.Add(3*time.Minute))
	d1lid.Sender = adaLID
	mustInsert(t, db, d1lid)
}

func TestMergeChatMovesASplitConversationIntoTheCanonicalChat(t *testing.T) {
	db := newMessagesDB(t)
	seedSplit(t, db)

	merged, err := MergeChat(db, adaPN, adaLID)
	if err != nil || !merged {
		t.Fatalf("MergeChat = %v, %v; want merged", merged, err)
	}

	chats, err := ListChats(db, 10)
	if err != nil || len(chats) != 1 || chats[0].JID != adaLID {
		t.Fatalf("ListChats = %+v, %v; want only the LID chat", chats, err)
	}
	c := chats[0]
	if c.Name != "Ada" || !c.Pinned || c.Archived || !c.MutedUntil.Equal(MutedForever) {
		t.Errorf("merged chat = %+v; want named Ada, pinned, not archived (one copy was in the inbox), muted forever", c)
	}
	// L1, P2 and P3 are unread; D1 was read in one copy, so it is read.
	if c.Unread != 3 {
		t.Errorf("merged unread = %d, want 3", c.Unread)
	}
	if n, err := TotalUnread(db); err != nil || n != 3 {
		t.Errorf("TotalUnread = %d, %v; want 3", n, err)
	}

	msgs, err := ListMessages(db, adaLID, 50)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if got, want := messageIDs(msgs), []string{"L1", "P9", "D1", "P3", "P2", "P1"}; !reflect.DeepEqual(got, want) {
		t.Errorf("merged messages = %v, want %v (each id once, the failed send listed)", got, want)
	}
	if msgs, _ := ListMessages(db, adaPN, 50); len(msgs) != 0 {
		t.Errorf("the phone-number chat still lists %v", messageIDs(msgs))
	}
	var rows int
	if err := db.QueryRow(`SELECT COUNT(*) FROM messages`).Scan(&rows); err != nil || rows != 5 {
		t.Errorf("messages rows = %d, %v; want 5 (the duplicate D1 kept once)", rows, err)
	}

	// The quote of L1 resolves in the same chat; the sender of her phone-number messages is her LID.
	q, err := GetMessage(db, adaLID, "P1")
	if err != nil || len(q.Reactions) != 1 || q.Reactions[0].Emoji != "👍" {
		t.Errorf("quoted P1 = %+v, %v; want it with Ada's reaction", q, err)
	}
	if l1, err := GetMessage(db, adaLID, "L1"); err != nil || l1.ReplyTo != "P1" {
		t.Errorf("L1 = %+v, %v; want reply_to P1", l1, err)
	}
	if p2, err := GetMessage(db, adaLID, "P2"); err != nil || p2.Sender != adaLID {
		t.Errorf("P2 = %+v, %v; want sender %s", p2, err, adaLID)
	}
	if p3, err := GetMessage(db, adaLID, "P3"); err != nil || string(p3.MediaRef) != "\x07" {
		t.Errorf("P3 = %+v, %v; want its media reference", p3, err)
	}
	if d1, err := GetMessage(db, adaLID, "D1"); err != nil || !d1.Read || len(d1.Reactions) != 1 || !d1.Reactions[0].FromMe {
		t.Errorf("D1 = %+v, %v; want it read with my reaction", d1, err)
	}
	var reactions int
	if err := db.QueryRow(`SELECT COUNT(*) FROM reactions WHERE chat_jid = ?`, adaPN).Scan(&reactions); err != nil || reactions != 0 {
		t.Errorf("reactions left under the phone number = %d, %v", reactions, err)
	}

	// The outbox follows: the failed send can still be retried, and it still quotes P1.
	e, err := GetOutbox(db, "c-1")
	if err != nil || e.ChatJID != adaLID || e.ReplyTo != "P1" || e.State != OutboxNotSent {
		t.Errorf("outbox c-1 = %+v, %v; want it in the LID chat, quoting P1, not sent", e, err)
	}

	// Search finds the merged messages, in the canonical chat.
	if hits, err := SearchChat(db, adaLID, "lunch", 10); err != nil || !reflect.DeepEqual(messageIDs(hits), []string{"P1"}) {
		t.Errorf("SearchChat(lid, lunch) = %v, %v; want P1", messageIDs(hits), err)
	}
	hits, err := Search(db, "copies", 10)
	if err != nil || len(hits) != 1 || hits[0].ChatJID != adaLID {
		t.Errorf("Search(copies) = %+v, %v; want D1 once, in the LID chat", hits, err)
	}
	var fts int
	if err := db.QueryRow(`SELECT COUNT(*) FROM messages_fts`).Scan(&fts); err != nil || fts != 5 {
		t.Errorf("messages_fts rows = %d, %v; want 5", fts, err)
	}

	var canonical string
	if err := db.QueryRow(`SELECT canonical FROM chat_aliases WHERE alias = ?`, adaPN).Scan(&canonical); err != nil ||
		canonical != adaLID {
		t.Errorf("chat_aliases[%s] = %q, %v; want %s", adaPN, canonical, err, adaLID)
	}

	// A second run finds nothing to move and changes nothing.
	before := dbHash(t, db)
	merged, err = MergeChat(db, adaPN, adaLID)
	if err != nil || merged {
		t.Errorf("second MergeChat = %v, %v; want nothing merged", merged, err)
	}
	if dbHash(t, db) != before {
		t.Error("the second MergeChat changed the database")
	}
}

func TestMergeChatIntoAChatNotStoredYetKeepsTheAliasFlags(t *testing.T) {
	db := newMessagesDB(t)
	if err := UpsertChat(db, Chat{JID: adaPN, Name: "Ada", Archived: true}); err != nil {
		t.Fatal(err)
	}
	mustInsert(t, db, text("P1", adaPN, "hi", false, t0))
	if merged, err := MergeChat(db, adaPN, adaLID); err != nil || !merged {
		t.Fatalf("MergeChat = %v, %v", merged, err)
	}
	c, err := GetChat(db, adaLID)
	if err != nil || c.Name != "Ada" || !c.Archived || c.Unread != 1 {
		t.Errorf("merged chat = %+v, %v; want Ada, archived, 1 unread", c, err)
	}
	if _, err := GetChat(db, adaPN); !errors.Is(err, ErrNoChat) {
		t.Errorf("GetChat(pn) = %v, want ErrNoChat", err)
	}
}

func TestMergeChatWithNothingUnderTheAliasChangesNothing(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("L1", adaLID, "hi", false, t0))
	before := dbHash(t, db)
	merged, err := MergeChat(db, adaPN, adaLID)
	if err != nil || merged {
		t.Errorf("MergeChat = %v, %v; want nothing merged", merged, err)
	}
	if dbHash(t, db) != before {
		t.Error("MergeChat with nothing to move changed the database")
	}
}

func TestMergeChatRefusesToMergeAChatIntoItself(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("L1", adaLID, "hi", false, t0))
	if _, err := MergeChat(db, adaLID, adaLID); err == nil {
		t.Error("MergeChat(x, x) succeeded, want an error")
	}
}

func TestAFailureMidMergeRollsEverythingBack(t *testing.T) {
	db := newMessagesDB(t)
	seedSplit(t, db)
	// Removing the phone-number chat row is the merge's last step; make it fail there, after every
	// other table has been rewritten inside the transaction.
	mustExec(t, db, `CREATE TRIGGER fail_merge BEFORE DELETE ON chats WHEN old.jid = '`+adaPN+`'
		BEGIN SELECT RAISE(ABORT, 'injected failure'); END`)
	before := dbHash(t, db)

	merged, err := MergeChat(db, adaPN, adaLID)
	if err == nil || merged {
		t.Fatalf("MergeChat = %v, %v; want the injected failure", merged, err)
	}
	if strings.Contains(err.Error(), "15551230001") {
		t.Errorf("the error names the number: %v", err)
	}
	if dbHash(t, db) != before {
		t.Error("a failed MergeChat left the database changed")
	}
}

func TestPhoneChatsListsEveryStoredPhoneNumberChat(t *testing.T) {
	db := newMessagesDB(t)
	mustInsert(t, db, text("A", adaPN, "x", false, t0))
	mustInsert(t, db, text("B", adaLID, "x", false, t0))
	mustInsert(t, db, text("G", chatGroup, "x", false, t0))
	// A reaction can be stored before anything else of its chat.
	if err := SetReaction(db, chatBob, "Z", chatBob, false, "👍"); err != nil {
		t.Fatal(err)
	}
	// Rows of every other table MergeChat moves, with no chats row (as a database edited by hand or
	// an older build could hold).
	const muted, meta, media = "15550000003@s.whatsapp.net", "15550000004@s.whatsapp.net", "15550000005@s.whatsapp.net"
	mustExec(t, db, `INSERT INTO chat_mutes (jid, until) VALUES (?, -1)`, muted)
	mustExec(t, db, `INSERT INTO message_meta (chat_jid, id, sender) VALUES (?, 'X', ?)`, meta, meta)
	mustExec(t, db, `INSERT INTO media_refs (chat_jid, id, ref) VALUES (?, 'Y', x'01')`, media)
	got, err := PhoneChats(db)
	if want := []string{chatBob, muted, meta, media, adaPN}; err != nil || !reflect.DeepEqual(got, want) {
		t.Errorf("PhoneChats = %v, %v; want %v", got, err, want)
	}
}

// t15Schema is the message schema a binary before chat_aliases runs on every open (commit 8dcad27).
const t15Schema = `
CREATE TABLE IF NOT EXISTS chats (jid TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
	archived INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY, chat_jid TEXT NOT NULL, id TEXT NOT NULL,
	kind TEXT NOT NULL, text TEXT NOT NULL DEFAULT '', from_me INTEGER NOT NULL, ts INTEGER NOT NULL,
	read INTEGER NOT NULL DEFAULT 0, mime TEXT, size INTEGER, file_name TEXT, UNIQUE (chat_jid, id));
CREATE INDEX IF NOT EXISTS messages_chat_ts ON messages (chat_jid, ts);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text);
CREATE TABLE IF NOT EXISTS media_refs (chat_jid TEXT NOT NULL, id TEXT NOT NULL, ref BLOB NOT NULL,
	PRIMARY KEY (chat_jid, id));
CREATE TABLE IF NOT EXISTS chat_mutes (jid TEXT PRIMARY KEY, until INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS outbox (seq INTEGER PRIMARY KEY, client_id TEXT NOT NULL UNIQUE,
	chat_jid TEXT NOT NULL, text TEXT NOT NULL, message_id TEXT NOT NULL, created INTEGER NOT NULL,
	state TEXT NOT NULL, sent_at INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS message_meta (chat_jid TEXT NOT NULL, id TEXT NOT NULL,
	sender TEXT NOT NULL DEFAULT '', reply_to TEXT NOT NULL DEFAULT '', PRIMARY KEY (chat_jid, id));
CREATE TABLE IF NOT EXISTS reactions (chat_jid TEXT NOT NULL, id TEXT NOT NULL, sender TEXT NOT NULL,
	from_me INTEGER NOT NULL, emoji TEXT NOT NULL, PRIMARY KEY (chat_jid, id, sender));
CREATE TABLE IF NOT EXISTS outbox_replies (client_id TEXT PRIMARY KEY, reply_to TEXT NOT NULL);
`

// A database written before chat_aliases opens with the new schema and merges; a merged database
// still opens with the old binary's schema, whose queries (the same store functions) read it.
func TestASplitDatabaseFromBeforeChatAliasesMergesAndStillOpensForAnOldBinary(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	old, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := old.Exec(t15Schema); err != nil {
		t.Fatalf("create the pre-alias schema: %v", err)
	}
	seedSplit(t, old)
	if err := old.Close(); err != nil {
		t.Fatal(err)
	}

	db, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema over the pre-alias schema: %v", err)
	}
	if merged, err := MergeChat(db, adaPN, adaLID); err != nil || !merged {
		t.Fatalf("MergeChat = %v, %v", merged, err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}

	// The old binary: its own schema statements, then its reads, over the merged file.
	back, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = back.Close() })
	if _, err := back.Exec(t15Schema); err != nil {
		t.Fatalf("the pre-alias schema over a merged database: %v", err)
	}
	chats, err := ListChats(back, 10)
	if err != nil || len(chats) != 1 || chats[0].JID != adaLID || chats[0].Unread != 3 {
		t.Fatalf("ListChats for the old binary = %+v, %v; want the one merged chat", chats, err)
	}
	if msgs, err := ListMessages(back, adaLID, 50); err != nil || len(msgs) != 6 {
		t.Errorf("ListMessages for the old binary = %v, %v; want 6", messageIDs(msgs), err)
	}
	if hits, err := Search(back, "lunch", 10); err != nil || len(hits) != 1 {
		t.Errorf("Search for the old binary = %+v, %v", hits, err)
	}
	mustInsert(t, back, text("N1", adaLID, "new after downgrade", false, t0.Add(time.Hour)))
}

// AliasesOf lists the phone-number chats merged into a chat (from chat_aliases), in JID order, and
// nothing for a chat nothing was merged into.
func TestAliasesOfListsThePhoneNumberChatsMergedIntoAChat(t *testing.T) {
	db := openStore(t)
	const otherPN = "15551230009@s.whatsapp.net"
	for _, pn := range []string{otherPN, adaPN} {
		if _, err := InsertMessage(db, Message{ID: "P-" + pn, ChatJID: pn, Kind: KindText, Text: "hi",
			Timestamp: time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)}); err != nil {
			t.Fatal(err)
		}
		if merged, err := MergeChat(db, pn, adaLID); err != nil || !merged {
			t.Fatalf("MergeChat(%s) = %v, %v", pn, merged, err)
		}
	}
	got, err := AliasesOf(db, adaLID)
	if err != nil || !reflect.DeepEqual(got, []string{adaPN, otherPN}) {
		t.Errorf("AliasesOf = %q, %v; want both numbers in JID order", got, err)
	}
	if got, err := AliasesOf(db, "100000000000002@lid"); err != nil || len(got) != 0 {
		t.Errorf("AliasesOf an unmerged chat = %q, %v; want none", got, err)
	}
}
