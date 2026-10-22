package wa

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"math/rand/v2"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"
)

// fakeClient stands in for whatsmeow: it never opens a socket. Connect delivers the events a real
// client would, through the handler the session registered.
type fakeClient struct {
	device     bool
	connectErr error
	// connecting makes a successful Connect deliver no Connected event, as while whatsmeow is
	// still logging in.
	connecting  bool
	connects    int
	disconnects int
	handlers    []func(any)
	// markReads counts MarkRead calls; markReadCalls records each as "chat sender type id,id" and
	// markReadAt the time of the last one. MarkRead answers markReadErr, or, when markReadFailAt is
	// above 0, only its markReadFailAt-th call (from 1) fails, with markReadErr.
	markReads      int
	markReadCalls  []string
	markReadAt     time.Time
	markReadErr    error
	markReadFailAt int
	// onMarkRead, when set, runs at the start of every MarkRead call with its context.
	onMarkRead func(ctx context.Context)
	// readReceipts is the account's read-receipt privacy ("" reads as "all"); ReadReceiptsPrivacy
	// counts its calls in privacyAsks and answers privacyErr when set.
	readReceipts string
	privacyErr   error
	privacyAsks  int
	// sends records every SendMessage call as "jid text" and sendIDs its WhatsApp message id; it
	// answers sendTime, or the next of sendErrs (nil entries succeed), then sendErr. NewMessageID
	// answers sendID when set, otherwise a fresh id per call. sendMu guards them: a drain sends
	// from its own goroutine.
	sendMu   sync.Mutex
	sends    []string
	sendIDs  []string
	newIDs   int
	sendID   string
	sendTime time.Time
	sendErr  error
	sendErrs []error
	// msgs records every message given to SendMessage, builtReactions every BuildReaction call as
	// "chat sender id emoji".
	msgs           []*waE2E.Message
	builtReactions []string
	// onSend, when set, runs at the start of every SendMessage with its text, outside sendMu.
	onSend func(text string)
	// logouts counts Logout calls; Logout answers logoutErr.
	logouts   int
	logoutErr error
	onLogout  func() // runs inside Logout, when set
}

func (f *fakeClient) Logout(context.Context) error {
	f.logouts++
	if f.onLogout != nil {
		f.onLogout()
	}
	return f.logoutErr
}

func (f *fakeClient) SendMessage(_ context.Context, jid string, msg *waE2E.Message, id string) (time.Time, error) {
	text := msg.GetConversation()
	if text == "" {
		text = msg.GetExtendedTextMessage().GetText()
	}
	if f.onSend != nil {
		f.onSend(text)
	}
	f.sendMu.Lock()
	defer f.sendMu.Unlock()
	f.sends = append(f.sends, jid+" "+text)
	f.sendIDs = append(f.sendIDs, id)
	f.msgs = append(f.msgs, msg)
	err := f.sendErr
	if len(f.sendErrs) > 0 {
		err, f.sendErrs = f.sendErrs[0], f.sendErrs[1:]
	}
	if err != nil {
		return time.Time{}, err
	}
	return f.sendTime, nil
}

// BuildReaction mirrors whatsmeow's BuildReaction and BuildMessageKey for a fake own JID: an empty
// sender is me; another sender in a group is the key's participant. It records each call.
func (f *fakeClient) BuildReaction(chat, sender, id, emoji string) (*waE2E.Message, error) {
	f.sendMu.Lock()
	f.builtReactions = append(f.builtReactions, chat+" "+sender+" "+id+" "+emoji)
	f.sendMu.Unlock()
	key := &waCommon.MessageKey{FromMe: proto.Bool(sender == ""), ID: proto.String(id), RemoteJID: proto.String(chat)}
	if sender != "" && strings.HasSuffix(chat, "@g.us") {
		key.Participant = proto.String(sender)
	}
	return &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{Key: key, Text: proto.String(emoji)}}, nil
}

func (f *fakeClient) OwnJID() string { return fakeOwnJID }

func (f *fakeClient) OwnLID() string { return fakeOwnLID }

// fakeOwnJID is the fake account's own JID, fakeOwnLID its hidden id (LID).
const (
	fakeOwnJID = "15551230009@s.whatsapp.net"
	fakeOwnLID = "100000000000009@lid"
)

// sentMessages returns a copy of every message given to SendMessage.
func (f *fakeClient) sentMessages() []*waE2E.Message {
	f.sendMu.Lock()
	defer f.sendMu.Unlock()
	return append([]*waE2E.Message(nil), f.msgs...)
}

func (f *fakeClient) NewMessageID() string {
	f.sendMu.Lock()
	defer f.sendMu.Unlock()
	if f.sendID != "" {
		return f.sendID
	}
	f.newIDs++
	return fmt.Sprintf("3EB0%06d", f.newIDs)
}

// sent returns a copy of the sends and their WhatsApp ids so far.
func (f *fakeClient) sent() (sends, ids []string) {
	f.sendMu.Lock()
	defer f.sendMu.Unlock()
	return append([]string(nil), f.sends...), append([]string(nil), f.sendIDs...)
}

func (f *fakeClient) HasDevice() bool { return f.device }

func (f *fakeClient) Connect() error {
	f.connects++
	if f.connectErr != nil {
		return f.connectErr
	}
	if f.connecting {
		return nil
	}
	f.emit(&waevents.Connected{})
	return nil
}

func (f *fakeClient) Disconnect() { f.disconnects++ }

// MarkRead records a read receipt, so a test can prove which were sent, or that none was.
func (f *fakeClient) MarkRead(ctx context.Context, chat, sender string, ids []string, at time.Time, receiptType string) error {
	if f.onMarkRead != nil {
		f.onMarkRead(ctx)
	}
	f.markReads++
	f.markReadCalls = append(f.markReadCalls, chat+" "+sender+" "+receiptType+" "+strings.Join(ids, ","))
	f.markReadAt = at
	if f.markReadFailAt > 0 && f.markReads != f.markReadFailAt {
		return nil
	}
	return f.markReadErr
}

func (f *fakeClient) ReadReceiptsPrivacy(context.Context) (string, error) {
	f.privacyAsks++
	if f.privacyErr != nil {
		return "", f.privacyErr
	}
	if f.readReceipts == "" {
		return "all", nil
	}
	return f.readReceipts, nil
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
	t.Cleanup(s.Close)
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
	t.Cleanup(s.Close)
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
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.Disconnected{})
	if got := s.State(); got != StateOffline {
		t.Errorf("State() after a disconnect = %q, want %q", got, StateOffline)
	}
	// The reconnect waits for the backoff delay; it never happens on the event itself.
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times right after a disconnect, want 1", fake.connects)
	}
}

func TestSessionFailedConnectReportsOffline(t *testing.T) {
	fake := &fakeClient{device: true, connectErr: errors.New("no network")}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
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
	kr := newTestKeyring()
	s, err := OpenSession(context.Background(), filepath.Join(dataDir, "nuncio.db"), kr)
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
	if n := kr.callCount(); n != 0 {
		t.Errorf("the keyring was called %d times without a database, want never", n)
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
			t.Cleanup(s.Close)
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
				t.Errorf("Connect was called %d times after %T, want 1: a reconnect never happens on the event itself", fake.connects, tc.event)
			}
		})
	}
}

func TestEventsWithNilHandlersIgnoreSessionEndEvents(t *testing.T) {
	var e Events
	for _, ev := range []any{&waevents.LoggedOut{}, &waevents.TemporaryBan{}, &waevents.StreamReplaced{}, &waevents.ClientOutdated{}, &waevents.ConnectFailure{}} {
		e.Handle(ev) // must not panic
	}
}

func TestSessionPublishesEachConnectionChangeOnce(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
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
		`{"v":1,"op":"event_connection","session":"unlinked","stop_reason":"logged_out"}`,
	)
}

func TestSessionWithoutEventsStillRuns(t *testing.T) {
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	t.Cleanup(s.Close)
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

// fakeTimers stands in for time.AfterFunc: it records each scheduled delay and runs a timer only
// when the test fires it, so no test waits real seconds.
type fakeTimers struct {
	delays    []time.Duration
	funcs     []func() // nil once fired or cancelled
	cancelled int
}

func (ft *fakeTimers) after(d time.Duration, f func()) func() bool {
	ft.delays = append(ft.delays, d)
	ft.funcs = append(ft.funcs, f)
	i := len(ft.funcs) - 1
	return func() bool {
		if ft.funcs[i] == nil {
			return false
		}
		ft.funcs[i] = nil
		ft.cancelled++
		return true
	}
}

// pending counts the timers that are scheduled and neither fired nor cancelled.
func (ft *fakeTimers) pending() int {
	n := 0
	for _, f := range ft.funcs {
		if f != nil {
			n++
		}
	}
	return n
}

// fire runs the newest pending timer, as if its delay had passed.
func (ft *fakeTimers) fire(t *testing.T) {
	t.Helper()
	for i := len(ft.funcs) - 1; i >= 0; i-- {
		if f := ft.funcs[i]; f != nil {
			ft.funcs[i] = nil
			f()
			return
		}
	}
	t.Fatal("no reconnect timer is pending")
}

// reconnectingSession is a started, linked session whose backoff runs on a fake clock, seeded
// jitter and fake timers.
func reconnectingSession(t *testing.T, fake *fakeClient) (*Session, *fakeTimers, *time.Time) {
	t.Helper()
	s := NewSession(fake, nil, nil)
	timers := &fakeTimers{}
	now := time.Date(2026, 10, 19, 9, 0, 0, 0, time.UTC)
	s.after = timers.after
	s.logf = t.Logf
	s.backoff.Rand = rand.New(rand.NewPCG(1, 2)).Float64
	s.backoff.Now = func() time.Time { return now }
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	return s, timers, &now
}

// within reports whether d is a jittered delay of step: in [step/2, step).
func within(d, step time.Duration) bool { return d >= step/2 && d < step }

func TestDisconnectReconnectsOnceAfterTheFirstJitteredDelay(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, _ := reconnectingSession(t, fake)
	fake.emit(&waevents.Disconnected{})
	if len(timers.delays) != 1 || !within(timers.delays[0], 2*time.Second) {
		t.Fatalf("scheduled delays = %v, want one in [1s, 2s)", timers.delays)
	}
	if fake.connects != 1 {
		t.Fatalf("Connect was called %d times before the delay passed, want 1", fake.connects)
	}
	timers.fire(t)
	if fake.connects != 2 {
		t.Errorf("Connect was called %d times after the delay, want 2", fake.connects)
	}
	if got := s.State(); got != StateLinked {
		t.Errorf("State() after the reconnect = %q, want %q", got, StateLinked)
	}
	if n := timers.pending(); n != 0 {
		t.Errorf("%d reconnect timers pending after a successful reconnect, want 0", n)
	}
}

func TestFailedStartSchedulesAReconnect(t *testing.T) {
	fake := &fakeClient{device: true, connectErr: errors.New("no network")}
	s := NewSession(fake, nil, nil)
	timers := &fakeTimers{}
	s.after = timers.after
	t.Cleanup(s.Close)
	if err := s.Start(); err == nil {
		t.Fatal("Start returned nil after a failed connect, want the error")
	}
	if len(timers.delays) != 1 || !within(timers.delays[0], 2*time.Second) {
		t.Fatalf("scheduled delays = %v, want one in [1s, 2s)", timers.delays)
	}
	fake.connectErr = nil
	timers.fire(t)
	if fake.connects != 2 || s.State() != StateLinked {
		t.Errorf("after the delay: %d connects, state %q; want 2 and %q", fake.connects, s.State(), StateLinked)
	}
}

func TestRepeatedFailuresGrowTheDelayUpToTheCeiling(t *testing.T) {
	fake := &fakeClient{device: true}
	_, timers, _ := reconnectingSession(t, fake)
	fake.connectErr = errors.New("no network")
	fake.emit(&waevents.Disconnected{})
	for range 12 {
		timers.fire(t) // each attempt fails and schedules the next one
	}
	steps := []time.Duration{2, 4, 8, 16, 32, 64, 128, 256}
	for i, d := range timers.delays {
		step := 5 * time.Minute
		if i < len(steps) {
			step = min(steps[i]*time.Second, step)
		}
		if !within(d, step) {
			t.Errorf("delay %d = %v, want in [%v, %v)", i, d, step/2, step)
		}
	}
	if len(timers.delays) != 13 {
		t.Errorf("%d delays scheduled, want 13 (one per failure)", len(timers.delays))
	}
	if fake.connects != 13 {
		t.Errorf("Connect was called %d times, want 13 (the start and 12 attempts)", fake.connects)
	}
}

// A failed reconnect attempt stays visible: one log line with the attempt number, the next delay
// and the error's type only, never the error's text, which can carry a JID.
func TestAFailedReconnectAttemptIsLoggedOnceWithoutNumbers(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, _ := reconnectingSession(t, fake)
	var lines []string
	s.logf = func(format string, args ...any) { lines = append(lines, fmt.Sprintf(format, args...)) }
	fake.connectErr = errors.New("dial 15551230001@s.whatsapp.net: connection refused")
	fake.emit(&waevents.Disconnected{})
	if len(lines) != 0 {
		t.Fatalf("logged %q on the drop, want nothing before an attempt failed", lines)
	}
	timers.fire(t) // attempt 1 fails
	timers.fire(t) // attempt 2 fails
	want := []string{
		fmt.Sprintf("wa: reconnect attempt 1 failed (*errors.errorString); next attempt in %s", timers.delays[1].Round(time.Millisecond)),
		fmt.Sprintf("wa: reconnect attempt 2 failed (*errors.errorString); next attempt in %s", timers.delays[2].Round(time.Millisecond)),
	}
	if !reflect.DeepEqual(lines, want) {
		t.Errorf("log = %q, want %q", lines, want)
	}
	for _, l := range lines {
		if strings.Contains(l, "1555123") || strings.Contains(l, "whatsapp.net") {
			t.Errorf("log line %q carries the error's JID", l)
		}
	}
	fake.connectErr = nil
	timers.fire(t) // attempt 3 connects
	if len(lines) != 2 {
		t.Errorf("log after a successful attempt = %q, want no new line", lines)
	}
}

func TestStableConnectionRestartsTheDelayAtTheBase(t *testing.T) {
	fake := &fakeClient{device: true}
	_, timers, now := reconnectingSession(t, fake)
	fake.connectErr = errors.New("no network")
	fake.emit(&waevents.Disconnected{})
	timers.fire(t)
	timers.fire(t) // two failures: the next step would be 8 s
	fake.connectErr = nil
	timers.fire(t) // connected again
	*now = now.Add(time.Minute)
	fake.emit(&waevents.Disconnected{})
	last := timers.delays[len(timers.delays)-1]
	if !within(last, 2*time.Second) {
		t.Errorf("delay after a one-minute connection = %v, want in [1s, 2s): a stable connection resets the backoff", last)
	}
}

// assertStopped checks that event ended reconnecting for good with a visible reason.
func assertStopped(t *testing.T, event any, wantState State, wantReason string) {
	t.Helper()
	fake := &fakeClient{device: true}
	s, timers, _ := reconnectingSession(t, fake)
	fake.emit(event)
	fake.emit(&waevents.Disconnected{}) // a later drop must not revive the loop
	if len(timers.delays) != 0 {
		t.Errorf("reconnects scheduled after %T: %v, want none", event, timers.delays)
	}
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times after %T, want 1", fake.connects, event)
	}
	if got := s.State(); got != wantState {
		t.Errorf("State() after %T = %q, want %q", event, got, wantState)
	}
	if got := s.StopReason(); !strings.Contains(got, wantReason) {
		t.Errorf("StopReason() after %T = %q, want it to contain %q", event, got, wantReason)
	}
}

func TestLoggedOutStopsReconnectingAndReportsUnlinked(t *testing.T) {
	assertStopped(t, &waevents.LoggedOut{}, StateUnlinked, "logged out")
}

func TestTemporaryBanStopsReconnectingAndReportsOffline(t *testing.T) {
	assertStopped(t, &waevents.TemporaryBan{}, StateOffline, "banned")
}

// Another connection of this device took the stream over; reconnecting would take it back and
// the two would replace each other in a loop.
func TestStreamReplacedStopsReconnecting(t *testing.T) {
	assertStopped(t, &waevents.StreamReplaced{}, StateOffline, "replaced")
}

func TestClientOutdatedStopsReconnecting(t *testing.T) {
	assertStopped(t, &waevents.ClientOutdated{}, StateOffline, "out of date")
}

func TestRejectedConnectFailureDoesNotReconnect(t *testing.T) {
	for _, r := range []waevents.ConnectFailureReason{waevents.ConnectFailureGeneric, waevents.ConnectFailureBadUserAgent, waevents.ConnectFailureClientUnknown, waevents.ConnectFailureNotFound, waevents.ConnectFailureExperimental} {
		fake := &fakeClient{device: true}
		s, timers, _ := reconnectingSession(t, fake)
		fake.emit(&waevents.ConnectFailure{Reason: r})
		if len(timers.delays) != 0 {
			t.Errorf("ConnectFailure %d: reconnects scheduled %v, stop reason %q; whatsmeow itself never retries this code", r, timers.delays, s.StopReason())
		}
		if s.StopReason() == "" {
			t.Errorf("ConnectFailure %d: StopReason() is empty, want the failure code", r)
		}
		if got := s.State(); got != StateOffline {
			t.Errorf("ConnectFailure %d: State() = %q, want %q", r, got, StateOffline)
		}
	}
}

func TestAStopIsLoggedOnceWithItsReason(t *testing.T) {
	cases := []struct {
		name   string
		event  any
		reason string
	}{
		{"temporary ban", &waevents.TemporaryBan{}, "banned"},
		{"rejected connect failure", &waevents.ConnectFailure{Reason: waevents.ConnectFailureBadUserAgent}, "409"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeClient{device: true}
			s := NewSession(fake, nil, nil)
			var lines []string
			s.logf = func(format string, args ...any) { lines = append(lines, fmt.Sprintf(format, args...)) }
			t.Cleanup(s.Close)
			if err := s.Start(); err != nil {
				t.Fatalf("Start: %v", err)
			}
			fake.emit(tc.event)
			fake.emit(&waevents.StreamReplaced{}) // a second terminal event logs nothing more
			if len(lines) != 1 {
				t.Fatalf("logged %q, want exactly one line", lines)
			}
			if !strings.Contains(lines[0], "stopped reconnecting") || !strings.Contains(lines[0], tc.reason) {
				t.Errorf("log line %q, want it to say it stopped reconnecting and contain %q", lines[0], tc.reason)
			}
			if got := s.StopReason(); !strings.Contains(got, tc.reason) {
				t.Errorf("StopReason() = %q after the second event, want the first reason (%q)", got, tc.reason)
			}
		})
	}
}

func TestOneDropSchedulesOneReconnect(t *testing.T) {
	fake := &fakeClient{device: true}
	_, timers, _ := reconnectingSession(t, fake)
	fake.emit(&waevents.Disconnected{})
	fake.emit(&waevents.Disconnected{})
	if n := timers.pending(); n != 1 {
		t.Errorf("%d reconnect timers pending after one drop reported twice, want 1", n)
	}
}

func TestCloseCancelsAPendingReconnect(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, _ := reconnectingSession(t, fake)
	fake.emit(&waevents.Disconnected{})
	if len(timers.funcs) != 1 {
		t.Fatalf("%d reconnects scheduled after a drop, want 1", len(timers.funcs))
	}
	run := timers.funcs[0]
	s.Close()
	if timers.cancelled != 1 {
		t.Errorf("Close cancelled %d timers, want 1", timers.cancelled)
	}
	run() // a timer that fired while Close ran
	fake.emit(&waevents.Disconnected{})
	if fake.connects != 1 {
		t.Errorf("Connect was called %d times after Close, want 1", fake.connects)
	}
	if len(timers.delays) != 1 {
		t.Errorf("reconnects scheduled after Close: %v, want only the one before it", timers.delays[1:])
	}
}

// The session can never mark the account online: the client it drives has no presence call at
// all (whatsmeow's Connect sends none by itself, see OpenSession).
func TestClientHasNoPresenceCall(t *testing.T) {
	c := reflect.TypeFor[Client]()
	for i := range c.NumMethod() {
		if name := c.Method(i).Name; strings.Contains(name, "Presence") {
			t.Errorf("Client has the method %s; the session must never send presence", name)
		}
	}
}

// With real timers and a millisecond backoff, attempts keep failing until Close; after it the
// count stays still. Run under -race, this also checks the timer goroutine against Close.
func TestNoReconnectAfterCloseWithRealTimers(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	s.backoff.Base, s.backoff.Ceiling = time.Millisecond, 2*time.Millisecond
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	s.connMu.Lock()
	fake.connectErr = errors.New("no network")
	s.connMu.Unlock()
	fake.emit(&waevents.Disconnected{})
	time.Sleep(30 * time.Millisecond)
	s.Close()
	closedAt := fake.connects // Close waited for any attempt in flight
	time.Sleep(30 * time.Millisecond)
	if fake.connects != closedAt {
		t.Errorf("Connect was called %d more times after Close, want 0", fake.connects-closedAt)
	}
	if closedAt < 3 {
		t.Errorf("Connect was called %d times before Close, want the failed attempts to keep retrying", closedAt)
	}
}
