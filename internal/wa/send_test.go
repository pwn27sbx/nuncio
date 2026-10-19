package wa

import (
	"context"
	"errors"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	waevents "go.mau.fi/whatsmeow/types/events"

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
	_, err := s.SendText(ada.String(), "On my way")
	if !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("SendText while unlinked = %v, want ipc.ErrNotLinked", err)
	}
	if len(fake.sends) != 0 {
		t.Errorf("client sends = %q, want none", fake.sends)
	}
	if got := storedMessages(t, s, ada.String()); len(got) != 0 {
		t.Errorf("stored %+v, want nothing", got)
	}
	assertEvents(t, events())
}

func TestSendWithoutAnyDeviceStoreIsNotLinked(t *testing.T) {
	s := NewSession(nil, nil, nil)
	if _, err := s.SendText(ada.String(), "On my way"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("SendText without a device store = %v, want ipc.ErrNotLinked", err)
	}
}

// Until the outbox (T11), an offline send is refused as not sent; it is never reported queued.
func TestSendWhileOfflineIsNotSentAndSendsNothing(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.Disconnected{})
	events := eventsOf(t, s)
	_, err := s.SendText(ada.String(), "On my way")
	if !errors.Is(err, ErrOffline) {
		t.Errorf("SendText while offline = %v, want ErrOffline", err)
	}
	if len(fake.sends) != 0 {
		t.Errorf("client sends = %q, want none", fake.sends)
	}
	if got := storedMessages(t, s, ada.String()); len(got) != 0 {
		t.Errorf("stored %+v, want nothing", got)
	}
	assertEvents(t, events())
}

func TestSendWhileLinkedSendsOnceStoresAndPublishes(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.sendID = "3EB0BBB1"
	fake.sendTime = time.Date(2026, 10, 18, 17, 56, 10, 0, time.UTC)

	got, err := s.SendText(ada.String(), "On my way")
	if err != nil {
		t.Fatalf("SendText: %v", err)
	}
	want := ipc.Message{ID: "3EB0BBB1", Kind: ipc.KindText, Text: "On my way", FromMe: true, Timestamp: "2026-10-18T17:56:10Z"}
	if got != want {
		t.Errorf("SendText = %+v, want %+v", got, want)
	}
	if want := []string{"15551230001@s.whatsapp.net On my way"}; len(fake.sends) != 1 || fake.sends[0] != want[0] {
		t.Errorf("client sends = %q, want %q", fake.sends, want)
	}
	stored := storedMessages(t, s, ada.String())
	if len(stored) != 1 || stored[0].ID != "3EB0BBB1" || !stored[0].FromMe || stored[0].Text != "On my way" {
		t.Errorf("stored %+v, want the sent message from me", stored)
	}
	if n := unread(t, s.Store(), ada.String()); n != 0 {
		t.Errorf("unread = %d, want 0: my own message is never unread", n)
	}
	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"3EB0BBB1","kind":"text","text":"On my way","from_me":true,"timestamp":"2026-10-18T17:56:10Z"}}`)
}

func TestAFailedSendIsNotSentAndStoresNothing(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.sendErr = errors.New("websocket closed for 15551230001@s.whatsapp.net")

	_, err := s.SendText(ada.String(), "On my way")
	if !errors.Is(err, ErrNotSent) {
		t.Fatalf("SendText after a client failure = %v, want ErrNotSent", err)
	}
	// The reason that reaches clients never carries whatsmeow's text, which may name a number.
	if got := err.Error(); got != ErrNotSent.Error() {
		t.Errorf("error = %q, want exactly %q", got, ErrNotSent.Error())
	}
	if got := storedMessages(t, s, ada.String()); len(got) != 0 {
		t.Errorf("stored %+v, want nothing", got)
	}
	assertEvents(t, events())
}

// whatsmeow writes the frame before it waits for the ack, so a timeout means the message may
// have reached WhatsApp: it is never reported as not accepted, and nothing is stored.
func TestAnAckTimeoutIsNotReportedAsNotAccepted(t *testing.T) {
	for _, e := range []error{context.DeadlineExceeded, whatsmeow.ErrMessageTimedOut} {
		s, fake, events := subscribedSession(t)
		fake.sendErr = e
		_, err := s.SendText(ada.String(), "On my way")
		if errors.Is(err, ErrNotSent) {
			t.Errorf("client err %v reported as %q; the frame may have reached WhatsApp", e, err)
		}
		if !errors.Is(err, ipc.ErrNotConfirmed) {
			t.Errorf("client err %v reported as %v, want ipc.ErrNotConfirmed", e, err)
		}
		if got := storedMessages(t, s, ada.String()); len(got) != 0 {
			t.Errorf("client err %v: stored %+v, want nothing", e, got)
		}
		assertEvents(t, events())
	}
}

// An ack without a timestamp still stores a usable time, never the zero time.
func TestASentMessageWithoutAnAckTimestampGetsTheCurrentTime(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	fake.sendID = "3EB0BBB2"
	before := time.Now().UTC().Truncate(time.Second)
	got, err := s.SendText(ada.String(), "On my way")
	if err != nil {
		t.Fatalf("SendText: %v", err)
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
