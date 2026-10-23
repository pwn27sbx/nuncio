package wa

import (
	"encoding/json"
	"errors"
	"sync"
	"testing"
	"time"

	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// windowPublisher records every connection frame published, as a subscriber reads them. Before it
// records the first frame for session state `at`, it runs land once: an event that arrives on
// another of whatsmeow's goroutines after the session decided that state and before that frame
// reached the subscribers.
type windowPublisher struct {
	mu     sync.Mutex
	frames []string
	at     string
	land   func()
}

func (p *windowPublisher) Publish(frame any) int {
	c, ok := frame.(ipc.EventConnection)
	if !ok {
		return 1
	}
	p.mu.Lock()
	land := p.land
	if c.Session == p.at {
		p.land = nil
	} else {
		land = nil
	}
	p.mu.Unlock()
	if land != nil {
		land()
	}
	p.mu.Lock()
	p.frames = append(p.frames, c.Session)
	p.mu.Unlock()
	return 1
}

func (p *windowPublisher) last() string {
	p.mu.Lock()
	defer p.mu.Unlock()
	if len(p.frames) == 0 {
		return ""
	}
	return p.frames[len(p.frames)-1]
}

func (p *windowPublisher) all() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.frames...)
}

// A terminal or connection event that lands between the session deciding a state and that state's
// frame reaching the subscribers must leave the subscribers on the session's final state, never on
// the stale one published last.
func TestAnEventInsideAStatePublishLeavesSubscribersOnTheFinalState(t *testing.T) {
	cases := []struct {
		name    string
		at      string // the state whose publish the event lands inside
		trigger func(fake *fakeClient, now time.Time)
		land    any
		want    State
	}{
		{"logout inside a drop's offline", ipc.SessionOffline,
			func(f *fakeClient, _ time.Time) { f.emit(&waevents.Disconnected{}) }, &waevents.LoggedOut{}, StateUnlinked},
		{"logout inside a dead keepalive's offline", ipc.SessionOffline,
			func(f *fakeClient, now time.Time) {
				f.emit(&waevents.KeepAliveTimeout{LastSuccess: now.Add(-4 * time.Minute)})
			}, &waevents.LoggedOut{}, StateUnlinked},
		{"drop inside a reconnect's linked", ipc.SessionLinked,
			func(f *fakeClient, _ time.Time) { f.emit(&waevents.Connected{}) }, &waevents.Disconnected{}, StateOffline},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeClient{device: true}
			s, _, now := reconnectingSession(t, fake)
			if tc.at == ipc.SessionLinked {
				fake.emit(&waevents.Disconnected{}) // offline, so a Connected changes the state
			}
			// The connection the keepalive watches started four minutes ago.
			*now = now.Add(4 * time.Minute)
			pub := &windowPublisher{at: tc.at, land: func() { fake.emit(tc.land) }}
			s.mu.Lock()
			s.events = pub
			s.mu.Unlock()

			tc.trigger(fake, *now)

			if got := s.State(); got != tc.want {
				t.Errorf("State() = %q, want %q", got, tc.want)
			}
			if got := pub.last(); got != string(s.State()) {
				t.Errorf("subscribers' last connection frame = %q (frames %v), want the session's state %q", got, pub.all(), s.State())
			}
		})
	}
}

// The same race with real goroutines: a drop and a logout arrive at once, as whatsmeow dispatches
// them. The logout wins whatever the order, and the subscribers' last frame is the final state.
func TestADropRacingALogoutEndsUnlinkedForSubscribers(t *testing.T) {
	for i := 0; i < 20; i++ {
		fake := &fakeClient{device: true}
		s := NewSession(fake, nil, nil)
		s.logf = func(string, ...any) {}
		b := ipc.NewBroker()
		s.SetEvents(b)
		frames, cancel := b.Subscribe(nil)
		if err := s.Start(); err != nil {
			t.Fatalf("Start: %v", err)
		}
		var wg sync.WaitGroup
		start := make(chan struct{})
		for _, ev := range []any{&waevents.Disconnected{}, &waevents.LoggedOut{}} {
			wg.Add(1)
			go func(ev any) {
				defer wg.Done()
				<-start
				fake.emit(ev)
			}(ev)
		}
		close(start)
		wg.Wait()
		var last string
		for done := false; !done; {
			select {
			case f := <-frames:
				var c ipc.EventConnection
				if err := json.Unmarshal(f, &c); err != nil {
					t.Fatalf("frame %q: %v", f, err)
				}
				last = c.Session
			default:
				done = true
			}
		}
		if got := s.State(); got != StateUnlinked {
			t.Fatalf("run %d: State() = %q, want %q: a logout must win over a racing drop", i, got, StateUnlinked)
		}
		if last != string(StateUnlinked) {
			t.Fatalf("run %d: subscribers' last connection frame = %q, want %q", i, last, StateUnlinked)
		}
		cancel()
		s.Close()
	}
}

// logoutOnConnect is a client whose Connect reports a logout, then fails, as whatsmeow can when
// the server answers the login with a 401.
type logoutOnConnect struct{ *fakeClient }

func (c logoutOnConnect) Connect() error {
	c.connects++
	c.emit(&waevents.LoggedOut{})
	return errors.New("websocket closed")
}

// A failed Start whose Connect already reported a logout stays unlinked: the failure's offline
// never overwrites the stop.
func TestAFailedStartAfterALogoutStaysUnlinked(t *testing.T) {
	c := logoutOnConnect{&fakeClient{device: true}}
	s := NewSession(c, nil, nil)
	s.logf = t.Logf
	t.Cleanup(s.Close)
	events := eventsOf(t, s)
	if err := s.Start(); err == nil {
		t.Fatal("Start = nil, want the connect error")
	}
	if got := s.State(); got != StateUnlinked {
		t.Errorf("State() = %q, want %q", got, StateUnlinked)
	}
	got := events()
	if n := len(got); n == 0 || got[n-1] != `{"v":1,"op":"event_connection","session":"unlinked","stop_reason":"logged_out"}` {
		t.Errorf("events = %v, want the last one unlinked", got)
	}
}

// panicOncePublisher panics on its first Publish, then records every connection frame.
type panicOncePublisher struct {
	mu       sync.Mutex
	panicked bool
	frames   []string
}

func (p *panicOncePublisher) Publish(frame any) int {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.panicked {
		p.panicked = true
		panic("encoder failed")
	}
	if c, ok := frame.(ipc.EventConnection); ok {
		p.frames = append(p.frames, c.Session)
	}
	return 1
}

// A Publish that panics (whatsmeow recovers its handlers' panics) loses at most its own frame: the
// next state change is published again.
func TestAPanickingPublishLosesOnlyItsOwnFrame(t *testing.T) {
	fake := &fakeClient{device: true}
	s, _, _ := reconnectingSession(t, fake)
	pub := &panicOncePublisher{}
	s.mu.Lock()
	s.events = pub
	s.mu.Unlock()
	func() {
		defer func() { _ = recover() }()
		fake.emit(&waevents.Disconnected{})
	}()
	fake.emit(&waevents.LoggedOut{})
	pub.mu.Lock()
	defer pub.mu.Unlock()
	if want := []string{"unlinked"}; len(pub.frames) != 1 || pub.frames[0] != want[0] {
		t.Errorf("frames after the panic = %v, want %v", pub.frames, want)
	}
}

// A Connected that arrives after a stop or Close never brings the session back to linked.
func TestAConnectedAfterAStopOrCloseKeepsTheStopState(t *testing.T) {
	cases := []struct {
		name string
		stop func(s *Session, f *fakeClient)
		want State
		// reason is the stop_reason the last frame carries ("" for none: Close is not a stop).
		reason string
	}{
		{"logout", func(_ *Session, f *fakeClient) { f.emit(&waevents.LoggedOut{}) }, StateUnlinked, "logged_out"},
		{"temporary ban", func(_ *Session, f *fakeClient) { f.emit(&waevents.TemporaryBan{}) }, StateOffline, "temporary_ban"},
		{"close", func(s *Session, _ *fakeClient) { s.Close() }, StateOffline, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeClient{device: true}
			s, timers, _ := reconnectingSession(t, fake)
			events := eventsOf(t, s)
			tc.stop(s, fake)
			fake.emit(&waevents.Connected{})
			if got := s.State(); got != tc.want {
				t.Errorf("State() = %q, want %q", got, tc.want)
			}
			got := events()
			want := `{"v":1,"op":"event_connection","session":"` + string(tc.want) + `"`
			if tc.reason != "" {
				want += `,"stop_reason":"` + tc.reason + `"`
			}
			want += `}`
			if n := len(got); n == 0 || got[n-1] != want {
				t.Errorf("events = %v, want the last one %s", got, want)
			}
			if timers.pending() != 0 {
				t.Errorf("%d reconnects pending after a stop", timers.pending())
			}
		})
	}
}
