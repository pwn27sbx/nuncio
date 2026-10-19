package wa

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
)

// fakeClient stands in for whatsmeow: it never opens a socket. Connect delivers the events a real
// client would, through the handler the session registered.
type fakeClient struct {
	device      bool
	connectErr  error
	connects    int
	disconnects int
	markReads   int
	handlers    []func(any)
}

func (f *fakeClient) HasDevice() bool { return f.device }

func (f *fakeClient) Connect() error {
	f.connects++
	if f.connectErr != nil {
		return f.connectErr
	}
	f.emit(&waevents.Connected{})
	return nil
}

func (f *fakeClient) Disconnect() { f.disconnects++ }

// MarkRead mirrors whatsmeow's read receipt call, so a test can prove it is never sent.
func (f *fakeClient) MarkRead(context.Context, []types.MessageID, time.Time, types.JID, types.JID) error {
	f.markReads++
	return nil
}

func (f *fakeClient) AddEventHandler(h func(any)) { f.handlers = append(f.handlers, h) }

func (f *fakeClient) emit(e any) {
	for _, h := range f.handlers {
		h(e)
	}
}

func TestSessionWithoutDeviceStaysUnlinkedAndNeverConnects(t *testing.T) {
	fake := &fakeClient{device: false}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if got := s.State(); got != StateUnlinked {
		t.Errorf("State() = %q, want %q", got, StateUnlinked)
	}
	if fake.connects != 0 {
		t.Errorf("Connect was called %d times without a stored device, want 0", fake.connects)
	}
	s.Close()
	if fake.disconnects != 0 {
		t.Errorf("Close called Disconnect %d times on a session that never connected, want 0", fake.disconnects)
	}
}

func TestSessionWithDeviceConnectsOnceAndReportsLinked(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times, want 1", fake.connects)
	}
	if got := s.State(); got != StateLinked {
		t.Errorf("State() = %q, want %q", got, StateLinked)
	}
}

func TestSessionDisconnectEventReportsOffline(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.Disconnected{})
	if got := s.State(); got != StateOffline {
		t.Errorf("State() after a disconnect = %q, want %q", got, StateOffline)
	}
	// No reconnect here: that is the backoff loop's job, not the session's.
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times after a disconnect, want 1", fake.connects)
	}
}

func TestSessionFailedConnectReportsOffline(t *testing.T) {
	fake := &fakeClient{device: true, connectErr: errors.New("no network")}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err == nil {
		t.Error("Start returned nil after a failed connect, want the error")
	}
	if got := s.State(); got != StateOffline {
		t.Errorf("State() after a failed connect = %q, want %q", got, StateOffline)
	}
}

func TestSessionCloseDisconnectsAndClosesTheStore(t *testing.T) {
	fake := &fakeClient{device: true}
	closed := 0
	s := NewSession(fake, nil, func() error { closed++; return nil })
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	s.Close()
	if fake.disconnects != 1 {
		t.Errorf("Close called Disconnect %d times, want 1", fake.disconnects)
	}
	if closed != 1 {
		t.Errorf("Close closed the store %d times, want 1", closed)
	}
}

func TestOpenSessionWithoutDatabaseIsUnlinkedAndCreatesNothing(t *testing.T) {
	dataDir := filepath.Join(t.TempDir(), "nuncio")
	s, err := OpenSession(context.Background(), filepath.Join(dataDir, "nuncio.db"))
	if err != nil {
		t.Fatalf("OpenSession: %v", err)
	}
	defer s.Close()
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if got := s.State(); got != StateUnlinked {
		t.Errorf("State() = %q, want %q", got, StateUnlinked)
	}
	if _, err := os.Stat(dataDir); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("stat %s = %v, want it not to exist: starting a session must not create the data dir", dataDir, err)
	}
}

func TestDBPathFollowsXDGDataHomeWithoutCreatingIt(t *testing.T) {
	base := t.TempDir()
	t.Setenv("XDG_DATA_HOME", base)
	got, err := DBPath()
	if err != nil {
		t.Fatalf("DBPath: %v", err)
	}
	if want := filepath.Join(base, "nuncio", "nuncio.db"); got != want {
		t.Errorf("DBPath() = %q, want %q", got, want)
	}
	if _, err := os.Stat(filepath.Join(base, "nuncio")); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("DBPath created the data dir (stat: %v)", err)
	}
}

// whatsmeow can end a session without a Disconnected event; each of these must still leave
// the linked state.
func TestSessionEndEventsLeaveLinked(t *testing.T) {
	cases := []struct {
		name  string
		event any
		want  State
	}{
		{"logged out reports unlinked", &waevents.LoggedOut{}, StateUnlinked},
		{"temporary ban reports offline", &waevents.TemporaryBan{}, StateOffline},
		{"stream replaced reports offline", &waevents.StreamReplaced{}, StateOffline},
		{"connect failure reports offline", &waevents.ConnectFailure{}, StateOffline},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeClient{device: true}
			s := NewSession(fake, nil, nil)
			if err := s.Start(); err != nil {
				t.Fatalf("Start: %v", err)
			}
			if got := s.State(); got != StateLinked {
				t.Fatalf("State() before the event = %q, want %q", got, StateLinked)
			}
			fake.emit(tc.event)
			if got := s.State(); got != tc.want {
				t.Errorf("State() after %T = %q, want %q", tc.event, got, tc.want)
			}
			if fake.connects != 1 {
				t.Errorf("Connect was called %d times after %T, want 1: reconnecting is not the session's job", fake.connects, tc.event)
			}
		})
	}
}

func TestEventsWithNilHandlersIgnoreSessionEndEvents(t *testing.T) {
	var e Events
	for _, ev := range []any{&waevents.LoggedOut{}, &waevents.TemporaryBan{}, &waevents.StreamReplaced{}, &waevents.ConnectFailure{}} {
		e.Handle(ev) // must not panic
	}
}

func TestSessionPublishesEachConnectionChangeOnce(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.Disconnected{})
	fake.emit(&waevents.Disconnected{}) // no change, no event
	fake.emit(&waevents.LoggedOut{})

	assertEvents(t, events(),
		`{"v":1,"op":"event_connection","session":"offline"}`,
		`{"v":1,"op":"event_connection","session":"linked"}`,
		`{"v":1,"op":"event_connection","session":"offline"}`,
		`{"v":1,"op":"event_connection","session":"unlinked"}`,
	)
}

func TestSessionWithoutEventsStillRuns(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	if err := s.MarkChatRead(ada.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	if n := totalUnread(t, db); n != 0 {
		t.Errorf("total unread = %d, want 0", n)
	}
}
