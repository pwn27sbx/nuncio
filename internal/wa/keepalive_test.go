package wa

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// keepAliveDead is the keepalive failure whatsmeow reports once the pings have failed for longer
// than its own KeepAliveMaxFailTime: the point where it forces a reconnect when auto-reconnect is on.
func keepAliveDead(now time.Time) *waevents.KeepAliveTimeout {
	return &waevents.KeepAliveTimeout{ErrorCount: 7, LastSuccess: now.Add(-whatsmeow.KeepAliveMaxFailTime - time.Second)}
}

// keepAliveSession is reconnectingSession with its connection already four minutes old, so a
// dead-keepalive event built from the clock belongs to this connection, not to an earlier one.
func keepAliveSession(t *testing.T, fake *fakeClient) (*Session, *fakeTimers, *time.Time) {
	t.Helper()
	s, timers, now := reconnectingSession(t, fake)
	*now = now.Add(4 * time.Minute)
	return s, timers, now
}

// A connection whose keepalives have failed past whatsmeow's threshold is dead even though no
// socket error arrived: the session reports offline, drops the dead socket and reconnects through
// its own backoff.
func TestDeadKeepAliveGoesOfflineDisconnectsAndReconnects(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, now := keepAliveSession(t, fake)
	b := ipc.NewBroker()
	s.SetEvents(b)
	frames, cancel := b.Subscribe(nil)
	t.Cleanup(cancel)
	if got := s.State(); got != StateLinked {
		t.Fatalf("State() before the keepalive failure = %q, want %q", got, StateLinked)
	}

	fake.emit(keepAliveDead(*now))

	if got := s.State(); got != StateOffline {
		t.Errorf("State() after a dead keepalive = %q, want %q", got, StateOffline)
	}
	select {
	case f := <-frames:
		var ev ipc.EventConnection
		if err := json.Unmarshal(f, &ev); err != nil || ev.Op != ipc.OpEventConnection || ev.Session != string(StateOffline) {
			t.Errorf("published %s, want event_connection offline", f)
		}
	default:
		t.Error("no event_connection published after a dead keepalive")
	}
	if fake.disconnects != 1 {
		t.Errorf("Disconnect was called %d times, want 1 (the dead socket is dropped)", fake.disconnects)
	}
	if len(timers.delays) != 1 || !within(timers.delays[0], 2*time.Second) {
		t.Fatalf("scheduled delays = %v, want one in [1s, 2s) from the backoff", timers.delays)
	}
	timers.fire(t)
	if fake.connects != 2 || s.State() != StateLinked {
		t.Errorf("after the delay: %d connects, state %q; want 2 and %q", fake.connects, s.State(), StateLinked)
	}
}

// whatsmeow reports every failed ping; one that fails while the last success is recent, followed by
// KeepAliveRestored, is a transient blip and changes nothing.
func TestTransientKeepAliveTimeoutThenRestoredDoesNothing(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, now := keepAliveSession(t, fake)
	fake.emit(&waevents.KeepAliveTimeout{ErrorCount: 1, LastSuccess: now.Add(-40 * time.Second)})
	fake.emit(&waevents.KeepAliveTimeout{ErrorCount: 2, LastSuccess: now.Add(-whatsmeow.KeepAliveMaxFailTime)})
	fake.emit(&waevents.KeepAliveRestored{})
	if got := s.State(); got != StateLinked {
		t.Errorf("State() = %q, want %q", got, StateLinked)
	}
	if fake.disconnects != 0 || len(timers.delays) != 0 {
		t.Errorf("%d disconnects, delays %v; want none below whatsmeow's threshold", fake.disconnects, timers.delays)
	}
}

// After a stop (here a logout), a dead keepalive must not revive reconnecting.
func TestDeadKeepAliveAfterAStopDoesNothing(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, now := keepAliveSession(t, fake)
	fake.emit(&waevents.LoggedOut{})
	fake.emit(keepAliveDead(*now))
	if len(timers.delays) != 0 || fake.disconnects != 0 {
		t.Errorf("after a logout: delays %v, %d disconnects; want none", timers.delays, fake.disconnects)
	}
	if got := s.State(); got != StateUnlinked {
		t.Errorf("State() = %q, want %q (the stop's state stays)", got, StateUnlinked)
	}
}

// While a reconnect is already pending (the session is offline), more dead keepalives from the old
// socket start no second loop.
func TestDeadKeepAliveWhileReconnectingStartsNoSecondLoop(t *testing.T) {
	fake := &fakeClient{device: true}
	_, timers, now := keepAliveSession(t, fake)
	fake.emit(keepAliveDead(*now))
	fake.emit(keepAliveDead(*now))
	fake.emit(&waevents.Disconnected{})
	fake.emit(keepAliveDead(*now))
	if n := timers.pending(); n != 1 || len(timers.delays) != 1 {
		t.Errorf("%d timers pending, delays %v; want exactly one reconnect", n, timers.delays)
	}
	if fake.disconnects != 1 {
		t.Errorf("Disconnect was called %d times, want 1", fake.disconnects)
	}
}

// A session still connecting (Connect returned, no Connected yet) is offline already; a stale dead
// keepalive must not tear that connection down and start a second attempt.
func TestDeadKeepAliveWhileConnectingDoesNothing(t *testing.T) {
	fake := &fakeClient{device: true, connecting: true}
	s, timers, now := keepAliveSession(t, fake)
	fake.emit(keepAliveDead(*now))
	if fake.disconnects != 0 || len(timers.delays) != 0 {
		t.Errorf("%d disconnects, delays %v; want none while connecting", fake.disconnects, timers.delays)
	}
	if got := s.State(); got != StateOffline {
		t.Errorf("State() = %q, want %q", got, StateOffline)
	}
}

// whatsmeow dispatches each KeepAliveTimeout on its own goroutine, so one from the old, dead
// connection can arrive after the reconnect is up. Its LastSuccess predates the new connection's
// Connect, and it must not tear the healthy connection down; a later dead keepalive of the new
// connection still acts.
func TestLateKeepAliveOfTheOldConnectionSparesTheNewOne(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, now := keepAliveSession(t, fake)
	old := keepAliveDead(*now)
	fake.emit(old)
	*now = now.Add(2 * time.Second)
	timers.fire(t) // the reconnect: Connect, then Connected
	if s.State() != StateLinked || fake.disconnects != 1 {
		t.Fatalf("after the reconnect: state %q, %d disconnects; want %q and 1", s.State(), fake.disconnects, StateLinked)
	}
	fake.emit(old) // late, from the old connection's keepalive loop
	if s.State() != StateLinked || fake.disconnects != 1 || timers.pending() != 0 {
		t.Errorf("after a late old-connection timeout: state %q, %d disconnects, %d pending; want %q, 1, 0",
			s.State(), fake.disconnects, timers.pending(), StateLinked)
	}
	*now = now.Add(4 * time.Minute)
	fake.emit(keepAliveDead(*now)) // the new connection dies too
	if s.State() != StateOffline || fake.disconnects != 2 {
		t.Errorf("after the new connection's dead keepalive: state %q, %d disconnects; want %q and 2", s.State(), fake.disconnects, StateOffline)
	}
}

// After Close nothing reconnects, a dead keepalive included.
func TestDeadKeepAliveAfterCloseDoesNothing(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, now := keepAliveSession(t, fake)
	s.Close()
	closed := fake.disconnects
	fake.emit(keepAliveDead(*now))
	if len(timers.delays) != 0 || fake.disconnects != closed {
		t.Errorf("after Close: delays %v, %d more disconnects; want none", timers.delays, fake.disconnects-closed)
	}
}

// The keepalive reconnect is logged once, with the time since the last good ping and nothing personal.
func TestDeadKeepAliveIsLoggedOnce(t *testing.T) {
	fake := &fakeClient{device: true}
	s, _, now := keepAliveSession(t, fake)
	var lines []string
	s.logf = func(format string, args ...any) { lines = append(lines, fmt.Sprintf(format, args...)) }
	fake.emit(keepAliveDead(*now))
	fake.emit(keepAliveDead(*now))
	if len(lines) != 1 || !strings.Contains(lines[0], "keepalive") {
		t.Errorf("log = %q, want one line about the keepalive", lines)
	}
}

// B1: the real whatsmeow client OpenSession builds sends normal delivery receipts (two grey
// ticks) without being marked online, and keeps whatsmeow's own reconnect loop off.
func TestOpenSessionForcesActiveDeliveryReceiptsWithoutPresence(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio.db")
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	_ = db.Close()
	s, err := OpenSession(context.Background(), path)
	if err != nil {
		t.Fatalf("OpenSession: %v", err)
	}
	t.Cleanup(s.Close)
	wc, ok := s.client.(whatsmeowClient)
	if !ok {
		t.Fatalf("OpenSession client is %T, want whatsmeowClient", s.client)
	}
	if wc.c.EnableAutoReconnect {
		t.Error("EnableAutoReconnect is on; the Session's Backoff owns reconnecting")
	}
	// whatsmeow keeps the mode in an unexported atomic (client.go: sendActiveReceipts): 0 sends
	// type="inactive" receipts, 1 is set by SendPresence(available), 2 by
	// SetForceActiveDeliveryReceipts(true). Reading it by reflection fails loudly if a whatsmeow
	// upgrade renames it, which is when this evidence must be re-checked.
	f := reflect.ValueOf(wc.c).Elem().FieldByName("sendActiveReceipts")
	if !f.IsValid() {
		t.Fatal("whatsmeow.Client has no sendActiveReceipts field any more; re-check receipt.go")
	}
	if got := f.FieldByName("v").Uint(); got != 2 {
		t.Errorf("sendActiveReceipts = %d, want 2 (forced active, not marked online)", got)
	}
}
