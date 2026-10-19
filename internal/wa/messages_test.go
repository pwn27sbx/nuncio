package wa

import (
	"context"
	"database/sql"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

var (
	ada   = types.NewJID("15551230001", types.DefaultUserServer)
	grace = types.NewJID("15551230002", types.DefaultUserServer)
	t0    = time.Date(2026, 10, 18, 17, 55, 4, 0, time.UTC)
)

func openTestStore(t *testing.T) *sql.DB {
	t.Helper()
	db, err := store.Open(filepath.Join(t.TempDir(), "nuncio.db"))
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	return db
}

func messageEvent(chat types.JID, id string, fromMe bool, ts time.Time, msg *waE2E.Message) *waevents.Message {
	return &waevents.Message{
		Info: types.MessageInfo{
			MessageSource: types.MessageSource{Chat: chat, Sender: chat, IsFromMe: fromMe},
			ID:            id,
			Timestamp:     ts,
		},
		Message: msg,
	}
}

func textEvent(chat types.JID, id string, fromMe bool, ts time.Time, text string) *waevents.Message {
	return messageEvent(chat, id, fromMe, ts, &waE2E.Message{Conversation: proto.String(text)})
}

func extendedTextEvent(chat types.JID, id string, fromMe bool, ts time.Time, text string) *waevents.Message {
	return messageEvent(chat, id, fromMe, ts, &waE2E.Message{
		ExtendedTextMessage: &waE2E.ExtendedTextMessage{Text: proto.String(text)},
	})
}

// startedSession is a linked session over a real store, fed through the fake client.
func startedSession(t *testing.T) (*Session, *fakeClient, *sql.DB) {
	t.Helper()
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	return s, fake, db
}

func onlyMessage(t *testing.T, db *sql.DB, chat string) store.Message {
	t.Helper()
	msgs, err := store.ListMessages(db, chat, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 1 {
		t.Fatalf("chat %s has %d messages, want 1: %+v", chat, len(msgs), msgs)
	}
	return msgs[0]
}

func unread(t *testing.T, db *sql.DB, chat string) int {
	t.Helper()
	n, err := store.UnreadCount(db, chat)
	if err != nil {
		t.Fatalf("UnreadCount: %v", err)
	}
	return n
}

func totalUnread(t *testing.T, db *sql.DB) int {
	t.Helper()
	n, err := store.TotalUnread(db)
	if err != nil {
		t.Fatalf("TotalUnread: %v", err)
	}
	return n
}

func TestIncomingTextMessageIsStoredAsUnread(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "MSG1", false, t0.In(time.FixedZone("x", 3600)), "See you at 18:00"))

	got := onlyMessage(t, db, ada.String())
	want := store.Message{ID: "MSG1", ChatJID: ada.String(), Kind: store.KindText, Text: "See you at 18:00", Timestamp: t0}
	if got != want {
		t.Errorf("stored %+v, want %+v", got, want)
	}
	if n := unread(t, db, ada.String()); n != 1 {
		t.Errorf("unread = %d, want 1", n)
	}
}

func TestIncomingExtendedTextMessageIsStored(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(extendedTextEvent(ada, "MSG2", false, t0, "a link https://example.org"))

	got := onlyMessage(t, db, ada.String())
	if got.Kind != store.KindText || got.Text != "a link https://example.org" || got.ID != "MSG2" {
		t.Errorf("stored %+v, want the extended text as a text message", got)
	}
}

func TestMessageFromMeIsStoredButNotUnread(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "MINE", true, t0, "on my way"))

	got := onlyMessage(t, db, ada.String())
	if !got.FromMe || got.Text != "on my way" {
		t.Errorf("stored %+v, want my message", got)
	}
	if n := totalUnread(t, db); n != 0 {
		t.Errorf("total unread = %d after a message from me, want 0", n)
	}
}

func TestUnreadIsCountedPerChatAndInTotal(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(extendedTextEvent(ada, "A2", false, t0.Add(time.Second), "two"))
	fake.emit(textEvent(ada, "A3", true, t0.Add(2*time.Second), "mine"))
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))

	if n := unread(t, db, ada.String()); n != 2 {
		t.Errorf("unread of ada = %d, want 2", n)
	}
	if n := unread(t, db, grace.String()); n != 1 {
		t.Errorf("unread of grace = %d, want 1", n)
	}
	if n := totalUnread(t, db); n != 3 {
		t.Errorf("total unread = %d, want 3", n)
	}
}

func TestDuplicateMessageEventIsStoredOnce(t *testing.T) {
	_, fake, db := startedSession(t)
	ev := textEvent(ada, "DUP", false, t0, "once")
	fake.emit(ev)
	fake.emit(ev)

	onlyMessage(t, db, ada.String())
	if n := totalUnread(t, db); n != 1 {
		t.Errorf("total unread = %d after a repeated event, want 1", n)
	}
}

// Media is a later task; until then a message with no text body is not stored at all.
func TestMessageWithoutTextIsIgnored(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(messageEvent(ada, "IMG", false, t0, &waE2E.Message{ImageMessage: &waE2E.ImageMessage{}}))

	if n := totalUnread(t, db); n != 0 {
		t.Errorf("total unread = %d after an image, want 0 (media is not stored yet)", n)
	}
}

// Status updates and broadcasts are not chats of the wire protocol.
func TestStatusUpdateIsNotStored(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(types.StatusBroadcastJID, "ST1", false, t0, "status"))

	chats, err := store.ListChats(db, 10)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	if len(chats) != 0 {
		t.Errorf("chats = %+v after a status update, want none", chats)
	}
}

func TestMarkChatReadClearsUnreadWithoutAReadReceipt(t *testing.T) {
	s, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(textEvent(ada, "A2", false, t0.Add(time.Second), "two"))
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))

	if err := s.MarkChatRead(ada.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	if n := unread(t, db, ada.String()); n != 0 {
		t.Errorf("unread of ada after marking it read = %d, want 0", n)
	}
	if n := totalUnread(t, db); n != 1 {
		t.Errorf("total unread = %d, want 1 (grace is still unread)", n)
	}
	// Read receipts only on an explicit view: marking read locally sends nothing.
	if fake.markReads != 0 {
		t.Errorf("MarkRead was sent %d times, want 0", fake.markReads)
	}
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times, want 1", fake.connects)
	}
}

func TestSessionWithoutStoreIgnoresMessages(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(textEvent(ada, "A1", false, t0, "one")) // must not panic
	if err := s.MarkChatRead(ada.String()); err != nil {
		t.Errorf("MarkChatRead without a store = %v, want nil", err)
	}
}

// An existing nuncio.db with no linked device stays unlinked, and its message store is ready.
func TestOpenSessionOverAnExistingDatabasePreparesTheMessageStore(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}

	s, err := OpenSession(context.Background(), path)
	if err != nil {
		t.Fatalf("OpenSession: %v", err)
	}
	defer s.Close()
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if got := s.State(); got != StateUnlinked {
		t.Fatalf("State() = %q, want %q with no device in the store", got, StateUnlinked)
	}
	if s.Store() == nil {
		t.Fatal("Store() = nil over an existing database")
	}
	if n, err := store.TotalUnread(s.Store()); err != nil || n != 0 {
		t.Errorf("TotalUnread = %d, %v; want 0 and the message schema in place", n, err)
	}
}

// eventsOf subscribes to a broker the session publishes to, and returns a function that takes
// every frame published so far, as JSON lines without the newline.
func eventsOf(t *testing.T, s *Session) func() []string {
	t.Helper()
	b := ipc.NewBroker()
	s.SetEvents(b)
	frames, cancel := b.Subscribe(nil)
	t.Cleanup(cancel)
	return func() []string {
		var got []string
		for {
			select {
			case f := <-frames:
				got = append(got, strings.TrimSuffix(string(f), "\n"))
			default:
				return got
			}
		}
	}
}

// subscribedSession is a linked session over a real store whose events are collected.
func subscribedSession(t *testing.T) (*Session, *fakeClient, func() []string) {
	t.Helper()
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	events() // the connection events of Start
	return s, fake, events
}

func assertEvents(t *testing.T, got []string, want ...string) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Errorf("events =\n %s\nwant\n %s", strings.Join(got, "\n "), strings.Join(want, "\n "))
	}
}

func TestAStoredIncomingMessagePublishesItAndTheNewUnread(t *testing.T) {
	_, fake, events := subscribedSession(t)
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))
	fake.emit(textEvent(ada, "A1", false, t0.Add(time.Second), "Running late"))

	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230002@s.whatsapp.net","message":{"id":"G1","kind":"text","text":"hello","from_me":false,"timestamp":"2026-10-18T17:55:04Z"}}`,
		`{"v":1,"op":"event_unread","unread":1,"jid":"15551230002@s.whatsapp.net","chat_unread":1}`,
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"A1","kind":"text","text":"Running late","from_me":false,"timestamp":"2026-10-18T17:55:05Z"}}`,
		`{"v":1,"op":"event_unread","unread":2,"jid":"15551230001@s.whatsapp.net","chat_unread":1}`,
	)
}

func TestAMessageFromMePublishesItButNoUnreadChange(t *testing.T) {
	_, fake, events := subscribedSession(t)
	fake.emit(textEvent(ada, "MINE", true, t0, "on my way"))

	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"MINE","kind":"text","text":"on my way","from_me":true,"timestamp":"2026-10-18T17:55:04Z"}}`,
	)
}

func TestADuplicateMessagePublishesNothing(t *testing.T) {
	_, fake, events := subscribedSession(t)
	ev := textEvent(ada, "DUP", false, t0, "once")
	fake.emit(ev)
	if got := events(); len(got) != 2 {
		t.Fatalf("first delivery published %d events, want 2: %q", len(got), got)
	}
	fake.emit(ev)
	assertEvents(t, events())
}

func TestAMessageThatIsNotStoredPublishesNothing(t *testing.T) {
	_, fake, events := subscribedSession(t)
	fake.emit(messageEvent(ada, "IMG", false, t0, &waE2E.Message{ImageMessage: &waE2E.ImageMessage{}}))
	fake.emit(textEvent(types.NewJID("status", types.BroadcastServer), "S1", false, t0, "story"))
	assertEvents(t, events())
}

func TestMarkChatReadPublishesTheClearedUnreadOnlyWhenItChanges(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))
	events()

	if err := s.MarkChatRead(ada.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	assertEvents(t, events(), `{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":0}`)

	if err := s.MarkChatRead(ada.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	assertEvents(t, events())
}
