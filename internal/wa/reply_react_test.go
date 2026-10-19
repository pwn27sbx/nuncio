package wa

import (
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

var group = types.NewJID("120363000000000000", types.GroupServer)

// storeIncoming stores one incoming text of Ada in chat as if it had arrived.
func storeIncoming(t *testing.T, fake *fakeClient, chat types.JID, id, text string) {
	t.Helper()
	evt := textEvent(chat, id, false, t0, text)
	evt.Info.Sender = ada
	fake.emit(evt)
}

func TestAReplyQuotesTheMessageTheWayWhatsAppExpects(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	storeIncoming(t, fake, group, "Q1", "Which venue?")
	fake.sendID = "3EB0R1"

	got, err := s.SendReply(group.String(), "The usual one", "c-1", "Q1")
	if err != nil || got.State != ipc.SendSent {
		t.Fatalf("SendReply = %+v, %v; want sent", got, err)
	}
	msgs := fake.sentMessages()
	if len(msgs) != 1 {
		t.Fatalf("sent %d messages, want 1", len(msgs))
	}
	ext := msgs[0].GetExtendedTextMessage()
	ci := ext.GetContextInfo()
	if ext.GetText() != "The usual one" || ci.GetStanzaID() != "Q1" || ci.GetParticipant() != ada.String() ||
		ci.GetQuotedMessage().GetConversation() != "Which venue?" {
		t.Errorf("sent %v, want an extended text quoting Q1 by Ada", msgs[0])
	}
	if msgs[0].Conversation != nil {
		t.Errorf("a reply must not also carry a plain conversation: %v", msgs[0])
	}
	stored, err := store.GetMessage(s.Store(), group.String(), "3EB0R1")
	if err != nil || stored.ReplyTo != "Q1" {
		t.Errorf("stored reply = %+v, %v; want reply_to Q1", stored, err)
	}
}

func TestAReplyToMyOwnMessageNamesMeAsTheParticipant(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	fake.emit(textEvent(ada, "M1", true, t0, "lunch?"))
	if _, err := s.SendReply(ada.String(), "or dinner", "c-1", "M1"); err != nil {
		t.Fatalf("SendReply: %v", err)
	}
	ci := fake.sentMessages()[0].GetExtendedTextMessage().GetContextInfo()
	if ci.GetStanzaID() != "M1" || ci.GetParticipant() != fakeOwnJID {
		t.Errorf("context info = %v, want M1 by me", ci)
	}
}

func TestAReplyToAnUnknownMessageIsRefusedAndNothingIsStored(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	_, err := s.SendReply(ada.String(), "hi", "c-1", "NOPE")
	if !errors.Is(err, ErrNoQuote) {
		t.Errorf("SendReply to an unknown id = %v, want ErrNoQuote", err)
	}
	if sends, _ := fake.sent(); len(sends) != 0 {
		t.Errorf("sends = %q, want none", sends)
	}
	if _, err := store.GetOutbox(s.Store(), "c-1"); !errors.Is(err, store.ErrNoOutboxEntry) {
		t.Errorf("outbox c-1 = %v, want nothing queued", err)
	}
}

func TestAQueuedReplyStillQuotesWhenTheDrainSendsIt(t *testing.T) {
	s, fake, events, _ := offlineSession(t)
	storeIncoming(t, fake, ada, "Q1", "Which venue?")
	events()
	got, err := s.SendReply(ada.String(), "The usual one", "c-1", "Q1")
	if err != nil || got.State != ipc.SendQueued {
		t.Fatalf("SendReply offline = %+v, %v; want queued", got, err)
	}
	if !jsonHas(events()[0], `"state":"queued"`, `"reply_to":"Q1"`) {
		t.Errorf("queued event lacks its reply_to")
	}
	reconnect(s, fake)
	msgs := fake.sentMessages()
	if len(msgs) != 1 || msgs[0].GetExtendedTextMessage().GetContextInfo().GetStanzaID() != "Q1" {
		t.Errorf("drained %v, want one send quoting Q1", msgs)
	}
}

// A send without reply_to is the M1 plain conversation, with no context info.
func TestAPlainSendIsStillAPlainConversation(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	if _, err := s.Send(ada.String(), "On my way", "c-1"); err != nil {
		t.Fatalf("Send: %v", err)
	}
	msgs := fake.sentMessages()
	if len(msgs) != 1 || msgs[0].GetConversation() != "On my way" || msgs[0].ExtendedTextMessage != nil {
		t.Errorf("sent %v, want a plain conversation", msgs)
	}
}

func TestAReactionIsBuiltForTheTargetSentAndKept(t *testing.T) {
	s, fake, events := subscribedSession(t)
	storeIncoming(t, fake, group, "Q1", "Which venue?")
	events()

	if err := s.React(group.String(), "Q1", "👍"); err != nil {
		t.Fatalf("React: %v", err)
	}
	if got, want := fake.builtReactions, []string{group.String() + " " + ada.String() + " Q1 👍"}; !reflect.DeepEqual(got, want) {
		t.Errorf("BuildReaction calls = %q, want %q", got, want)
	}
	msgs := fake.sentMessages()
	if len(msgs) != 1 || msgs[0].GetReactionMessage().GetText() != "👍" ||
		msgs[0].GetReactionMessage().GetKey().GetID() != "Q1" {
		t.Fatalf("sent %v, want the reaction on Q1", msgs)
	}
	m, _ := store.GetMessage(s.Store(), group.String(), "Q1")
	if !reflect.DeepEqual(m.Reactions, []store.Reaction{{Emoji: "👍", FromMe: true}}) {
		t.Errorf("reactions = %+v, want mine", m.Reactions)
	}
	if ev := events(); len(ev) != 1 || !jsonHas(ev[0], `"id":"Q1"`, `"reactions":[{"emoji":"👍","from_me":true}]`) {
		t.Errorf("events = %q, want Q1 republished with my reaction", ev)
	}

	if err := s.React(group.String(), "Q1", ""); err != nil {
		t.Fatalf("React remove: %v", err)
	}
	m, _ = store.GetMessage(s.Store(), group.String(), "Q1")
	if len(m.Reactions) != 0 {
		t.Errorf("after removal reactions = %+v, want none", m.Reactions)
	}
}

func TestAReactionIsNeverQueued(t *testing.T) {
	s, fake, _, _ := offlineSession(t)
	storeIncoming(t, fake, ada, "Q1", "hi")
	if err := s.React(ada.String(), "Q1", "👍"); !errors.Is(err, ErrReactOffline) {
		t.Errorf("React offline = %v, want ErrReactOffline", err)
	}
	reconnect(s, fake)
	if msgs := fake.sentMessages(); len(msgs) != 0 {
		t.Errorf("sent %v, want nothing, not even after the reconnect", msgs)
	}
}

func TestAReactionNeedsALinkedDeviceAndAKnownMessage(t *testing.T) {
	unlinked := NewSession(&fakeClient{}, openTestStore(t), nil)
	if err := unlinked.React(ada.String(), "Q1", "👍"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("React unlinked = %v, want ipc.ErrNotLinked", err)
	}
	s, fake, _ := subscribedSession(t)
	if err := s.React(ada.String(), "NOPE", "👍"); !errors.Is(err, ErrNoQuote) {
		t.Errorf("React on an unknown id = %v, want ErrNoQuote", err)
	}
	if msgs := fake.sentMessages(); len(msgs) != 0 {
		t.Errorf("sent %v, want nothing", msgs)
	}
}

func TestAnIncomingReactionIsStoredOnItsTargetNotAsAMessage(t *testing.T) {
	s, fake, events := subscribedSession(t)
	db := s.Store()
	storeIncoming(t, fake, ada, "Q1", "hello")
	events()
	reaction := func(emoji string) {
		evt := messageEvent(ada, "R"+emoji, false, t0.Add(time.Minute), &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{
			Key:  &waCommon.MessageKey{ID: proto.String("Q1"), RemoteJID: proto.String(ada.String()), FromMe: proto.Bool(false)},
			Text: proto.String(emoji),
		}})
		fake.emit(evt)
	}
	reaction("😂")
	msgs, _ := store.ListMessages(db, ada.String(), 10)
	if len(msgs) != 1 || !reflect.DeepEqual(msgs[0].Reactions, []store.Reaction{{Emoji: "😂"}}) {
		t.Errorf("messages = %+v, want only Q1 with Ada's reaction", msgs)
	}
	if n := unread(t, db, ada.String()); n != 1 {
		t.Errorf("unread = %d, want 1: a reaction is not an unread message", n)
	}
	if ev := events(); len(ev) != 1 || !jsonHas(ev[0], `"id":"Q1"`, `"reactions":[{"emoji":"😂","from_me":false}]`) {
		t.Errorf("events = %q, want Q1 republished with the reaction", ev)
	}
	reaction("")
	msgs, _ = store.ListMessages(db, ada.String(), 10)
	if len(msgs[0].Reactions) != 0 {
		t.Errorf("after removal reactions = %+v, want none", msgs[0].Reactions)
	}
}

func TestAnIncomingQuoteRecordsWhatItQuotes(t *testing.T) {
	s, fake, events := subscribedSession(t)
	db := s.Store()
	fake.emit(messageEvent(ada, "A2", false, t0, &waE2E.Message{ExtendedTextMessage: &waE2E.ExtendedTextMessage{
		Text:        proto.String("yes"),
		ContextInfo: &waE2E.ContextInfo{StanzaID: proto.String("M1"), Participant: proto.String(fakeOwnJID)},
	}}))
	m := onlyMessage(t, db, ada.String())
	if m.ReplyTo != "M1" || m.Text != "yes" {
		t.Errorf("stored %+v, want the text with reply_to M1", m)
	}
	if ev := events(); len(ev) == 0 || !jsonHas(ev[0], `"reply_to":"M1"`) {
		t.Errorf("events = %q, want the message with its reply_to", ev)
	}
}

// A group message stored before senders were recorded (M1 data) has no sender: reacting to it
// must not build a key that claims the message is mine.
func TestAReactionOnAGroupMessageWithAnUnknownSenderIsRefused(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	if _, err := store.InsertMessage(s.Store(), store.Message{ID: "OLD1", ChatJID: group.String(),
		Kind: store.KindText, Text: "old", Timestamp: t0}); err != nil {
		t.Fatal(err)
	}
	if err := s.React(group.String(), "OLD1", "👍"); err == nil {
		t.Errorf("React on a group message with no recorded sender succeeded; want an error")
	}
	if msgs := fake.sentMessages(); len(msgs) != 0 {
		t.Errorf("sent %v, want nothing", msgs)
	}
	if m, _ := store.GetMessage(s.Store(), group.String(), "OLD1"); len(m.Reactions) != 0 {
		t.Errorf("reactions = %+v, want none stored", m.Reactions)
	}
}

// whatsmeow's ParseJID error can quote part of the JID, and a React error reaches the client as an
// internal error, so the adapter's error names no JID. Both checks fail before the whatsmeow
// client is used.
func TestBuildReactionErrorsCarryNoJID(t *testing.T) {
	const bad = "15551230009:15551230008x@s.whatsapp.net"
	for _, tc := range []struct{ chat, sender string }{
		{bad, ""},
		{group.String(), bad},
	} {
		_, err := whatsmeowClient{}.BuildReaction(tc.chat, tc.sender, "Q1", "👍")
		if err == nil {
			t.Fatalf("BuildReaction(%q, %q) succeeded, want an error", tc.chat, tc.sender)
		}
		if strings.Contains(err.Error(), "1555123000") {
			t.Errorf("BuildReaction(%q, %q) error %q carries the JID", tc.chat, tc.sender, err)
		}
	}
}
