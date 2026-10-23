package wa

import (
	"errors"
	"math/rand/v2"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// linkedOutboxSession is a linked session over a real store whose reconnects run on fake timers
// (nothing reconnects unless the test emits Connected) and whose clock is four minutes past its
// Connect, so a dead keepalive built from it belongs to this connection.
func linkedOutboxSession(t *testing.T) (*Session, *fakeClient, func() []string, *time.Time) {
	t.Helper()
	fake := &fakeClient{device: true}
	s := NewSession(fake, openTestStore(t), nil)
	now := time.Date(2026, 10, 19, 9, 0, 0, 0, time.UTC)
	s.after = (&fakeTimers{}).after
	s.logf = t.Logf
	s.backoff.Rand = rand.New(rand.NewPCG(1, 2)).Float64
	s.backoff.Now = func() time.Time { return now }
	events := eventsOf(t, s)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	s.drains.Wait()
	now = now.Add(4 * time.Minute)
	events()
	return s, fake, events, &now
}

// dropErr is what real whatsmeow answers a SendMessage pending when the socket drops: its
// response waiter is cleared with a stream end, and retryFrame gives up with this error once the
// connection does not come back within 5 s, or at once after an expected Disconnect.
func dropErr() error { return &whatsmeow.DisconnectedError{Action: "message send"} }

// during runs f inside the first n SendMessage calls, as whatever happens to the connection
// while those attempts wait for their ack.
func during(fake *fakeClient, n int64, f func()) {
	var calls atomic.Int64
	fake.onSend = func(string) {
		if calls.Add(1) <= n {
			f()
		}
	}
}

// assertQueuedForTheDrain checks that send clientID is queued, still under messageID, listed and
// published as queued and never as not sent.
func assertQueuedForTheDrain(t *testing.T, s *Session, events []string, clientID, messageID string) {
	t.Helper()
	if e := outboxState(t, s, clientID); e.State != store.OutboxQueued || e.MessageID != messageID {
		t.Errorf("outbox %s = %s under %s, want queued under %s", clientID, e.State, e.MessageID, messageID)
	}
	listed := storedMessages(t, s, ada.String())
	if len(listed) != 1 || WireMessage(listed[0]).State != ipc.MessageQueued || listed[0].ID != messageID {
		t.Errorf("listed %+v, want the message once, queued", listed)
	}
	var queuedEvents int
	for _, e := range events {
		if strings.Contains(e, `"state":"not_sent"`) {
			t.Errorf("event %s, want the send never published as not sent", e)
		}
		if jsonHas(e, `"op":"event_message"`, `"state":"queued"`, `"client_id":"`+clientID+`"`, `"id":"`+messageID+`"`) {
			queuedEvents++
		}
	}
	if queuedEvents != 1 {
		t.Errorf("queued events of %s = %d in %q, want 1", clientID, queuedEvents, events)
	}
}

// assertDrainedOnce reconnects and checks that the drain sent clientID once more, under the same
// WhatsApp id, ending sent; a later reconnect sends nothing again.
func assertDrainedOnce(t *testing.T, s *Session, fake *fakeClient, clientID, messageID string, sendsBefore int) {
	t.Helper()
	reconnect(s, fake)
	sends, ids := fake.sent()
	if len(sends) != sendsBefore+1 || ids[len(ids)-1] != messageID {
		t.Fatalf("client sends after the reconnect = %q with ids %q, want one more under %s", sends, ids, messageID)
	}
	if e := outboxState(t, s, clientID); e.State != store.OutboxSent {
		t.Errorf("outbox %s after the drain = %s, want sent", clientID, e.State)
	}
	if listed := storedMessages(t, s, ada.String()); len(listed) != 1 || listed[0].ID != messageID || listed[0].State != "" {
		t.Errorf("listed %+v, want the message once, sent", listed)
	}
	fake.emit(&waevents.Disconnected{})
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != sendsBefore+1 {
		t.Errorf("client sends after a later reconnect = %q, want no further send", sends)
	}
}

// An ack that times out because the connection died (a dead keepalive takes the session offline
// while the send waits) is queued, as WhatsApp itself queues it, and the next reconnect's drain
// sends it once under the WhatsApp id it was queued with.
func TestAnAckTimeoutAsADeadKeepaliveTakesTheSessionOfflineIsQueued(t *testing.T) {
	s, fake, events, now := linkedOutboxSession(t)
	fake.sendErrs = []error{whatsmeow.ErrMessageTimedOut}
	during(fake, 1, func() { fake.emit(keepAliveDead(*now)) })

	got, err := s.Send(ada.String(), "On my way", "c-1")
	if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
		t.Fatalf("Send whose ack timed out while the session went offline = %+v, %v; want queued", got, err)
	}
	if st := s.State(); st != StateOffline {
		t.Fatalf("State() = %q, want offline", st)
	}
	_, ids := fake.sent()
	assertQueuedForTheDrain(t, s, events(), "c-1", ids[0])
	// Repeating the client_id answers its stored state: still queued.
	if again, err := s.Send(ada.String(), "On my way", "c-1"); err != nil || again != got {
		t.Errorf("repeated Send = %+v, %v; want %+v", again, err, got)
	}
	assertDrainedOnce(t, s, fake, "c-1", ids[0], 1)
}

// With real whatsmeow a socket drop answers the pending send with a DisconnectedError, never a
// timeout: the outcome is just as unknown (the frame may have been written), so it is queued.
func TestARealDropAnswersDisconnectedErrorAndIsQueued(t *testing.T) {
	for _, tc := range []struct {
		name string
		drop func(fake *fakeClient, now time.Time)
	}{
		{"disconnected", func(fake *fakeClient, _ time.Time) { fake.emit(&waevents.Disconnected{}) }},
		{"dead keepalive", func(fake *fakeClient, now time.Time) { fake.emit(keepAliveDead(now)) }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, events, now := linkedOutboxSession(t)
			fake.sendErrs = []error{dropErr()}
			during(fake, 1, func() { tc.drop(fake, *now) })

			got, err := s.Send(ada.String(), "On my way", "c-1")
			if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
				t.Fatalf("Send = %+v, %v; want queued", got, err)
			}
			_, ids := fake.sent()
			assertQueuedForTheDrain(t, s, events(), "c-1", ids[0])
			assertDrainedOnce(t, s, fake, "c-1", ids[0], 1)
		})
	}
}

// A DisconnectedError while the session still counts as linked is an unknown outcome too: not
// confirmed, never "not accepted", and never drained.
func TestADisconnectedErrorWhileStillLinkedIsNotConfirmed(t *testing.T) {
	s, fake, _, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{dropErr()}

	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotConfirmed) {
		t.Fatalf("Send = %v, want ipc.ErrNotConfirmed", err)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxNotConfirmed {
		t.Errorf("outbox c-1 = %s, want not_confirmed", e.State)
	}
	fake.emit(&waevents.Disconnected{})
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Errorf("client sends after a reconnect = %q, want no resend", sends)
	}
}

// A live send behind an older queued row whose attempt drops the session is not attempted on the
// dead connection (whatsmeow would answer ErrNotConnected): it stays queued for the drain.
func TestASendBehindAnOlderRowThatDropsTheSessionStaysQueued(t *testing.T) {
	s, fake, events, _ := linkedOutboxSession(t)
	// An older send still queued while linked, as when a drain has not reached it yet.
	if _, _, err := store.Enqueue(s.Store(), store.OutboxEntry{ClientID: "c-0", ChatJID: ada.String(), Text: "older",
		MessageID: "3EB0CCC0", Created: time.Now().UTC().Add(-time.Minute)}); err != nil {
		t.Fatalf("Enqueue: %v", err)
	}
	fake.sendErrs = []error{dropErr(), whatsmeow.ErrNotConnected}
	during(fake, 1, func() { fake.emit(&waevents.Disconnected{}) })

	got, err := s.Send(ada.String(), "On my way", "c-1")
	if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
		t.Fatalf("Send = %+v, %v; want queued", got, err)
	}
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Fatalf("client sends = %q, want only the older row's attempt", sends)
	}
	if e := outboxState(t, s, "c-0"); e.State != store.OutboxNotConfirmed {
		t.Errorf("outbox c-0 = %s, want not_confirmed", e.State)
	}
	id := outboxState(t, s, "c-1").MessageID
	var queuedEvents int
	for _, e := range events() {
		if jsonHas(e, `"state":"queued"`, `"client_id":"c-1"`, `"id":"`+id+`"`) {
			queuedEvents++
		}
	}
	if queuedEvents != 1 {
		t.Errorf("queued events of c-1 = %d, want 1", queuedEvents)
	}
	fake.sendErrs = nil
	reconnect(s, fake)
	sends, ids := fake.sent()
	if len(sends) != 2 || ids[1] != id {
		t.Fatalf("client sends after the reconnect = %q with ids %q, want c-1 once under %s", sends, ids, id)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxSent {
		t.Errorf("outbox c-1 = %s, want sent", e.State)
	}
}

// A connection that drops (a Disconnected, or a dead keepalive) and comes back while the send
// waits leaves the session linked when the ack times out; the send still left a dead connection,
// so it is queued, and the drain of the reconnect that happened meanwhile sends it once (it waits
// for this attempt to end).
func TestAnAckTimeoutAcrossADropAndReconnectIsSentByThatReconnectsDrain(t *testing.T) {
	for _, tc := range []struct {
		name string
		drop func(fake *fakeClient, now time.Time)
	}{
		{"disconnected", func(fake *fakeClient, _ time.Time) { fake.emit(&waevents.Disconnected{}) }},
		{"dead keepalive", func(fake *fakeClient, now time.Time) { fake.emit(keepAliveDead(now)) }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, events, now := linkedOutboxSession(t)
			// whatsmeow's retryFrame resent the frame on the new connection and lost that one too.
			fake.sendErrs = []error{&whatsmeow.DisconnectedError{Action: "message send (retry)"}}
			during(fake, 1, func() {
				tc.drop(fake, *now)
				fake.emit(&waevents.Connected{})
			})

			got, err := s.Send(ada.String(), "On my way", "c-1")
			if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
				t.Fatalf("Send = %+v, %v; want queued", got, err)
			}
			if st := s.State(); st != StateLinked {
				t.Fatalf("State() = %q, want linked again", st)
			}
			// The drain is free to run as soon as Send returns, so wait for it before looking.
			s.drains.Wait()
			_, ids := fake.sent()
			var queuedEvents int
			for _, e := range events() {
				if strings.Contains(e, `"state":"not_sent"`) {
					t.Errorf("event %s, want the send never published as not sent", e)
				}
				if jsonHas(e, `"state":"queued"`, `"client_id":"c-1"`, `"id":"`+ids[0]+`"`) {
					queuedEvents++
				}
			}
			if queuedEvents != 1 {
				t.Errorf("queued events of c-1 = %d, want 1", queuedEvents)
			}
			sends, sentIDs := fake.sent()
			if len(sends) != 2 || sentIDs[1] != ids[0] {
				t.Fatalf("client sends = %q with ids %q, want the drain to send it once more under %s", sends, sentIDs, ids[0])
			}
			if e := outboxState(t, s, "c-1"); e.State != store.OutboxSent {
				t.Errorf("outbox c-1 = %s, want sent", e.State)
			}
		})
	}
}

// While the session is still linked when the ack times out, the outcome stays unknown: not
// confirmed, listed as not sent, never resent by a later drain; only a manual retry sends it.
func TestAnAckTimeoutWhileStillLinkedStaysNotConfirmedAndIsNotDrained(t *testing.T) {
	s, fake, _, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{whatsmeow.ErrMessageTimedOut}

	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotConfirmed) {
		t.Fatalf("Send = %v, want ipc.ErrNotConfirmed", err)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxNotConfirmed {
		t.Errorf("outbox c-1 = %s, want not_confirmed", e.State)
	}
	fake.emit(&waevents.Disconnected{})
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Errorf("client sends after a reconnect = %q, want no resend of a not-confirmed send", sends)
	}
}

// A queued send that the drain resends and that times out again as the connection drops follows
// the existing rules: it ends not confirmed, visible as not sent, and nothing resends it on its own.
func TestADrainedResendThatTimesOutAgainEndsNotConfirmedAndIsNotResent(t *testing.T) {
	s, fake, events, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{dropErr(), dropErr()}
	during(fake, 2, func() { fake.emit(&waevents.Disconnected{}) })

	if got, err := s.Send(ada.String(), "On my way", "c-1"); err != nil || got.State != ipc.SendQueued {
		t.Fatalf("Send = %+v, %v; want queued", got, err)
	}
	events()
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != 2 {
		t.Fatalf("client sends = %q, want the send and one drained resend", sends)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxNotConfirmed {
		t.Errorf("outbox c-1 after the failed resend = %s, want not_confirmed", e.State)
	}
	var notSent int
	for _, e := range events() {
		if jsonHas(e, `"state":"not_sent"`, `"client_id":"c-1"`) {
			notSent++
		}
	}
	if notSent != 1 {
		t.Errorf("not-sent events of c-1 = %d, want 1", notSent)
	}
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != 2 {
		t.Errorf("client sends after the next reconnect = %q, want no further send", sends)
	}
}

// A manual retry whose ack times out as the session drops is queued like a send, under the first
// WhatsApp id, and drained once.
func TestARetryWhoseAckTimesOutAsTheSessionDropsIsQueued(t *testing.T) {
	s, fake, events, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{errors.New("websocket closed"), dropErr()}
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send = %v, want ErrNotSent", err)
	}
	firstID := outboxState(t, s, "c-1").MessageID
	events()
	var calls atomic.Int64
	fake.onSend = func(string) {
		if calls.Add(1) == 1 {
			fake.emit(&waevents.Disconnected{})
		}
	}

	got, err := s.Retry("c-1")
	if err != nil || got != (ipc.Sent{State: ipc.SendQueued}) {
		t.Fatalf("Retry = %+v, %v; want queued", got, err)
	}
	assertQueuedForTheDrain(t, s, events(), "c-1", firstID)
	assertDrainedOnce(t, s, fake, "c-1", firstID, 2)
}

// A send WhatsApp refused (any error that is neither a timeout nor a lost connection) stays not
// sent even when the session drops during the attempt: only an unknown outcome is queued.
func TestARefusedSendWhileTheSessionDropsStaysNotSent(t *testing.T) {
	s, fake, _, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{errors.New("websocket closed")}
	during(fake, 1, func() { fake.emit(&waevents.Disconnected{}) })

	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send = %v, want ErrNotSent", err)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxNotSent {
		t.Errorf("outbox c-1 = %s, want not_sent", e.State)
	}
	reconnect(s, fake)
	if sends, _ := fake.sent(); len(sends) != 1 {
		t.Errorf("client sends after the reconnect = %q, want no resend", sends)
	}
}

// A logout during the attempt leaves nothing to drain into: the timeout stays not confirmed.
func TestAnAckTimeoutAsTheDeviceIsLoggedOutStaysNotConfirmed(t *testing.T) {
	s, fake, _, _ := linkedOutboxSession(t)
	fake.sendErrs = []error{whatsmeow.ErrMessageTimedOut}
	during(fake, 1, func() { fake.emit(&waevents.LoggedOut{}) })

	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ipc.ErrNotConfirmed) {
		t.Fatalf("Send = %v, want ipc.ErrNotConfirmed", err)
	}
	if e := outboxState(t, s, "c-1"); e.State != store.OutboxNotConfirmed {
		t.Errorf("outbox c-1 = %s, want not_confirmed", e.State)
	}
}
