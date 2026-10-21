package wa

import (
	"errors"
	"fmt"
	"sync/atomic"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// pause is one pause a drain asked for: fire ends it as its timer would, unless it was cancelled.
type pause struct {
	d         time.Duration
	f         func()
	cancelled *atomic.Bool
}

func (p pause) fire() {
	if !p.cancelled.Load() {
		p.f()
	}
}

// pacer stands in for paceAfter: each pause the drain asks for is handed to the test, which ends
// it with fire, so nothing sleeps for real.
// Once the test ends (auto), every pause ends at once, so a failing test never leaves its
// cleanup's Close waiting on a paused drain.
type pacer struct {
	calls chan pause
	auto  atomic.Bool
}

func newPacer() *pacer { return &pacer{calls: make(chan pause, 16)} }

func (p *pacer) after(d time.Duration, f func()) func() bool {
	c := &atomic.Bool{}
	if p.auto.Load() {
		go f()
		return func() bool { return false }
	}
	p.calls <- pause{d: d, f: f, cancelled: c}
	return func() bool { return !c.Swap(true) }
}

// release ends every pending pause and every later one.
func (p *pacer) release() {
	p.auto.Store(true)
	for {
		select {
		case c := <-p.calls:
			c.fire()
		default:
			return
		}
	}
}

// next is the next pause the drain asks for; it fails the test when none comes.
func (p *pacer) next(t *testing.T) pause {
	t.Helper()
	select {
	case c := <-p.calls:
		return c
	case <-time.After(5 * time.Second):
		t.Fatal("the drain asked for no pause")
		return pause{}
	}
}

// none checks that no pause is pending.
func (p *pacer) none(t *testing.T) {
	t.Helper()
	select {
	case c := <-p.calls:
		t.Errorf("unexpected pause of %s", c.d)
	default:
	}
}

// pacedSession is offlineSession with the pacer in place of the drain's real pauses.
func pacedSession(t *testing.T) (*Session, *fakeClient, *pacer) {
	t.Helper()
	s, fake, _, _ := offlineSession(t)
	p := newPacer()
	s.paceAfter = p.after
	t.Cleanup(p.release) // runs before offlineSession's Close
	return s, fake, p
}

// waitDrains waits for the running drains, failing the test if they do not end within d.
func waitDrains(t *testing.T, s *Session, d time.Duration) {
	t.Helper()
	done := make(chan struct{})
	go func() { s.drains.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(d):
		t.Fatalf("the drain did not end within %s", d)
	}
}

func sendCount(fake *fakeClient) int {
	sends, _ := fake.sent()
	return len(sends)
}

// Rule 1: consecutive drained sends are spaced by drainPace, with no pause before the first or
// after the last.
func TestDrainedSendsAreSpacedByThePace(t *testing.T) {
	s, fake, p := pacedSession(t)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")
	queue(t, s, "c-3", "three")

	fake.emit(&waevents.Connected{})
	for want := 1; want <= 2; want++ {
		c := p.next(t)
		if c.d != drainPace {
			t.Errorf("pause %d = %s, want %s", want, c.d, drainPace)
		}
		if n := sendCount(fake); n != want {
			t.Fatalf("sends while paused = %d, want %d: the next one waits for the pause", n, want)
		}
		c.fire()
	}
	waitDrains(t, s, 5*time.Second)
	if n := sendCount(fake); n != 3 {
		t.Errorf("sends = %d, want 3", n)
	}
	p.none(t)
	if drainPace <= 0 || drainPace > time.Second {
		t.Errorf("drainPace = %s, want a small positive pause", drainPace)
	}
}

// Rule 2: a stop (or Close) during the pause ends the drain at once and sends nothing more; the
// rest stays queued for the next start.
func TestAStopDuringThePauseEndsTheDrainAndSendsNothingMore(t *testing.T) {
	for _, tc := range []struct {
		name string
		stop func(*Session, *fakeClient)
	}{
		{"stream replaced", func(_ *Session, f *fakeClient) { f.emit(&waevents.StreamReplaced{}) }},
		{"logged out", func(_ *Session, f *fakeClient) { f.emit(&waevents.LoggedOut{}) }},
		{"dropped", func(_ *Session, f *fakeClient) { f.emit(&waevents.Disconnected{}) }},
		{"closed", func(s *Session, _ *fakeClient) { s.Close() }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, p := pacedSession(t)
			queue(t, s, "c-1", "one")
			queue(t, s, "c-2", "two")
			queue(t, s, "c-3", "three")

			fake.emit(&waevents.Connected{})
			c := p.next(t)
			t.Cleanup(c.fire) // a pause already taken from the pacer is not released by it
			stopped := make(chan struct{})
			go func() { tc.stop(s, fake); close(stopped) }()
			select {
			case <-stopped:
			case <-time.After(5 * time.Second):
				t.Fatal("the stop did not return while the drain was paused")
			}
			waitDrains(t, s, 5*time.Second)
			if !c.cancelled.Load() {
				t.Error("the pause's timer was not cancelled")
			}
			if n := sendCount(fake); n != 1 {
				t.Errorf("sends = %d, want 1: nothing after the stop", n)
			}
			for _, id := range []string{"c-2", "c-3"} {
				if e := outboxState(t, s, id); e.State != store.OutboxQueued {
					t.Errorf("%s = %s, want still queued", id, e.State)
				}
			}
			p.none(t)
		})
	}
}

// Rule 3: a normal send is never paced. While a drain pauses, a send goes out at once (after the
// older queued sends, in order, as always), and the drain does not pause again for sends that are
// no longer queued.
func TestANormalSendIsNotDelayedByThePace(t *testing.T) {
	s, fake, p := pacedSession(t)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")
	queue(t, s, "c-3", "three")

	fake.emit(&waevents.Connected{})
	c := p.next(t)
	type result struct {
		sent ipc.Sent
		err  error
	}
	done := make(chan result, 1)
	go func() {
		got, err := s.Send(ada.String(), "NEW", "c-new")
		done <- result{got, err}
	}()
	select {
	case r := <-done:
		if r.err != nil || r.sent.State != ipc.SendSent {
			t.Errorf("Send during the pause = %+v, %v; want sent", r.sent, r.err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a normal send waited for the drain's pause")
	}
	p.none(t)
	c.fire()
	waitDrains(t, s, 5*time.Second)
	p.none(t)

	sends, _ := fake.sent()
	want := []string{"one", "two", "three", "NEW"}
	for i := range want {
		want[i] = ada.String() + " " + want[i]
	}
	if fmt.Sprint(sends) != fmt.Sprint(want) {
		t.Errorf("client sends = %q, want %q", sends, want)
	}

	// A linked send with nothing queued asks for no pause either.
	if _, err := s.Send(ada.String(), "again", "c-again"); err != nil {
		t.Fatalf("Send: %v", err)
	}
	p.none(t)
}

// The drain still sends in order under the ids chosen when queued, with its usual outcomes, and a
// retry of the one not confirmed reuses its WhatsApp id.
func TestAPacedDrainKeepsOrderIDsOutcomesAndRetry(t *testing.T) {
	s, fake, p := pacedSession(t)
	stopAuto := make(chan struct{})
	go func() {
		for {
			select {
			case c := <-p.calls:
				c.fire()
			case <-stopAuto:
				return
			}
		}
	}()
	defer close(stopAuto)
	queue(t, s, "c-1", "one")
	queue(t, s, "c-2", "two")
	queue(t, s, "c-3", "three")
	queuedIDs := []string{outboxState(t, s, "c-1").MessageID, outboxState(t, s, "c-2").MessageID, outboxState(t, s, "c-3").MessageID}
	fake.sendErrs = []error{nil, whatsmeow.ErrMessageTimedOut, errors.New("websocket closed")}

	reconnect(s, fake)

	sends, ids := fake.sent()
	wantSends := []string{ada.String() + " one", ada.String() + " two", ada.String() + " three"}
	if fmt.Sprint(sends) != fmt.Sprint(wantSends) || fmt.Sprint(ids) != fmt.Sprint(queuedIDs) {
		t.Fatalf("client sends = %q with ids %q, want %q with %q", sends, ids, wantSends, queuedIDs)
	}
	for c, want := range map[string]string{"c-1": store.OutboxSent, "c-2": store.OutboxNotConfirmed, "c-3": store.OutboxNotSent} {
		if e := outboxState(t, s, c); e.State != want {
			t.Errorf("%s = %s, want %s", c, e.State, want)
		}
	}
	got, err := s.Retry("c-2")
	if err != nil || got.State != ipc.SendSent || got.ID != queuedIDs[1] {
		t.Errorf("Retry c-2 = %+v, %v; want sent under %s", got, err, queuedIDs[1])
	}
	if _, ids := fake.sent(); len(ids) != 4 || ids[3] != queuedIDs[1] {
		t.Errorf("sent ids = %q, want the retry under %s", ids, queuedIDs[1])
	}
}
