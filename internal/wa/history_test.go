package wa

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
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
	waitDrains(t, s, 10*time.Second)

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
	waitDrains(t, s, 10*time.Second)
	first := importedMessages(t, s.Store(), ada.String())
	if len(first) != 2 {
		t.Fatalf("the first sync stored %d messages, want 2: %+v", len(first), first)
	}
	events() // the frames of the first sync

	fake.emit(blob)
	waitDrains(t, s, 10*time.Second)

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
	waitDrains(t, s, 10*time.Second)

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
	waitDrains(t, s, 10*time.Second)

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
	waitDrains(t, s, 10*time.Second)

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
