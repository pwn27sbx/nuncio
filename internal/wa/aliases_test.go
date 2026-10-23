package wa

import (
	"context"
	"database/sql"
	"fmt"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// adaLID is Ada (ada, her phone-number JID) as WhatsApp addresses her by hidden id: fake.
var adaLID = types.NewJID("100000000000001", types.HiddenUserServer)

// lidClient is fakeClient with whatsmeow's LID map: it knows the phone-number -> LID pairs in lids,
// which a test can add to while the session runs, as whatsmeow learns them.
type lidClient struct {
	*fakeClient
	mu   sync.Mutex
	lids map[string]string
}

func newLIDClient(pairs ...string) *lidClient {
	c := &lidClient{fakeClient: &fakeClient{device: true}, lids: map[string]string{}}
	for i := 0; i+1 < len(pairs); i += 2 {
		c.lids[pairs[i]] = pairs[i+1]
	}
	return c
}

func (c *lidClient) learn(pn, lid string) {
	c.mu.Lock()
	c.lids[pn] = lid
	c.mu.Unlock()
}

func (c *lidClient) LIDForPN(_ context.Context, pn string) (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.lids[pn], nil
}

func (c *lidClient) PNForLID(_ context.Context, lid string) (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for pn, l := range c.lids {
		if l == lid {
			return pn, nil
		}
	}
	return "", nil
}

// lidSession is a linked session over db and client c, its events collected and its log captured.
func lidSession(t *testing.T, c Client, db *sql.DB) (*Session, func() []string, *[]string) {
	t.Helper()
	s := NewSession(c, db, nil)
	var logs []string
	var logMu sync.Mutex
	s.logf = func(format string, args ...any) {
		logMu.Lock()
		logs = append(logs, fmt.Sprintf(format, args...))
		logMu.Unlock()
	}
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	events()
	return s, events, &logs
}

func chatJIDsOf(t *testing.T, db *sql.DB) []string {
	t.Helper()
	chats, err := store.ListChats(db, 50)
	if err != nil {
		t.Fatal(err)
	}
	var jids []string
	for _, c := range chats {
		jids = append(jids, c.JID)
	}
	return jids
}

// quotingReply is Ada's reply in chat quoting message quoted.
func quotingReply(chat types.JID, id, quoted, text string, at time.Time) *waevents.Message {
	evt := messageEvent(chat, id, false, at, &waE2E.Message{ExtendedTextMessage: &waE2E.ExtendedTextMessage{
		Text: proto.String(text), ContextInfo: &waE2E.ContextInfo{StanzaID: proto.String(quoted)}}})
	return evt
}

// The T15 home scenario: a send to Ada's phone-number JID, then her reply in the LID chat quoting
// it. One chat, and the quote resolves.
func TestASendToThePhoneNumberAndAReplyQuotingItAreOneConversation(t *testing.T) {
	c := newLIDClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	s, events, _ := lidSession(t, c, db)
	c.emit(textEvent(adaLID, "L0", false, t0, "hello"))
	c.sendID = "3EB0S1"
	events()

	if _, err := s.Send(ada.String(), "lunch?", "c-1"); err != nil {
		t.Fatalf("Send: %v", err)
	}
	if sends, _ := c.sent(); !reflect.DeepEqual(sends, []string{adaLID.String() + " lunch?"}) {
		t.Errorf("sends = %q, want the send addressed to the LID chat", sends)
	}
	got := events()
	if len(got) != 1 || !strings.Contains(got[0], `"jid":"`+adaLID.String()+`"`) {
		t.Errorf("events of the send = %q, want one event_message for the LID chat", got)
	}

	reply := quotingReply(adaLID, "L1", "3EB0S1", "yes!", t0.Add(time.Minute))
	reply.Info.Sender = adaLID
	c.emit(reply)

	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String()}) {
		t.Fatalf("chats = %v, want only the LID chat", jids)
	}
	l1, err := store.GetMessage(db, adaLID.String(), "L1")
	if err != nil || l1.ReplyTo != "3EB0S1" {
		t.Fatalf("L1 = %+v, %v; want reply_to 3EB0S1", l1, err)
	}
	if q, err := store.GetMessage(db, adaLID.String(), l1.ReplyTo); err != nil || q.Text != "lunch?" || !q.FromMe {
		t.Errorf("the quoted message = %+v, %v; want my send, in the same chat", q, err)
	}
	if e, err := store.GetOutbox(db, "c-1"); err != nil || e.ChatJID != adaLID.String() {
		t.Errorf("outbox c-1 = %+v, %v; want it in the LID chat", e, err)
	}
}

// whatsmeow learns the LID of a phone number it did not know while it sends to it (GetUserInfo,
// user.go:265-276); the send then lands in the LID chat and anything stored under the number moves.
func TestASendToAnUnmappedNumberMovesToTheLIDChatOnceWhatsAppLearnsIt(t *testing.T) {
	c := newLIDClient()
	db := openTestStore(t)
	s, events, _ := lidSession(t, c, db)
	c.emit(textEvent(adaLID, "L0", false, t0, "hello"))
	c.sendID = "3EB0S1"
	c.onSend = func(string) { c.learn(ada.String(), adaLID.String()) }
	events()

	if _, err := s.Send(ada.String(), "lunch?", "c-1"); err != nil {
		t.Fatalf("Send: %v", err)
	}
	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String()}) {
		t.Fatalf("chats = %v, want only the LID chat", jids)
	}
	if m, err := store.GetMessage(db, adaLID.String(), "3EB0S1"); err != nil || m.Text != "lunch?" {
		t.Errorf("the send = %+v, %v; want it stored in the LID chat", m, err)
	}
	if e, err := store.GetOutbox(db, "c-1"); err != nil || e.ChatJID != adaLID.String() || e.State != store.OutboxSent {
		t.Errorf("outbox c-1 = %+v, %v; want it sent, in the LID chat", e, err)
	}
	// Only event_chat's `aliases` may name the number, never an event's chat.
	for _, e := range events() {
		if strings.Contains(e, `"jid":"`+ada.String()+`"`) {
			t.Errorf("an event names the phone-number chat: %s", e)
		}
	}
}

// A store split before this fix (home) is merged when the session starts and the mapping is known.
func TestStartMergesAStoredSplitWhoseMappingIsKnown(t *testing.T) {
	db := openTestStore(t)
	for _, m := range []store.Message{
		{ID: "P1", ChatJID: ada.String(), Kind: store.KindText, Text: "lunch?", FromMe: true, Timestamp: t0},
		{ID: "L1", ChatJID: adaLID.String(), Kind: store.KindText, Text: "yes", Timestamp: t0.Add(time.Minute),
			Sender: adaLID.String(), ReplyTo: "P1"},
		{ID: "G1", ChatJID: grace.String(), Kind: store.KindText, Text: "hi", Timestamp: t0},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	c := newLIDClient(ada.String(), adaLID.String())
	_, _, logs := lidSession(t, c, db)

	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String(), grace.String()}) {
		t.Fatalf("chats = %v, want Ada's LID chat and Grace's unmapped one", jids)
	}
	if _, err := store.GetMessage(db, adaLID.String(), "P1"); err != nil {
		t.Errorf("P1 = %v; want it in the LID chat, where L1 quotes it", err)
	}
	if len(*logs) != 0 {
		t.Errorf("logs = %q, want none", *logs)
	}
}

// A mapping whatsmeow learns from an incoming message (message.go:51-55) merges the chat stored
// under the number when that message arrives.
func TestAnIncomingMessageMergesTheNumberChatOnceItsMappingIsLearnt(t *testing.T) {
	c := newLIDClient()
	db := openTestStore(t)
	_, events, _ := lidSession(t, c, db)
	c.emit(textEvent(ada, "P1", true, t0, "lunch?"))
	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{ada.String()}) {
		t.Fatalf("chats before the mapping = %v", jids)
	}
	events()

	c.learn(ada.String(), adaLID.String())
	reply := quotingReply(adaLID, "L1", "P1", "yes", t0.Add(time.Minute))
	reply.Info.Sender = adaLID
	c.emit(reply)

	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String()}) {
		t.Fatalf("chats = %v, want only the LID chat", jids)
	}
	if _, err := store.GetMessage(db, adaLID.String(), "P1"); err != nil {
		t.Errorf("P1 = %v; want it merged into the LID chat", err)
	}
	var chatEvent bool
	for _, e := range events() {
		if strings.Contains(e, `"op":"event_chat"`) && strings.Contains(e, adaLID.String()) {
			chatEvent = true
		}
		if strings.Contains(e, `"jid":"`+ada.String()+`"`) {
			t.Errorf("an event names the phone-number chat: %s", e)
		}
	}
	if !chatEvent {
		t.Error("no event_chat for the merged LID chat")
	}
}

// Every request naming the phone-number JID acts on the LID chat.
func TestRequestsNamingThePhoneNumberActOnTheLIDChat(t *testing.T) {
	c := newLIDClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	s, _, _ := lidSession(t, c, db)
	in := textEvent(adaLID, "L1", false, t0, "hello")
	in.Info.Sender = adaLID
	c.emit(in)

	if got := s.CanonicalJID(ada.String()); got != adaLID.String() {
		t.Errorf("CanonicalJID(pn) = %s, want %s", got, adaLID)
	}
	if got := s.CanonicalJID(grace.String()); got != grace.String() {
		t.Errorf("CanonicalJID(unmapped) = %s, want it unchanged", got)
	}
	if got := s.CanonicalJID(group.String()); got != group.String() {
		t.Errorf("CanonicalJID(group) = %s, want it unchanged", got)
	}
	if err := s.React(ada.String(), "L1", "👍"); err != nil {
		t.Errorf("React by the number: %v", err)
	}
	if _, err := s.SendReply(ada.String(), "hi back", "c-1", "L1"); err != nil {
		t.Errorf("SendReply by the number quoting a LID-chat message: %v", err)
	}
	if _, err := s.MarkRead(ada.String()); err != nil {
		t.Errorf("MarkRead by the number: %v", err)
	}
	if len(c.markReadCalls) != 1 || !strings.HasPrefix(c.markReadCalls[0], adaLID.String()+" ") {
		t.Errorf("receipts = %q, want one for the LID chat", c.markReadCalls)
	}
	if n := unread(t, db, adaLID.String()); n != 0 {
		t.Errorf("LID chat unread = %d, want 0", n)
	}
	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String()}) {
		t.Errorf("chats = %v, want only the LID chat", jids)
	}
}

// script drives one session through what a user does in a phone-number chat, and returns every
// event it published and the stored chats and messages.
func script(t *testing.T, c Client, emit func(any)) []string {
	t.Helper()
	db := openTestStore(t)
	s, events, _ := lidSession(t, c, db)
	in := textEvent(ada, "A1", false, t0, "hello")
	in.Info.Sender = ada
	emit(in)
	_, _ = s.Send(ada.String(), "hi", "c-1")
	_, _ = s.SendReply(ada.String(), "about that", "c-2", "A1")
	_ = s.React(ada.String(), "A1", "👍")
	_, _ = s.MarkRead(ada.String())
	out := events()
	msgs, err := store.ListMessages(db, ada.String(), 50)
	if err != nil {
		t.Fatal(err)
	}
	out = append(out, fmt.Sprint(chatJIDsOf(t, db)), fmt.Sprintf("%+v", msgs))
	return out
}

// Without a mapping nothing changes: the same frames, byte for byte, and the same store.
func TestWithoutAMappingAPhoneNumberChatBehavesExactlyAsBefore(t *testing.T) {
	plain := &fakeClient{device: true, sendTime: t0.Add(time.Hour)}
	want := script(t, plain, plain.emit)
	unmapped := newLIDClient(grace.String(), "100000000000002@lid")
	unmapped.sendTime = t0.Add(time.Hour)
	got := script(t, unmapped, unmapped.emit)
	if !reflect.DeepEqual(got, want) {
		t.Errorf("with an unrelated mapping =\n %s\nwithout any =\n %s", strings.Join(got, "\n "), strings.Join(want, "\n "))
	}
	if len(want) < 5 {
		t.Errorf("the script published too little to compare: %q", want)
	}
}

// A merge that fails rolls back, is logged once without a number or a LID, and is not retried for
// every message.
func TestAFailedMergeKeepsBothChatsAndIsLoggedOnceWithoutNumbers(t *testing.T) {
	db := openTestStore(t)
	if _, err := store.InsertMessage(db, store.Message{ID: "P1", ChatJID: ada.String(), Kind: store.KindText,
		Text: "lunch?", FromMe: true, Timestamp: t0}); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TRIGGER fail_merge BEFORE DELETE ON chats WHEN old.jid = '` + ada.String() + `'
		BEGIN SELECT RAISE(ABORT, 'injected'); END`); err != nil {
		t.Fatal(err)
	}
	c := newLIDClient(ada.String(), adaLID.String())
	s, _, logs := lidSession(t, c, db)
	for i := range 3 {
		in := textEvent(adaLID, fmt.Sprintf("L%d", i), false, t0.Add(time.Duration(i+1)*time.Minute), "yes")
		in.Info.Sender = adaLID
		c.emit(in)
	}

	if _, err := store.GetMessage(db, ada.String(), "P1"); err != nil {
		t.Errorf("P1 = %v; want it still in the phone-number chat", err)
	}
	// Both chats are kept as they are, so the history left under the number stays readable by it.
	if got := s.CanonicalJID(ada.String()); got != ada.String() {
		t.Errorf("CanonicalJID after a failed merge = %s, want the phone-number chat itself", got)
	}
	if len(*logs) != 1 {
		t.Fatalf("logs = %q, want one line", *logs)
	}
	for _, digits := range []string{ada.User, adaLID.User} {
		if strings.Contains((*logs)[0], digits) {
			t.Errorf("the log names a number or LID: %s", (*logs)[0])
		}
	}
}

// A LID map answer that is not a LID chat JID is not a mapping.
func TestALIDMapAnswerThatIsNotALIDIsIgnored(t *testing.T) {
	c := newLIDClient(ada.String(), grace.String())
	s, _, _ := lidSession(t, c, openTestStore(t))
	if got := s.CanonicalJID(ada.String()); got != ada.String() {
		t.Errorf("CanonicalJID = %s, want it unchanged", got)
	}
}

// A message, a reaction or an app-state change addressed by phone number lands in the LID chat when
// the mapping is known, and the contact is recorded as the sender by her LID.
func TestEventsAddressedByPhoneNumberLandInTheLIDChat(t *testing.T) {
	c := newLIDClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	_, events, _ := lidSession(t, c, db)
	in := textEvent(ada, "P1", false, t0, "hello")
	in.Info.Sender = ada
	c.emit(in)
	r := messageEvent(ada, "R1", false, t0.Add(time.Second), &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{
		Key: &waCommon.MessageKey{ID: proto.String("P1")}, Text: proto.String("👍")}})
	r.Info.Sender = ada
	c.emit(r)
	c.emit(pinEvent(ada, true))

	if jids := chatJIDsOf(t, db); !reflect.DeepEqual(jids, []string{adaLID.String()}) {
		t.Fatalf("chats = %v, want only the LID chat", jids)
	}
	m, err := store.GetMessage(db, adaLID.String(), "P1")
	if err != nil || m.Sender != adaLID.String() || len(m.Reactions) != 1 {
		t.Errorf("P1 = %+v, %v; want it in the LID chat, sent by her LID, with her reaction", m, err)
	}
	if ch, err := store.GetChat(db, adaLID.String()); err != nil || !ch.Pinned {
		t.Errorf("LID chat = %+v, %v; want it pinned", ch, err)
	}
	for _, e := range events() {
		if strings.Contains(e, ada.User) {
			t.Errorf("an event names the phone number: %s", e)
		}
	}
}

// chatEvents are the event_chat frames among events.
func chatEvents(events []string) []string {
	var out []string
	for _, e := range events {
		if strings.Contains(e, `"op":"event_chat"`) {
			out = append(out, e)
		}
	}
	return out
}

// A merge publishes the LID chat with the phone number merged into it (`aliases`), so a client
// holding the number adopts the LID chat; the alias is listed only while whatsmeow's mapping still
// points the number there.
func TestAMergedChatIsPublishedAndListedWithItsAliasWhileTheMappingPointsThere(t *testing.T) {
	c := newLIDClient()
	db := openTestStore(t)
	s, events, _ := lidSession(t, c, db)
	c.emit(textEvent(ada, "P1", true, t0, "lunch?"))
	events()

	c.learn(ada.String(), adaLID.String())
	reply := quotingReply(adaLID, "L1", "P1", "yes", t0.Add(time.Minute))
	reply.Info.Sender = adaLID
	c.emit(reply)
	got := chatEvents(events())
	want := `"aliases":["` + ada.String() + `"]`
	if len(got) != 1 || !strings.Contains(got[0], `"jid":"`+adaLID.String()+`"`) || !strings.Contains(got[0], want) {
		t.Fatalf("event_chat after the merge = %q, want the LID chat with %s", got, want)
	}
	if a := s.ChatAliases(adaLID.String()); !reflect.DeepEqual(a, []string{ada.String()}) {
		t.Errorf("ChatAliases = %q, want the phone number", a)
	}

	// The number is recycled: whatsmeow now maps it to someone else's LID.
	c.learn(ada.String(), "100000000000009@lid")
	if a := s.ChatAliases(adaLID.String()); len(a) != 0 {
		t.Errorf("ChatAliases after the mapping moved = %q, want none", a)
	}
	c.emit(pinEvent(adaLID, true))
	for _, e := range chatEvents(events()) {
		if strings.Contains(e, `"aliases"`) || strings.Contains(e, ada.User) {
			t.Errorf("event_chat after the mapping moved lists the old number: %s", e)
		}
	}
}

// Without a LID map, a recorded merge lists nothing: no mapping, no alias.
func TestWithoutAMappingNoAliasIsListed(t *testing.T) {
	db := openTestStore(t)
	if _, err := db.Exec(`INSERT INTO chat_aliases (alias, canonical, merged_at) VALUES (?, ?, 1)`,
		ada.String(), adaLID.String()); err != nil {
		t.Fatal(err)
	}
	s, _, _ := lidSession(t, &fakeClient{device: true}, db)
	if a := s.ChatAliases(adaLID.String()); len(a) != 0 {
		t.Errorf("ChatAliases without a LID map = %q, want none", a)
	}
	unknown, _, _ := lidSession(t, newLIDClient(), db)
	if a := unknown.ChatAliases(adaLID.String()); len(a) != 0 {
		t.Errorf("ChatAliases with the pair unknown = %q, want none", a)
	}
}

// A merge that fails publishes no event_chat and lists no alias, even one a merge of an earlier
// run recorded: the number is its own chat again.
func TestAFailedMergePublishesNothingAndListsNoAlias(t *testing.T) {
	db := openTestStore(t)
	if _, err := db.Exec(`INSERT INTO chat_aliases (alias, canonical, merged_at) VALUES (?, ?, 1)`,
		ada.String(), adaLID.String()); err != nil {
		t.Fatal(err)
	}
	if _, err := store.InsertMessage(db, store.Message{ID: "P1", ChatJID: ada.String(), Kind: store.KindText,
		Text: "lunch?", FromMe: true, Timestamp: t0}); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TRIGGER fail_merge BEFORE DELETE ON chats WHEN old.jid = '` + ada.String() + `'
		BEGIN SELECT RAISE(ABORT, 'injected'); END`); err != nil {
		t.Fatal(err)
	}
	c := newLIDClient(ada.String(), adaLID.String())
	s := NewSession(c, db, nil)
	s.logf = func(string, ...any) {}
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatal(err)
	}
	in := textEvent(adaLID, "L1", false, t0.Add(time.Minute), "yes")
	in.Info.Sender = adaLID
	c.emit(in)
	if got := chatEvents(events()); len(got) != 0 {
		t.Errorf("event_chat after a failed merge = %q, want none", got)
	}
	if a := s.ChatAliases(adaLID.String()); len(a) != 0 {
		t.Errorf("ChatAliases after a failed merge = %q, want none", a)
	}
}
