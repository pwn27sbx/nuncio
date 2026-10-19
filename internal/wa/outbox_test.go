package wa

import (
	"errors"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// jsonHas reports whether the JSON line contains every fragment.
func jsonHas(line string, fragments ...string) bool {
	for _, f := range fragments {
		if !strings.Contains(line, f) {
			return false
		}
	}
	return true
}

// offlineSession is a linked session over a real store that has just dropped its connection.
// Reconnects run on fake timers, so nothing reconnects unless the test emits Connected.
func offlineSession(t *testing.T) (*Session, *fakeClient, func() []string, *fakeTimers) {
	t.Helper()
	fake := &fakeClient{device: true}
	s := NewSession(fake, openTestStore(t), nil)
	timers := &fakeTimers{}
	s.after = timers.after
	s.logf = t.Logf
	events := eventsOf(t, s)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.Disconnected{})
	s.drains.Wait()
	events()
	return s, fake, events, timers
}

// reconnect delivers a Connected event and waits for the drain it starts.
func reconnect(s *Session, fake *fakeClient) {
	fake.emit(&waevents.Connected{})
	s.drains.Wait()
}

func queue(t *testing.T, s *Session, clientID, text string) {
	t.Helper()
	got, err := s.Send(ada.String(), text, clientID)
	if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
		t.Fatalf("Send %s while offline = %+v, %v; want queued", clientID, got, err)
	}
}

func outboxState(t *testing.T, s *Session, clientID string) store.OutboxEntry {
	t.Helper()
	e, err := store.GetOutbox(s.Store(), clientID)
	if err != nil {
		t.Fatalf("GetOutbox %s: %v", clientID, err)
	}
	return e
}

func TestAnOfflineSendIsQueuedAndSendsNothing(t *testing.T) {
	s, fake, events, _ := offlineSession(t)
	got, err := s.Send(ada.String(), "On my way", "c-7f3a9e22")
	if err != nil {
		t.Fatalf("Send while offline: %v", err)
	}
	if want := (ipc.Sent{State: ipc.SendQueued}); got != want {
		t.Errorf("Send while offline = %+v, want %+v", got, want)
	}
	if sends, _ := fake.sent(); len(sends) != 0 {
		t.Errorf("client sends = %q, want none while offline", sends)
	}
	listed := storedMessages(t, s, ada.String())
	if len(listed) != 1 || listed[0].State != store.OutboxQueued || listed[0].ClientID != "c-7f3a9e22" || !listed[0].FromMe {
		t.Errorf("listed %+v, want the queued message", listed)
	}
	ev := events()
	if len(ev) != 1 || !jsonHas(ev[0], `"op":"event_message"`, `"state":"queued"`, `"client_id":"c-7f3a9e22"`) {
		t.Errorf("events = %q, want one event_message of the queued message", ev)
	}
}

func TestReconnectSendsEachQueuedMessageOnceInOrder(t *testing.T) {
	s, fake, events, _ := offlineSession(t)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")
	queue(t, s, "c-3", "three")
	queuedIDs := []string{outboxState(t, s, "c-1").MessageID, outboxState(t, s, "c-2").MessageID, outboxState(t, s, "c-3").MessageID}
	events()

	reconnect(s, fake)

	sends, ids := fake.sent()
	wantSends := []string{ada.String() + " one", ada.String() + " two", ada.String() + " three"}
	if fmt.Sprint(sends) != fmt.Sprint(wantSends) {
		t.Fatalf("client sends = %q, want %q: one each, in the order they were queued", sends, wantSends)
	}
	if fmt.Sprint(ids) != fmt.Sprint(queuedIDs) {
		t.Errorf("sent ids = %q, want the ids chosen when queued %q", ids, queuedIDs)
	}
	seen := map[string]bool{}
	for _, id := range ids {
		if seen[id] {
			t.Errorf("WhatsApp id %s sent twice", id)
		}
		seen[id] = true
	}
	for _, c := range []string{"c-1", "c-2", "c-3"} {
		if e := outboxState(t, s, c); e.State != store.OutboxSent {
			t.Errorf("%s = %s, want sent", c, e.State)
		}
	}
	listed := storedMessages(t, s, ada.String())
	if len(listed) != 3 {
		t.Fatalf("listed %+v, want the three sent messages, once each", listed)
	}
	for _, m := range listed {
		if m.State != "" {
			t.Errorf("listed %+v, want it sent", m)
		}
	}
	var published []string
	for _, e := range events() {
		if strings.Contains(e, `"op":"event_message"`) {
			if strings.Contains(e, `"state"`) {
				t.Errorf("event %s, want a sent message", e)
			}
			published = append(published, e)
		}
	}
	if len(published) != 3 || !strings.Contains(published[0], `"text":"one"`) || !strings.Contains(published[2], `"text":"three"`) {
		t.Errorf("published %q, want the three sent messages once each, in order", published)
	}
}

func TestTwoConnectedEventsStillSendEachQueuedMessageOnce(t *testing.T) {
	s, fake, _, _ := offlineSession(t)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")

	var wg sync.WaitGroup
	for range 2 {
		wg.Add(1)
		go func() { defer wg.Done(); fake.emit(&waevents.Connected{}) }()
	}
	wg.Wait()
	s.drains.Wait()
	reconnect(s, fake)

	if sends, _ := fake.sent(); len(sends) != 2 {
		t.Errorf("client sends = %q, want exactly one per queued message", sends)
	}
}

func TestAFailureMidDrainEndsNotSentAndIsNeverResentOnItsOwn(t *testing.T) {
	for _, tc := range []struct {
		failure error
		stored  string // the outbox keeps why it failed; the wire says not_sent for both
	}{
		{errors.New("websocket closed"), store.OutboxNotSent},
		{whatsmeow.ErrMessageTimedOut, store.OutboxNotConfirmed},
	} {
		failure := tc.failure
		t.Run(failure.Error(), func(t *testing.T) {
			s, fake, events, _ := offlineSession(t)
			queue(t, s, "c-1", "one")
			queue(t, s, "c-2", "two")
			queue(t, s, "c-3", "three")
			failedID := outboxState(t, s, "c-2").MessageID
			events()
			fake.sendErrs = []error{nil, failure, nil}

			reconnect(s, fake)

			if sends, _ := fake.sent(); len(sends) != 3 {
				t.Fatalf("client sends = %q, want one attempt per queued message", sends)
			}
			for c, want := range map[string]string{"c-1": store.OutboxSent, "c-2": tc.stored, "c-3": store.OutboxSent} {
				if e := outboxState(t, s, c); e.State != want {
					t.Errorf("%s = %s, want %s", c, e.State, want)
				}
			}
			var notSentEvents int
			for _, e := range events() {
				if jsonHas(e, `"state":"not_sent"`, `"client_id":"c-2"`, `"id":"`+failedID+`"`) {
					notSentEvents++
				}
			}
			if notSentEvents != 1 {
				t.Errorf("not-sent events of c-2 = %d, want 1", notSentEvents)
			}

			// The next reconnect resends nothing: a failed send waits for a manual retry.
			fake.emit(&waevents.Disconnected{})
			reconnect(s, fake)
			if sends, _ := fake.sent(); len(sends) != 3 {
				t.Errorf("client sends after the next reconnect = %q, want no resend", sends)
			}
			listed := storedMessages(t, s, ada.String())
			var stillListed bool
			for _, m := range listed {
				if m.ClientID == "c-2" && WireMessage(m).State == ipc.MessageNotSent && m.ID == failedID {
					stillListed = true
				}
			}
			if len(listed) != 3 || !stillListed {
				t.Errorf("listed %+v, want the two sent messages and c-2 still listed as not sent", listed)
			}
		})
	}
}

func TestARetryOfANotSentMessageSendsOnceWithTheSameWhatsAppID(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.sendErr = errors.New("websocket closed")
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send = %v, want ErrNotSent", err)
	}
	firstID := outboxState(t, s, "c-1").MessageID
	fake.sendErr = nil
	events()

	got, err := s.Retry("c-1")
	if err != nil {
		t.Fatalf("Retry: %v", err)
	}
	if got.State != ipc.SendSent || got.ID != firstID {
		t.Errorf("Retry = %+v, want sent under the first WhatsApp id %s", got, firstID)
	}
	sends, ids := fake.sent()
	if len(sends) != 2 || ids[1] != firstID {
		t.Errorf("client sends = %q with ids %q, want one more send under %s", sends, ids, firstID)
	}
	if listed := storedMessages(t, s, ada.String()); len(listed) != 1 || listed[0].ID != firstID || listed[0].State != "" {
		t.Errorf("listed %+v, want the message once, sent", listed)
	}
	if ev := events(); len(ev) != 1 || strings.Contains(ev[0], `"state"`) {
		t.Errorf("events = %q, want the sent message once", ev)
	}

	// A retry of a sent message reports it and sends nothing.
	again, err := s.Retry("c-1")
	if err != nil || again != got {
		t.Errorf("second Retry = %+v, %v; want %+v", again, err, got)
	}
	if sends, _ := fake.sent(); len(sends) != 2 {
		t.Errorf("client sends after retrying a sent message = %q, want no new send", sends)
	}
}

func TestARetryWhileOfflineQueuesItForTheNextReconnect(t *testing.T) {
	s, fake, _, _ := offlineSession(t)
	queue(t, s, "c-1", "one")
	fake.sendErrs = []error{errors.New("websocket closed")}
	reconnect(s, fake)
	firstID := outboxState(t, s, "c-1").MessageID
	fake.emit(&waevents.Disconnected{})

	got, err := s.Retry("c-1")
	if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
		t.Fatalf("Retry while offline = %+v, %v; want queued", got, err)
	}
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Errorf("client sends = %q, want nothing new while offline", sends)
	}
	reconnect(s, fake)
	sends, ids := fake.sent()
	if len(sends) != 2 || ids[1] != firstID || outboxState(t, s, "c-1").State != store.OutboxSent {
		t.Errorf("after reconnect: sends %q ids %q, want one more send under %s, now sent", sends, ids, firstID)
	}
}

func TestARetryOfAnUnknownSendSendsNothing(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	if _, err := s.Retry("c-unknown"); !errors.Is(err, ErrNoSend) {
		t.Errorf("Retry of an unknown client_id = %v, want ErrNoSend", err)
	}
	if sends, _ := fake.sent(); len(sends) != 0 {
		t.Errorf("client sends = %q, want none", sends)
	}
}

func TestARetryWhileUnlinkedIsNotLinked(t *testing.T) {
	fake := &fakeClient{device: false}
	s := NewSession(fake, openTestStore(t), nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if _, err := s.Retry("c-1"); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("Retry while unlinked = %v, want ipc.ErrNotLinked", err)
	}
}

// A send repeating a client_id already accepted answers that send's stored state and never
// becomes a second message, whatever text or chat it carries.
func TestARepeatedClientIDReportsTheStoredStateAndSendsNothing(t *testing.T) {
	t.Run("queued", func(t *testing.T) {
		s, fake, _, _ := offlineSession(t)
		queue(t, s, "c-1", "one")
		queue(t, s, "c-1", "something else")
		if q, err := store.QueuedOutbox(s.Store()); err != nil || len(q) != 1 || q[0].Text != "one" {
			t.Errorf("queued = %+v, %v; want the first send only", q, err)
		}
		reconnect(s, fake)
		if sends, _ := fake.sent(); len(sends) != 1 {
			t.Errorf("client sends = %q, want one", sends)
		}
	})
	t.Run("sent", func(t *testing.T) {
		s, fake, _ := subscribedSession(t)
		first, err := s.Send(ada.String(), "one", "c-1")
		if err != nil {
			t.Fatalf("Send: %v", err)
		}
		again, err := s.Send(grace.String(), "two", "c-1")
		if err != nil || again != first {
			t.Errorf("repeated Send = %+v, %v; want the stored %+v", again, err, first)
		}
		if sends, _ := fake.sent(); len(sends) != 1 {
			t.Errorf("client sends = %q, want one", sends)
		}
	})
	t.Run("not sent", func(t *testing.T) {
		s, fake, _ := subscribedSession(t)
		fake.sendErr = errors.New("websocket closed")
		if _, err := s.Send(ada.String(), "one", "c-1"); !errors.Is(err, ErrNotSent) {
			t.Fatalf("Send = %v, want ErrNotSent", err)
		}
		fake.sendErr = nil
		if _, err := s.Send(ada.String(), "one", "c-1"); !errors.Is(err, ErrNotSent) {
			t.Errorf("repeated Send of a not-sent client_id = %v, want ErrNotSent: only a retry sends it again", err)
		}
		if sends, _ := fake.sent(); len(sends) != 1 {
			t.Errorf("client sends = %q, want one", sends)
		}
	})
}

// The outbox never schedules anything: it drains on a Connected event and on a manual retry
// only, so the only timer is the reconnect one a drop schedules.
func TestTheOutboxNeverSchedulesATimer(t *testing.T) {
	s, fake, _, timers := offlineSession(t)
	before := len(timers.delays)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")
	fake.sendErrs = []error{errors.New("websocket closed"), whatsmeow.ErrMessageTimedOut}
	reconnect(s, fake)
	if _, err := s.Retry("c-1"); err != nil {
		t.Fatalf("Retry: %v", err)
	}
	if got := len(timers.delays); got != before {
		t.Errorf("timers scheduled by the outbox = %d, want 0", got-before)
	}
}

// A send whose ack timed out stays not confirmed: repeating its client_id answers
// ipc.ErrNotConfirmed again (never "not accepted", which would invite a resend with a new id) and
// sends nothing; it is listed as not sent, and a retry sends it under the same id.
func TestARepeatedSendOfANotConfirmedClientIDStaysNotConfirmed(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	fake.sendErr = whatsmeow.ErrMessageTimedOut
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotConfirmed) {
		t.Fatalf("Send = %v, want ipc.ErrNotConfirmed", err)
	}
	fake.sendErr = nil
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotConfirmed) {
		t.Errorf("repeated Send of a not-confirmed client_id = %v, want ipc.ErrNotConfirmed", err)
	}
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Errorf("client sends = %q, want one", sends)
	}
	listed := storedMessages(t, s, ada.String())
	if len(listed) != 1 || WireMessage(listed[0]).State != ipc.MessageNotSent {
		t.Errorf("listed %+v, want the message listed as not sent on the wire", listed)
	}
	firstID := outboxState(t, s, "c-1").MessageID
	got, err := s.Retry("c-1")
	if err != nil || got.State != ipc.SendSent || got.ID != firstID {
		t.Errorf("Retry = %+v, %v; want sent under %s", got, err, firstID)
	}
}

// A linked send that races a reconnect's drain may be attempted by the drain; when that attempt is
// not acknowledged, the send is still answered ipc.ErrNotConfirmed, never ErrNotSent, and it is
// attempted once. The race is repeated so both orders of the two occur.
func TestASendRacingTheDrainReportsNotConfirmed(t *testing.T) {
	for i := range 300 {
		fake := &fakeClient{device: true, sendErr: whatsmeow.ErrMessageTimedOut}
		s := NewSession(fake, openTestStore(t), nil)
		s.after = (&fakeTimers{}).after
		s.logf = t.Logf
		if err := s.Start(); err != nil {
			t.Fatalf("Start: %v", err)
		}
		s.drains.Wait()
		var wg sync.WaitGroup
		wg.Add(1)
		go func() { defer wg.Done(); fake.emit(&waevents.Connected{}) }()
		_, err := s.Send(ada.String(), "On my way", "c-1")
		wg.Wait()
		s.drains.Wait()
		if !errors.Is(err, ipc.ErrNotConfirmed) {
			t.Fatalf("run %d: Send whose ack timed out = %v, want ipc.ErrNotConfirmed", i, err)
		}
		if sends, _ := fake.sent(); len(sends) != 1 {
			t.Fatalf("run %d: client sends = %q, want one attempt", i, sends)
		}
		s.Close()
	}
}

// A send made while a drain runs never overtakes the messages queued before it: they go first,
// in order, and each is attempted once.
func TestASendDuringADrainGoesAfterEveryOlderQueuedMessage(t *testing.T) {
	s, fake, _, _ := offlineSession(t)
	for i := range 5 {
		queue(t, s, fmt.Sprintf("c-%d", i), fmt.Sprintf("q%d", i))
	}
	release := make(chan struct{})
	newSent := make(chan error, 1)
	var once sync.Once
	fake.onSend = func(text string) {
		if text != "q0" {
			return
		}
		once.Do(func() {
			// While the drain is inside its first send, a new linked send arrives.
			go func() {
				_, err := s.Send(ada.String(), "NEW", "c-new")
				newSent <- err
			}()
			for {
				if _, err := store.GetOutbox(s.Store(), "c-new"); err == nil {
					break
				}
				time.Sleep(time.Millisecond)
			}
			// Let the new send wait on the outbox long enough to be first in line for it.
			time.Sleep(5 * time.Millisecond)
			close(release)
		})
		<-release
	}
	reconnect(s, fake)
	if err := <-newSent; err != nil {
		t.Fatalf("Send during the drain: %v", err)
	}
	s.drains.Wait()
	sends, ids := fake.sent()
	want := []string{"q0", "q1", "q2", "q3", "q4", "NEW"}
	for i := range want {
		want[i] = ada.String() + " " + want[i]
	}
	if fmt.Sprint(sends) != fmt.Sprint(want) {
		t.Errorf("client sends = %q, want %q", sends, want)
	}
	seen := map[string]bool{}
	for _, id := range ids {
		if seen[id] {
			t.Errorf("WhatsApp id %s sent twice", id)
		}
		seen[id] = true
	}
}

// Concurrent retries of one not-sent message make one attempt between them.
func TestConcurrentRetriesOfANotSentMessageMakeOneAttempt(t *testing.T) {
	for i := range 200 {
		s, fake, _ := subscribedSession(t)
		fake.sendErr = errors.New("websocket closed")
		if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ErrNotSent) {
			t.Fatalf("Send = %v, want ErrNotSent", err)
		}
		fake.sendMu.Lock()
		fake.sendErr = nil
		fake.sendMu.Unlock()
		var wg sync.WaitGroup
		start := make(chan struct{})
		for range 8 {
			wg.Add(1)
			go func() { defer wg.Done(); <-start; _, _ = s.Retry("c-1") }()
		}
		close(start)
		wg.Wait()
		if sends, _ := fake.sent(); len(sends) != 2 {
			t.Fatalf("run %d: client sends = %q, want the failed send and exactly one retry attempt", i, sends)
		}
		if e := outboxState(t, s, "c-1"); e.State != store.OutboxSent {
			t.Fatalf("run %d: state %s, want sent", i, e.State)
		}
		s.drains.Wait()
		s.Close()
	}
}
