package wa

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waHistorySync"
	"go.mau.fi/whatsmeow/proto/waWeb"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// team is a synthetic group chat: fake ids only, like every other fixture here.
var team = types.NewJID("120363000000000001", types.GroupServer)

// webMessage is one historical message as the phone sends it: the key names the chat, its author
// (in a group) and the message id, and the timestamp is in seconds.
func webMessage(chat, id string, fromMe bool, ts time.Time, msg *waE2E.Message) *waWeb.WebMessageInfo {
	info := &waWeb.WebMessageInfo{
		Key: &waCommon.MessageKey{
			RemoteJID: proto.String(chat),
			FromMe:    proto.Bool(fromMe),
			ID:        proto.String(id),
		},
		Message:          msg,
		MessageTimestamp: proto.Uint64(uint64(ts.Unix())),
	}
	if strings.HasSuffix(chat, "@g.us") && !fromMe {
		info.Key.Participant = proto.String(grace.String())
	}
	return info
}

// conversationOf is one conversation of a history blob, with its messages in the order the phone
// sends them (newest first).
func conversationOf(chat string, msgs ...*waWeb.WebMessageInfo) *waHistorySync.Conversation {
	c := &waHistorySync.Conversation{ID: proto.String(chat)}
	for _, m := range msgs {
		c.Messages = append(c.Messages, &waHistorySync.HistorySyncMsg{Message: m})
	}
	return c
}

// historyOf is a history-sync event carrying the given conversations, as whatsmeow emits it.
func historyOf(convs ...*waHistorySync.Conversation) *waevents.HistorySync {
	return &waevents.HistorySync{Data: &waHistorySync.HistorySync{
		SyncType:      waHistorySync.HistorySync_FULL.Enum(),
		Conversations: convs,
	}}
}

// importedMessages is every message stored in chat, newest first.
func importedMessages(t *testing.T, db *sql.DB, chat string) []store.Message {
	t.Helper()
	msgs, err := store.ListMessages(db, chat, 100)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	return msgs
}

// opsOf is the op of every frame collected, in order.
func opsOf(t *testing.T, frames []string) []string {
	t.Helper()
	ops := make([]string, len(frames))
	for i, f := range frames {
		var frame struct {
			Op string `json:"op"`
		}
		if err := json.Unmarshal([]byte(f), &frame); err != nil {
			t.Fatalf("frame %s: %v", f, err)
		}
		ops[i] = frame.Op
	}
	return ops
}

// The phone sends older messages after a device is linked. They are stored with the same row a
// live delivery produces, and as read: an old message never counts as unread.
func TestHistorySyncStoresItsMessages(t *testing.T) {
	s, fake, db := startedSession(t)
	fake.emit(historyOf(
		conversationOf(ada.String(),
			webMessage(ada.String(), "H1", false, t0, &waE2E.Message{Conversation: proto.String("old hello")}),
			webMessage(ada.String(), "H2", true, t0.Add(time.Minute), &waE2E.Message{Conversation: proto.String("old reply")}),
		),
		conversationOf(team.String(),
			webMessage(team.String(), "G1", false, t0.Add(2*time.Minute), &waE2E.Message{Conversation: proto.String("group hello")}),
		),
	))
	waitImports(t, s, 10*time.Second)

	direct := importedMessages(t, db, ada.String())
	want := []store.Message{
		{ID: "H2", ChatJID: ada.String(), Kind: store.KindText, Text: "old reply", FromMe: true, Timestamp: t0.Add(time.Minute), Read: true},
		{ID: "H1", ChatJID: ada.String(), Kind: store.KindText, Text: "old hello", Timestamp: t0, Read: true, Sender: ada.String()},
	}
	if len(direct) != len(want) {
		t.Fatalf("chat %s holds %d messages, want %d: %+v", ada, len(direct), len(want), direct)
	}
	for i := range want {
		if !sameMessage(direct[i], want[i]) {
			t.Errorf("message %d = %+v, want %+v", i, direct[i], want[i])
		}
	}

	group := importedMessages(t, db, team.String())
	if len(group) != 1 || group[0].ID != "G1" || group[0].Sender != grace.String() || group[0].Text != "group hello" {
		t.Errorf("group chat holds %+v, want G1 by %s", group, grace)
	}

	if n := totalUnread(t, db); n != 0 {
		t.Errorf("total unread = %d after a history sync, want 0", n)
	}
	if n := unread(t, db, ada.String()); n != 0 {
		t.Errorf("chat unread = %d after a history sync, want 0", n)
	}
	chats, err := store.ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 2 {
		t.Fatalf("stored chats = %+v, want the two in the blob", chats)
	}
	var adaChat store.Chat
	for _, c := range chats {
		if c.JID == ada.String() {
			adaChat = c
		}
	}
	if !adaChat.LastMessageAt.Equal(t0.Add(time.Minute)) || adaChat.Unread != 0 {
		t.Errorf("chat %+v, want its newest message at %s and no unread", adaChat, t0.Add(time.Minute))
	}
}

// sameMessage compares the fields the store round-trips; MediaRef is compared by content.
func sameMessage(got, want store.Message) bool {
	return reflect.DeepEqual(got, want)
}

// WhatsApp resends a blob: the same messages applied twice are stored once.
func TestHistorySyncAppliedTwiceStoresNothingNew(t *testing.T) {
	s, fake, events := subscribedSession(t)
	blob := historyOf(conversationOf(ada.String(),
		webMessage(ada.String(), "H1", false, t0, &waE2E.Message{Conversation: proto.String("one")}),
		webMessage(ada.String(), "H2", false, t0.Add(time.Second), &waE2E.Message{Conversation: proto.String("two")}),
	))
	fake.emit(blob)
	waitImports(t, s, 10*time.Second)
	first := importedMessages(t, s.Store(), ada.String())
	if len(first) != 2 {
		t.Fatalf("the first sync stored %d messages, want 2: %+v", len(first), first)
	}
	events() // the frames of the first sync

	fake.emit(blob)
	waitImports(t, s, 10*time.Second)

	second := importedMessages(t, s.Store(), ada.String())
	if len(second) != len(first) {
		t.Fatalf("the second sync left %d messages, want the same %d: %+v", len(second), len(first), second)
	}
	for i := range first {
		if !sameMessage(second[i], first[i]) {
			t.Errorf("message %d changed on the second sync: %+v, was %+v", i, second[i], first[i])
		}
	}
	// Nothing new to tell a client about.
	if ops := opsOf(t, events()); len(ops) != 0 {
		t.Errorf("the repeated sync published %v, want nothing", ops)
	}
}

// History is old: storing it never lights the unread badge and never publishes a read state. One
// event_chat per chat tells an open client the chat now has its last message.
func TestHistorySyncNeverCountsAsUnread(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.emit(historyOf(conversationOf(ada.String(),
		webMessage(ada.String(), "H1", false, t0, &waE2E.Message{Conversation: proto.String("one")}),
		webMessage(ada.String(), "H2", false, t0.Add(time.Second), &waE2E.Message{Conversation: proto.String("two")}),
	)))
	waitImports(t, s, 10*time.Second)

	if n := totalUnread(t, s.Store()); n != 0 {
		t.Errorf("total unread = %d, want 0", n)
	}
	frames := events()
	if ops := opsOf(t, frames); len(ops) != 1 || ops[0] != ipc.OpEventChat {
		t.Fatalf("the sync published %v, want one event_chat and nothing else", ops)
	}
	var chat struct {
		Op   string `json:"op"`
		Chat struct {
			JID         string       `json:"jid"`
			Unread      int          `json:"unread"`
			LastMessage *ipc.Message `json:"last_message"`
		} `json:"chat"`
	}
	if err := json.Unmarshal([]byte(frames[0]), &chat); err != nil {
		t.Fatalf("event_chat %s: %v", frames[0], err)
	}
	if chat.Chat.JID != ada.String() || chat.Chat.Unread != 0 {
		t.Errorf("event_chat = %+v, want chat %s with no unread", chat.Chat, ada)
	}
	if chat.Chat.LastMessage == nil || chat.Chat.LastMessage.ID != "H2" {
		t.Errorf("event_chat carries last_message %+v, want H2", chat.Chat.LastMessage)
	}
}

// Protocol entries, system entries and chats the wire does not carry are not messages: they are
// skipped, and they do not create a chat.
func TestHistorySyncIgnoresWhatIsNotAMessage(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.emit(historyOf(
		conversationOf("status@broadcast",
			webMessage("status@broadcast", "S1", false, t0, &waE2E.Message{Conversation: proto.String("a status")}),
		),
		conversationOf("not a jid",
			webMessage("not a jid", "N1", false, t0, &waE2E.Message{Conversation: proto.String("nowhere")}),
		),
		conversationOf(ada.String(),
			webMessage(ada.String(), "P1", false, t0, &waE2E.Message{ProtocolMessage: &waE2E.ProtocolMessage{
				Type: waE2E.ProtocolMessage_REVOKE.Enum(),
			}}),
			webMessage(ada.String(), "P2", false, t0, nil),
			webMessage(ada.String(), "P3", false, t0, &waE2E.Message{}),
			webMessage(ada.String(), "P4", false, t0, &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{
				Text: proto.String("\U0001F44D"),
			}}),
		),
	))
	waitImports(t, s, 10*time.Second)

	chats, err := store.ListChats(s.Store(), 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 0 {
		t.Errorf("stored chats = %+v, want none", chats)
	}
	if ops := opsOf(t, events()); len(ops) != 0 {
		t.Errorf("published %v, want nothing", ops)
	}
}

// The safety net behind that stop: an import that cannot end (here, the store's one connection is
// held by the test, so the import is blocked before its first write) is not awaited either. Close
// answers the import's error, leaves the store open, and the import closes it when it ends; nothing
// closes a store under a write.
func TestCloseAnswersWhenAnImportCannotStop(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	// A session that never reports a connection: no outbox drain may wait for the connection the
	// test is holding.
	fake := &fakeClient{device: true, connecting: true}
	var storeClosed atomic.Bool
	s := NewSession(fake, db, func() error {
		storeClosed.Store(true)
		return db.Close()
	})
	s.importStop = 10 * time.Millisecond
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}

	tx, err := db.Begin()
	if err != nil {
		t.Fatalf("Begin: %v", err)
	}
	fake.emit(bigHistory(ada.String(), "B", 100))
	// The import holds historyMu while it waits for that connection.
	blocked := false
	for deadline := time.Now().Add(5 * time.Second); !blocked && time.Now().Before(deadline); {
		if s.historyMu.TryLock() {
			s.historyMu.Unlock()
			time.Sleep(2 * time.Millisecond)
			continue
		}
		blocked = true
	}
	if !blocked {
		t.Fatal("the history import never reached the store")
	}

	if err := s.Close(); !errors.Is(err, errImportRunning) {
		t.Fatalf("Close with an import blocked in the store = %v, want errImportRunning", err)
	}
	if storeClosed.Load() {
		t.Error("Close closed the store under a running import")
	}
	// It ends when the store answers again, and closes the store itself.
	if err := tx.Rollback(); err != nil {
		t.Fatalf("Rollback: %v", err)
	}
	waitFor(t, "the import to end and close the store", func() bool { return storeClosed.Load() })
}

// waitImports waits for the running history imports, failing the test if they do not end within d.
func waitImports(t *testing.T, s *Session, d time.Duration) {
	t.Helper()
	done := make(chan struct{})
	go func() { s.imports.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(d):
		t.Fatalf("the import did not end within %s", d)
	}
}

// historyBlobSize is the size of the blob the close test feeds the session: a full sync is
// thousands of messages (the owner's own carried 943), so an import of this one cannot finish in
// the seconds a closed session may take, and a Close that waited for it is visible as such.
const historyBlobSize = 60000

// bigHistory is one conversation of n text messages, as the phone sends a full history sync. The
// ids carry prefix, so a test tells the messages of one blob from another's.
func bigHistory(chat, prefix string, n int) *waevents.HistorySync {
	msgs := make([]*waWeb.WebMessageInfo, n)
	for i := range msgs {
		msgs[i] = webMessage(chat, fmt.Sprintf("%s%d", prefix, i), false, t0.Add(time.Duration(i)*time.Second),
			&waE2E.Message{Conversation: proto.String("history")})
	}
	return historyOf(conversationOf(chat, msgs...))
}

// storedCount is how many messages the store at path holds, read through a database of its own.
func storedCount(t *testing.T, path string) int {
	return storedIDs(t, path, "")
}

// storedIDs is how many messages the store at path holds whose id starts with prefix (all of them
// with ""), read through a database of its own.
func storedIDs(t *testing.T, path, prefix string) int {
	t.Helper()
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	defer func() { _ = db.Close() }()
	var n int
	if err := db.QueryRow(`SELECT COUNT(*) FROM messages WHERE id LIKE ?`, prefix+"%").Scan(&n); err != nil {
		t.Fatalf("cannot count the stored messages: %v", err)
	}
	return n
}

// A full history sync is thousands of messages. Closing the session must not wait for it to end:
// that is what left the daemon alive, holding the bus name and with no socket, when the owner
// unlinked right after linking. Close asks the import to stop, the import ends at its next
// message, and the store is closed once no import is writing to it.
func TestCloseDoesNotWaitForARunningHistoryImport(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	fake := &fakeClient{device: true}
	// storeClosed records that the session closed the store its imports write to.
	var storeClosed atomic.Bool
	s := NewSession(fake, db, func() error {
		storeClosed.Store(true)
		return db.Close()
	})
	var mu sync.Mutex
	var logs []string
	s.logf = func(format string, args ...any) {
		mu.Lock()
		defer mu.Unlock()
		logs = append(logs, fmt.Sprintf(format, args...))
	}
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}

	fake.emit(bigHistory(ada.String(), "B", historyBlobSize))
	// A second blob arrives while the first is being stored: it is queued behind it (one blob at a
	// time) and must be dropped, not imported after the close.
	fake.emit(bigHistory(ada.String(), "Q", 5))
	waitFor(t, "the history import to store its first message", func() bool {
		var n int
		if err := db.QueryRow(`SELECT COUNT(*) FROM messages`).Scan(&n); err != nil {
			return false
		}
		return n > 0
	})

	closed := make(chan struct{})
	go func() {
		s.Close()
		close(closed)
	}()
	select {
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("Close waited for the history import instead of stopping it")
	}

	// The store is closed once the import stopped, and before Close returns: a write into a
	// closed store would have been logged by its type.
	if !storeClosed.Load() {
		t.Error("Close answered without closing the store")
	}
	// The import stopped at its next message: the blob was not stored whole, and the blob that
	// was queued behind it stored nothing at all.
	if n := storedCount(t, path); n >= historyBlobSize {
		t.Errorf("the import stored all %d messages before the session closed", n)
	}
	if n := storedIDs(t, path, "Q"); n != 0 {
		t.Errorf("the queued blob stored %d messages after the session closed", n)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(logs) != 0 {
		t.Errorf("the close logged %v, want nothing", logs)
	}
}

// A blob that cannot be stored is logged by its error type only (the store's text could quote a
// message or a JID), and the rest of the blob is still imported.
func TestAFailedHistoryMessageIsLoggedByTypeAndTheImportGoesOn(t *testing.T) {
	s, fake, db := startedSession(t)
	var logs []string
	s.logf = func(format string, args ...any) { logs = append(logs, fmt.Sprintf(format, args...)) }
	// One chat the store refuses, so the failure is the store's and not a broken fixture.
	if _, err := db.Exec(fmt.Sprintf(
		`CREATE TRIGGER refuse BEFORE INSERT ON messages WHEN NEW.chat_jid = '%s' BEGIN SELECT RAISE(ABORT, 'refused'); END`,
		grace.String())); err != nil {
		t.Fatalf("cannot refuse inserts for one chat: %v", err)
	}
	fake.emit(historyOf(
		conversationOf(grace.String(),
			webMessage(grace.String(), "R1", false, t0, &waE2E.Message{Conversation: proto.String("never stored")}),
		),
		conversationOf(ada.String(),
			webMessage(ada.String(), "A1", false, t0, &waE2E.Message{Conversation: proto.String("stored")}),
		),
	))
	waitImports(t, s, 10*time.Second)

	if msgs := importedMessages(t, db, ada.String()); len(msgs) != 1 || msgs[0].ID != "A1" {
		t.Errorf("the blob after the failing chat holds %+v, want A1 stored", msgs)
	}
	if len(logs) != 1 {
		t.Fatalf("logged %v, want exactly the one failure", logs)
	}
	if !strings.HasPrefix(logs[0], "wa: cannot store a historical message (") {
		t.Errorf("logged %q, want the failure by its error type", logs[0])
	}
	for _, secret := range []string{"never stored", "stored", grace.String(), ada.String()} {
		if strings.Contains(logs[0], secret) {
			t.Errorf("the log line names %q: %q", secret, logs[0])
		}
	}
}
