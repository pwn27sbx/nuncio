package wa

import (
	"context"
	"errors"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

func storedMessages(t *testing.T, s *Session, chat string) []store.Message {
	t.Helper()
	if s.Store() == nil {
		return nil
	}
	msgs, err := store.ListMessages(s.Store(), chat, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	return msgs
}

func TestSendWhileUnlinkedIsNotLinkedAndSendsNothing(t *testing.T) {
	fake := &fakeClient{device: false}
	s := NewSession(fake, openTestStore(t), nil)
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	_, err := s.Send(ada.String(), "On my way", "c-1")
	if !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("Send while unlinked = %v, want ipc.ErrNotLinked", err)
	}
	if sends, _ := fake.sent(); len(sends) != 0 {
		t.Errorf("client sends = %q, want none", sends)
	}
	if got := storedMessages(t, s, ada.String()); len(got) != 0 {
		t.Errorf("stored %+v, want nothing", got)
	}
	// Nothing is queued either: an unlinked session has nowhere to send it.
	if _, err := store.GetOutbox(s.Store(), "c-1"); !errors.Is(err, store.ErrNoOutboxEntry) {
		t.Errorf("outbox entry of c-1 = %v, want none", err)
	}
	assertEvents(t, events())
}

func TestSendWithoutAnyDeviceStoreIsNotLinked(t *testing.T) {
	s := NewSession(nil, nil, nil)
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("Send without a device store = %v, want ipc.ErrNotLinked", err)
	}
	if _, err := s.Retry("c-1"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("Retry without a device store = %v, want ipc.ErrNotLinked", err)
	}
}

func TestSendWhileLinkedSendsOnceStoresAndPublishes(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.sendID = "3EB0BBB1"
	fake.sendTime = time.Date(2026, 10, 18, 17, 56, 10, 0, time.UTC)

	got, err := s.Send(ada.String(), "On my way", "c-1")
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	want := ipc.Sent{State: ipc.SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
	if got != want {
		t.Errorf("Send = %+v, want %+v", got, want)
	}
	sends, ids := fake.sent()
	if len(sends) != 1 || sends[0] != "15551230001@s.whatsapp.net On my way" || ids[0] != "3EB0BBB1" {
		t.Errorf("client sends = %q with ids %q, want one send with the id chosen up front", sends, ids)
	}
	stored := storedMessages(t, s, ada.String())
	if len(stored) != 1 || stored[0].ID != "3EB0BBB1" || !stored[0].FromMe || stored[0].Text != "On my way" || stored[0].State != "" {
		t.Errorf("stored %+v, want the sent message from me", stored)
	}
	if n := unread(t, s.Store(), ada.String()); n != 0 {
		t.Errorf("unread = %d, want 0: my own message is never unread", n)
	}
	// A linked send goes through the outbox too, so it has a durable state.
	if e, err := store.GetOutbox(s.Store(), "c-1"); err != nil || e.State != store.OutboxSent || e.MessageID != "3EB0BBB1" {
		t.Errorf("outbox entry = %+v, %v; want it sent under its WhatsApp id", e, err)
	}
	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"3EB0BBB1","kind":"text","text":"On my way","from_me":true,"timestamp":"2026-10-18T17:56:10Z"}}`)
}

func TestAFailedSendIsNotSentAndStaysVisible(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.sendID = "3EB0BBB3"
	fake.sendErr = errors.New("websocket closed for 15551230001@s.whatsapp.net")

	_, err := s.Send(ada.String(), "On my way", "c-7f3a9e23")
	if !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send after a client failure = %v, want ErrNotSent", err)
	}
	// The reason that reaches clients never carries whatsmeow's text, which may name a number.
	if got := err.Error(); got != ErrNotSent.Error() {
		t.Errorf("error = %q, want exactly %q", got, ErrNotSent.Error())
	}
	stored := storedMessages(t, s, ada.String())
	if len(stored) != 1 || stored[0].State != store.OutboxNotSent || stored[0].ClientID != "c-7f3a9e23" || stored[0].ID != "3EB0BBB3" {
		t.Errorf("listed %+v, want only the not-sent message with its client_id", stored)
	}
	events1 := events()
	if len(events1) != 1 || !jsonHas(events1[0], `"state":"not_sent"`, `"client_id":"c-7f3a9e23"`, `"id":"3EB0BBB3"`) {
		t.Errorf("events = %q, want one event_message of the not-sent message", events1)
	}
}

// whatsmeow writes the frame before it waits for the ack, so a timeout means the message may
// have reached WhatsApp: it is never reported as not accepted. It stays visible as not sent, for a
// manual retry under the same WhatsApp id.
func TestAnAckTimeoutIsNotReportedAsNotAccepted(t *testing.T) {
	for _, e := range []error{context.DeadlineExceeded, whatsmeow.ErrMessageTimedOut} {
		s, fake, _ := subscribedSession(t)
		fake.sendErr = e
		_, err := s.Send(ada.String(), "On my way", "c-1")
		if errors.Is(err, ErrNotSent) {
			t.Errorf("client err %v reported as %q; the frame may have reached WhatsApp", e, err)
		}
		if !errors.Is(err, ipc.ErrNotConfirmed) {
			t.Errorf("client err %v reported as %v, want ipc.ErrNotConfirmed", e, err)
		}
		if got := storedMessages(t, s, ada.String()); len(got) != 1 || got[0].State != store.OutboxNotConfirmed || WireMessage(got[0]).State != ipc.MessageNotSent {
			t.Errorf("client err %v: listed %+v, want the message not sent", e, got)
		}
	}
}

// An ack without a timestamp still stores a usable time, never the zero time.
func TestASentMessageWithoutAnAckTimestampGetsTheCurrentTime(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	fake.sendID = "3EB0BBB2"
	before := time.Now().UTC().Truncate(time.Second)
	got, err := s.Send(ada.String(), "On my way", "c-1")
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	ts, err := time.Parse(ipc.TimestampFormat, got.Timestamp)
	if err != nil || ts.Before(before) || ts.After(time.Now().Add(time.Second)) {
		t.Errorf("timestamp = %q, %v; want about now", got.Timestamp, err)
	}
	stored := storedMessages(t, s, ada.String())
	if len(stored) != 1 || stored[0].Timestamp.Before(before) {
		t.Errorf("stored %+v, want the current time", stored)
	}
}
