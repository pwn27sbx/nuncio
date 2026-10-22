package wa

import (
	"bytes"
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/media"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// State is the session state the daemon reports, with the wire values of proto/wire.md.
type State string

const (
	// StateUnlinked: no device is paired. Nothing connects.
	StateUnlinked State = ipc.SessionUnlinked
	// StateLinked: paired and connected.
	StateLinked State = ipc.SessionLinked
	// StateOffline: paired but not connected (connecting, dropped, or a failed connect).
	StateOffline State = ipc.SessionOffline
)

// Client is the slice of whatsmeow the session drives. It is an interface so the session logic is
// testable with a fake that never opens a socket.
type Client interface {
	// HasDevice reports whether the store holds a linked device.
	HasDevice() bool
	Connect() error
	Disconnect()
	AddEventHandler(func(any))
	// NewMessageID returns a fresh WhatsApp message id. The outbox picks it when a send is queued
	// and every attempt of that send uses it, so a retry is never a second message.
	NewMessageID() string
	// SendMessage sends one built message to the chat jid under the WhatsApp message id (empty
	// lets whatsmeow choose one) and returns WhatsApp's timestamp.
	SendMessage(ctx context.Context, jid string, msg *waE2E.Message, id string) (ts time.Time, err error)
	// BuildReaction builds my reaction (empty emoji removes it) on message id of chat, sent by
	// sender (its JID without device, empty for mine), with whatsmeow's BuildReaction.
	BuildReaction(chat, sender, id, emoji string) (*waE2E.Message, error)
	// OwnJID is this device's account JID without device, empty when nothing is linked.
	OwnJID() string
	// OwnLID is this account's hidden id (LID) without device, from whatsmeow's device store; empty
	// when it is not known.
	OwnLID() string
	// ReadReceiptsPrivacy is the account's read-receipt privacy setting ("all" or "none"), from
	// whatsmeow's TryFetchPrivacySettings (whatsmeow keeps it current from WhatsApp's change
	// notifications). An error means it is unknown.
	ReadReceiptsPrivacy(ctx context.Context) (string, error)
	// MarkRead sends one read receipt of receiptType ("read" or "read-self") for messages ids of
	// chat, all sent by sender (its JID without device), with whatsmeow's MarkRead. Only
	// Session.MarkRead, behind a UI's explicit mark_read, calls it.
	MarkRead(ctx context.Context, chat, sender string, ids []string, at time.Time, receiptType string) error
	// Logout tells WhatsApp to unlink this device, then disconnects. It is the only call that
	// unlinks, and only `nuncio logout --confirm` reaches it.
	Logout(ctx context.Context) error
}

// Session is the daemon's one WhatsApp session. It connects only a device that is already linked
// and never starts a link: with no device it stays unlinked and Connect is never called.
//
// A dropped or failed connection reconnects after the Backoff delay, one pending attempt at a
// time, and each failed attempt is logged once with its number, the next delay and the error's
// type (never its text, which can carry a JID). A logout, a temporary ban, a replaced stream, an
// outdated client or a connection WhatsApp rejects (any other ConnectFailure) stops reconnecting
// for good, and StopReason says why; the daemon logs that reason once. Clients get it as StopKind,
// a fixed word (stop_reason on event_connection and status), never the reason's text.
//
// A connection can also die silently (a NAT timeout or a network change drops the TCP stream with
// no FIN or RST), and then no Disconnected ever arrives. whatsmeow notices only through its
// keepalive pings, and with EnableAutoReconnect off it just reports them (keepalive.go:42-51). The
// session applies whatsmeow's own rule: a KeepAliveTimeout whose last good ping is more than
// whatsmeow.KeepAliveMaxFailTime (3 min) old means the socket is dead, so a linked session goes
// offline, disconnects it, logs it once and reconnects through the same Backoff and stop rules as a
// drop. An earlier timeout (a blip that KeepAliveRestored ends) changes nothing, and so does one
// while the session is already offline, stopped or closed, so there is never a second loop. A late
// timeout from the previous connection (its last good ping predates the latest Connect) is ignored,
// so it never tears a healthy new connection down.
type Session struct {
	client     Client
	db         *sql.DB
	closeStore func() error

	// connMu serialises a reconnect's Connect with Close's Disconnect, so nothing connects again
	// once Close has run.
	connMu sync.Mutex

	mu    sync.Mutex
	state State
	// drops counts, under mu, every time the session left linked (a drop, a dead keepalive, a stop
	// or Close), so a send can tell that its connection died while it waited (see leftLinked).
	drops   uint64
	started bool
	// published and publishedStop are, under mu, the state and stop_reason of the last
	// event_connection published; publishing is set while one caller publishes state frames
	// (publishState).
	published     State
	publishedStop string
	publishing    bool
	// events is where the session publishes (an *ipc.Broker outside tests).
	events publisher
	// unreadChanged, when not nil, runs after every change that may move the total unread count
	// (SetUnreadChanged); it must not block.
	unreadChanged func()
	backoff       *Backoff
	// after schedules f once after d and returns its cancel; time.AfterFunc outside tests.
	after  func(d time.Duration, f func()) (cancel func() bool)
	cancel func() bool // the pending reconnect, nil when none
	// connectAt is when the latest Connect was called (backoff's clock). whatsmeow starts each
	// connection's keepalive loop inside Connect with lastSuccess = now (client.go:598,
	// keepalive.go:31), so a KeepAliveTimeout whose LastSuccess is before connectAt comes from an
	// earlier connection's loop (each is dispatched on its own goroutine) and is ignored.
	connectAt time.Time
	// logf reports a stop, once, and each failed reconnect attempt; log.Printf outside tests.
	logf func(format string, args ...any)
	// media serves DownloadMedia; nil until SetMedia.
	media *media.Cache

	// sendMu serialises the outbox's send attempts, so no entry is ever sent twice at once.
	sendMu sync.Mutex
	// markReadSem (capacity 1) serialises MarkRead, so two UIs opening a chat at once send its
	// receipts once; a call waits for it only inside its own 4 s bound.
	markReadSem chan struct{}
	// drains counts the running outbox drains; Close waits for them before closing the store.
	drains sync.WaitGroup
	// paceAfter schedules the pause between two drained sends (drainPace) like after; a separate
	// field so the outbox never shows up among the reconnect timers. time.AfterFunc outside tests.
	paceAfter func(d time.Duration, f func()) (cancel func() bool)
	// left is, under mu, closed when the session leaves linked and made anew when it becomes linked,
	// so a drain's pause ends as soon as its connection does.
	left chan struct{}
	// mergeFailed holds, under mu, the phone-number chats whose merge into their LID chat failed in
	// this session; they are not tried (or logged) again until the next start.
	mergeFailed map[string]bool
	// groupNames holds, under mu, the group subjects learnt in this session (names.go); groupsSynced
	// is set once the one joined-groups query of this Start was started.
	groupNames   map[string]string
	groupsSynced bool
}

// publisher is the slice of *ipc.Broker the session publishes to; it never blocks.
type publisher interface{ Publish(frame any) int }

// NewSession wraps client, which may be nil for "no device store at all". db is the message
// store that incoming messages are saved to, nil for none. closeStore, when not nil, runs on Close
// after the client is disconnected.
func NewSession(client Client, db *sql.DB, closeStore func() error) *Session {
	return &Session{client: client, db: db, closeStore: closeStore, state: StateUnlinked, published: StateUnlinked,
		backoff: NewBackoff(), after: afterFunc, paceAfter: afterFunc, logf: log.Printf, markReadSem: make(chan struct{}, 1)}
}

func afterFunc(d time.Duration, f func()) func() bool { return time.AfterFunc(d, f).Stop }

// StopReason is why the session stopped reconnecting for good (logged out, banned, stream
// replaced, client out of date), or "" while it reconnects.
func (s *Session) StopReason() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.backoff.Stopped()
}

// StopKind is the stop reason as clients get it (status and event_connection's stop_reason): one
// of the fixed ipc.Stop* values, or "" while the session reconnects.
func (s *Session) StopKind() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return stopKind(s.backoff.Stopped())
}

// stopKind maps a reason given to stop (Start's handlers, Logout) to its ipc.Stop* value, so no
// text of WhatsApp's (ban codes, expiry, failure codes) ever reaches a client; a reason it does
// not know is ipc.StopOther. The prefixes are the reasons as Events and Logout write them.
func stopKind(reason string) string {
	switch {
	case reason == "":
		return ""
	case strings.HasPrefix(reason, "logged out"):
		return ipc.StopLoggedOut
	case strings.Contains(reason, "temporarily banned"):
		return ipc.StopTemporaryBan
	case strings.HasPrefix(reason, "stream replaced"):
		return ipc.StopStreamReplaced
	case strings.HasPrefix(reason, "client is out of date"):
		return ipc.StopClientOutdated
	case strings.HasPrefix(reason, "connect rejected"):
		return ipc.StopConnectRejected
	}
	return ipc.StopOther
}

// SetEvents makes the session publish its events to b: event_connection when the state changes,
// event_message for every newly stored message and event_unread when an unread count changes.
// Call it before Start.
func (s *Session) SetEvents(b *ipc.Broker) {
	s.mu.Lock()
	s.events = nil
	if b != nil {
		s.events = b
	}
	s.mu.Unlock()
}

// SetUnreadChanged makes the session call f after every change that may move the total unread
// count: an incoming message stored, a chat read here (mark_read) or on another device (app
// state). f must return at once; the daemon's D-Bus face recounts and emits on its own goroutine.
// Call it before Start.
func (s *Session) SetUnreadChanged(f func()) {
	s.mu.Lock()
	s.unreadChanged = f
	s.mu.Unlock()
}

// publish sends frame to the subscribers, if the session has a broker.
func (s *Session) publish(frame any) {
	s.mu.Lock()
	b := s.events
	s.mu.Unlock()
	if b != nil {
		b.Publish(frame)
	}
}

// Store is the message store, or nil when there is no database (unlinked).
func (s *Session) Store() *sql.DB { return s.db }

// Start connects a stored device. It returns the connect error, if any, leaves the session
// offline and schedules a reconnect after the first backoff delay.
func (s *Session) Start() error {
	if s.client == nil || !s.client.HasDevice() {
		s.set(StateUnlinked)
		return nil
	}
	s.mu.Lock()
	s.started = true
	s.mu.Unlock()
	// Subscribe before Connect, and count as offline until the Connected event arrives.
	events := Events{
		Connected:        s.connected,
		Disconnected:     s.dropped,
		KeepAliveTimeout: s.keepAliveFailed,
		LoggedOut:        func() { s.stop("logged out", StateUnlinked) },
		// A replaced stream means another connection of this same device (a second nunciod, or
		// `nuncio link` reusing the device) took the session over. Reconnecting would take it back,
		// the other side would do the same, and the two would replace each other in a loop: stop
		// and say why instead.
		Stopped: func(reason string) { s.stop(reason, StateOffline) },
	}
	if s.db != nil {
		// A store split before ADR 0001 is merged before anything is served or received.
		s.mergeKnownAliases()
		s.refreshStoredNames()
		events.Message = s.storeMessage
		events.ContactName = s.contactChanged
		events.GroupName = s.groupNamed
		events.Pin = s.applyPin
		events.Archive = s.applyArchive
		events.Mute = s.applyMute
		events.MarkChatAsRead = s.applyMarkChatAsRead
	}
	s.client.AddEventHandler(events.Handle)
	s.set(StateOffline)
	s.markConnect()
	if err := s.client.Connect(); err != nil {
		s.offlineUnlessStopped()
		s.reconnectLater()
		return fmt.Errorf("wa: connecting the linked device: %w", err)
	}
	return nil
}

// connected records a connection that came up; one that lasts resets the backoff. With a store,
// it starts the outbox's one drain pass for this connection, off whatsmeow's event goroutine. After
// a stop or Close it changes nothing, checked under the lock that sets linked, so a late Connected
// never overwrites the stop's state.
func (s *Session) connected() {
	s.mu.Lock()
	act := s.started && s.backoff.Stopped() == ""
	if act {
		s.backoff.Connected()
		s.setLocked(StateLinked)
	}
	s.mu.Unlock()
	if !act {
		return
	}
	s.publishState()
	s.syncGroupNamesOnce()
	if s.db != nil {
		s.drains.Add(1)
		go func() {
			defer s.drains.Done()
			s.drain()
		}()
	}
}

// dropped handles a lost or failed connection: offline, and one reconnect after the backoff
// delay. After a stop it changes nothing, so the stop's state and reason stay visible.
func (s *Session) dropped() {
	if s.offlineUnlessStopped() {
		s.reconnectLater()
	}
}

// offlineUnlessStopped reports offline unless the session is stopped, checked under the same lock
// that sets the state, so a stop racing it keeps its own state. It reports whether it acted.
func (s *Session) offlineUnlessStopped() bool {
	s.mu.Lock()
	act := s.backoff.Stopped() == ""
	if act {
		s.setLocked(StateOffline)
	}
	s.mu.Unlock()
	if act {
		s.publishState()
	}
	return act
}

// keepAliveFailed handles whatsmeow's KeepAliveTimeout (see the Session doc). Only a linked,
// started, unstopped session with no reconnect pending acts, and it flips to offline under the same
// lock that checks this, so concurrent timeouts act once.
func (s *Session) keepAliveFailed(lastSuccess time.Time) {
	s.mu.Lock()
	gap := s.backoff.Now().Sub(lastSuccess)
	act := gap > whatsmeow.KeepAliveMaxFailTime && !lastSuccess.Before(s.connectAt) && s.started && s.state == StateLinked &&
		s.backoff.Stopped() == "" && s.cancel == nil
	if act {
		s.setLocked(StateOffline)
	}
	logf := s.logf
	s.mu.Unlock()
	if !act {
		return
	}
	s.publishState()
	logf("wa: keepalive failed for %s; dropping the connection and reconnecting", gap.Round(time.Second))
	// whatsmeow's Disconnect marks the disconnect as expected, so it emits no Disconnected event
	// (client.go:695-704, :608-618); the reconnect is scheduled here instead.
	s.connMu.Lock()
	s.client.Disconnect()
	s.connMu.Unlock()
	s.reconnectLater()
}

// markConnect records that a Connect is about to start (see connectAt).
func (s *Session) markConnect() {
	s.mu.Lock()
	s.connectAt = s.backoff.Now()
	s.mu.Unlock()
}

// stop ends reconnecting for good, cancels a pending reconnect, logs the reason once and reports
// st. A later terminal event still sets its state but keeps the first reason and logs nothing.
func (s *Session) stop(reason string, st State) {
	s.mu.Lock()
	first := s.backoff.Stopped() == ""
	if first {
		s.backoff.Stop(reason)
	}
	cancel := s.cancel
	s.cancel = nil
	logf := s.logf
	s.mu.Unlock()
	if cancel != nil {
		cancel()
	}
	if first {
		// reason carries whatsmeow's codes and ban expiry only: no JID, number or message text.
		logf("wa: stopped reconnecting: %s", reason)
	}
	s.set(st)
}

// reconnectLater schedules one reconnect after the backoff delay, unless the session is closed,
// stopped, or already has one pending (one drop can arrive as several events). It reports the
// delay, and ok false when it scheduled nothing.
func (s *Session) reconnectLater() (delay time.Duration, ok bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.started || s.cancel != nil {
		return 0, false
	}
	delay, ok = s.backoff.Next()
	if !ok {
		return 0, false
	}
	s.cancel = s.after(delay, s.reconnect)
	return delay, true
}

// reconnect is the timer's attempt; a failed one is logged once and schedules the next.
func (s *Session) reconnect() {
	s.connMu.Lock()
	defer s.connMu.Unlock()
	s.mu.Lock()
	s.cancel = nil
	run := s.started && s.backoff.Stopped() == ""
	// The attempts since the last stable connection: this one is the latest Next handed out.
	attempt := s.backoff.attempt
	logf := s.logf
	s.mu.Unlock()
	if !run {
		return
	}
	s.markConnect()
	if err := s.client.Connect(); err != nil {
		// The error's type only: whatsmeow's text can carry a JID.
		if delay, ok := s.reconnectLater(); ok {
			logf("wa: reconnect attempt %d failed (%s); next attempt in %s", attempt, ErrKind(err), delay.Round(time.Millisecond))
		} else {
			logf("wa: reconnect attempt %d failed (%s); no further attempt", attempt, ErrKind(err))
		}
	}
}

// State is the current session state; safe for concurrent use.
func (s *Session) State() State {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state
}

func (s *Session) set(st State) {
	s.mu.Lock()
	s.setLocked(st)
	s.mu.Unlock()
	s.publishState()
}

// setLocked sets the state; the caller holds mu and calls publishState after releasing it.
func (s *Session) setLocked(st State) {
	if s.state == StateLinked && st != StateLinked {
		s.drops++
		close(s.left)
	}
	if s.state != StateLinked && st == StateLinked {
		s.left = make(chan struct{})
	}
	s.state = st
}

// publishState publishes event_connection until the last state published is the current one. One
// caller publishes at a time, outside mu (Publish never blocks); a caller that changes the state
// meanwhile returns at once and the publishing one picks its change up. So subscribers get the
// changes in order, each frame differs from the one before, and the last one is always the
// session's final state, even when a logout lands between a drop's offline and its publish. A
// change undone before it was published (offline then linked again) may never be published. A
// Publish that panics (whatsmeow recovers its handlers' panics) loses only its own frame: the flag
// is cleared on the way out, so the next change is published again.
func (s *Session) publishState() {
	s.mu.Lock()
	if s.publishing {
		s.mu.Unlock()
		return
	}
	s.publishing = true
	done := false
	defer func() {
		if !done { // Publish panicked, outside mu
			s.mu.Lock()
			s.publishing = false
			s.mu.Unlock()
		}
	}()
	// A stop that keeps the state (a ban while offline) still differs by its stop_reason.
	for s.published != s.state || s.publishedStop != stopKind(s.backoff.Stopped()) {
		st, stop := s.state, stopKind(s.backoff.Stopped())
		s.published, s.publishedStop = st, stop
		b := s.events
		s.mu.Unlock()
		if b != nil {
			b.Publish(ipc.EventConnection{V: ipc.Version, Op: ipc.OpEventConnection, Session: string(st), StopReason: stop})
		}
		s.mu.Lock()
	}
	// Cleared under the same lock as the last check, so a change made after it publishes itself.
	s.publishing = false
	done = true
	s.mu.Unlock()
}

// Close cancels a pending reconnect, disconnects a started session, waits for a running outbox
// drain or group-subject query and closes the device store.
// Nothing reconnects after it, and a linked session reports offline from then on (an unlinked
// one stays unlinked).
func (s *Session) Close() {
	s.mu.Lock()
	started := s.started
	s.started = false
	cancel := s.cancel
	s.cancel = nil
	s.mu.Unlock()
	if cancel != nil {
		cancel()
	}
	if started {
		s.connMu.Lock()
		s.client.Disconnect()
		s.connMu.Unlock()
	}
	s.mu.Lock()
	if s.state == StateLinked {
		s.setLocked(StateOffline)
	}
	s.mu.Unlock()
	s.publishState()
	// A running drain finishes (its sends fail fast once disconnected) before the store closes.
	s.drains.Wait()
	if s.closeStore != nil {
		_ = s.closeStore()
		s.closeStore = nil
	}
}

// DBPath is `$XDG_DATA_HOME/nuncio/nuncio.db` (default `~/.local/share`). It creates nothing.
func DBPath() (string, error) {
	base := os.Getenv("XDG_DATA_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("cannot find a data directory: %w", err)
		}
		base = filepath.Join(home, ".local", "share")
	}
	return filepath.Join(base, "nuncio", "nuncio.db"), nil
}

// ErrDeviceUnreadable reports a stored device that cannot be opened: its data key is gone from the
// keyring, the keyring holds another key, or its sealed row does not open (altered bytes). nuncio
// never creates a key over sealed data and never reads a sealed value as plain (ADR 0003, 12-13).
var ErrDeviceUnreadable = errors.New("the stored device cannot be read; run nuncio purge --confirm, then nuncio link")

// ErrCleanupPending reports a migration that sealed the database but could not yet wipe the old
// plain keys from its free pages (store.MigrationResult.CleanupPending). The device is not opened
// until a later call finishes that cleanup.
var ErrCleanupPending = errors.New("wa: the old plain keys are not wiped from the database's free pages yet")

// OpenDevice opens the WhatsApp device kept in db (the database at dbPath) with its private
// material sealed under the data key in kr (ADR 0003), in this order:
//
//  1. Without a stored device, the daemon (forLink false) opens nothing: it returns a nil device
//     and never asks the keyring, so it never creates a key.
//  2. A keyring that cannot be read stops everything before anything is written:
//     store.ErrKeyringUnavailable.
//  3. whatsmeow's schema upgrade, then store.MigrateToSealed, which backs a plain database up and
//     seals it in place; its result is returned. A migration whose free-page cleanup is still
//     pending is ErrCleanupPending: the device is not opened.
//  4. The data key, never created over sealed data: a stored device whose key is missing or
//     invalid, or whose seal check row is missing or was sealed under another key, is
//     ErrDeviceUnreadable.
//  5. The sealed device store, and the stored device, or for a link (forLink true) without one a
//     new unpaired device. Nothing of a new device is written until pairing saves it: only then
//     does its first save create the data key (when the keyring has none), read it back, write
//     the seal check row, read the keyring again and compare (so a key another process put there
//     meanwhile is caught) and save the device. A link that never pairs leaves no key and no row.
func OpenDevice(ctx context.Context, db *sql.DB, dbPath string, kr store.Keyring, forLink bool) (*wastore.Device, store.MigrationResult, error) {
	var res store.MigrationResult
	stored, err := HasStoredDevice(ctx, db)
	if err != nil {
		return nil, res, err
	}
	if !stored && !forLink {
		return nil, res, nil
	}
	// A read only, so a locked keyring stops the start before the upgrade, the migration's
	// backup or anything else writes.
	if _, err := store.LoadDataKey(kr, false); errors.Is(err, store.ErrKeyringUnavailable) {
		return nil, res, fmt.Errorf("wa: %w", err)
	}
	if err := sqlstore.NewWithDB(db, "sqlite3", nil).Upgrade(ctx); err != nil {
		return nil, res, fmt.Errorf("wa: cannot prepare the device store: %w", err)
	}
	res, err = store.MigrateToSealed(ctx, db, dbPath, kr)
	if err != nil {
		if res.CleanupPending {
			return nil, res, fmt.Errorf("%w: %w", ErrCleanupPending, err)
		}
		return nil, res, fmt.Errorf("wa: cannot seal the device store: %w", err)
	}
	if res.CleanupPending {
		return nil, res, ErrCleanupPending
	}

	key, err := store.LoadDataKey(kr, false)
	create := false
	switch {
	case err == nil:
	case errors.Is(err, store.ErrKeyringUnavailable):
		return nil, res, fmt.Errorf("wa: %w", err)
	case stored || !errors.Is(err, store.ErrDataKeyMissing):
		return nil, res, fmt.Errorf("%w: %w", ErrDeviceUnreadable, err)
	default:
		// A link over a database with no device: a new key, kept only by the first save.
		key, create = store.NewDataKey(), true
	}
	sealer, err := store.NewSealer(key)
	if err != nil {
		return nil, res, fmt.Errorf("%w: %w", ErrDeviceUnreadable, err)
	}
	switch err := store.VerifySealCheck(ctx, db, sealer); {
	case err == nil:
	case errors.Is(err, store.ErrSealCheckMissing) && !stored:
		// Written by the first save.
	case errors.Is(err, store.ErrSealCheckMissing), errors.Is(err, store.ErrWrongDataKey):
		return nil, res, fmt.Errorf("%w: %w", ErrDeviceUnreadable, err)
	default:
		return nil, res, fmt.Errorf("wa: %w", err)
	}
	devices, err := store.NewDeviceStore(ctx, db, sealer)
	if err != nil {
		return nil, res, fmt.Errorf("wa: cannot prepare the device store: %w", err)
	}
	if stored {
		device, err := devices.LoadDevice(ctx)
		switch {
		case err == nil:
			return device, res, nil
		case !errors.Is(err, store.ErrNoDevice):
			return nil, res, fmt.Errorf("%w: %w", ErrDeviceUnreadable, err)
		case !forLink:
			return nil, res, nil
		}
	}
	device := devices.NewDevice()
	device.Container = &firstSave{devices: devices, db: db, kr: kr, sealer: sealer, key: key, create: create}
	return device, res, nil
}

// HasStoredDevice reports whether db holds a whatsmeow_device row. It only reads, and never asks
// the keyring: without a stored device OpenSessionOn never asks it either.
func HasStoredDevice(ctx context.Context, db *sql.DB) (bool, error) {
	var stored bool
	err := db.QueryRowContext(ctx, `SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'whatsmeow_device'`).Scan(&stored)
	if err == nil && stored {
		err = db.QueryRowContext(ctx, `SELECT EXISTS (SELECT 1 FROM whatsmeow_device)`).Scan(&stored)
	}
	if err != nil {
		return false, fmt.Errorf("wa: cannot read the device store: %w", err)
	}
	return stored, nil
}

// firstSave is the container of a new device until pairing saves it (OpenDevice, step 5). Its
// first PutDevice keeps the data key and the seal check row; from then on DeviceStore is the
// device's container (PutDevice wires it).
type firstSave struct {
	devices *store.DeviceStore
	db      *sql.DB
	kr      store.Keyring
	sealer  *store.Sealer
	key     []byte
	create  bool // the keyring holds no data key yet

	mu    sync.Mutex
	ready bool
}

var _ wastore.DeviceContainer = (*firstSave)(nil)

func (f *firstSave) PutDevice(ctx context.Context, device *wastore.Device) error {
	if device == nil || device.ID == nil {
		// Nothing is kept for a device that is not paired yet.
		return sqlstore.ErrDeviceIDMustBeSet
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if !f.ready {
		if err := f.keepKey(ctx); err != nil {
			return err
		}
		f.ready = true
	}
	return f.devices.PutDevice(ctx, device)
}

func (f *firstSave) DeleteDevice(ctx context.Context, device *wastore.Device) error {
	return f.devices.DeleteDevice(ctx, device)
}

// keepKey writes the data key (when it is new) and reads it back, writes the seal check row, and
// reads the keyring again: the device is saved only under the key the keyring holds.
func (f *firstSave) keepKey(ctx context.Context) error {
	if f.create {
		if err := store.StoreDataKey(f.kr, f.key); err != nil {
			return fmt.Errorf("wa: cannot keep the data key in the keyring; nothing was saved: %w", err)
		}
	}
	if err := store.WriteSealCheck(ctx, f.db, f.sealer); err != nil {
		if f.create {
			_ = store.ForgetDataKey(f.kr, f.key)
		}
		return fmt.Errorf("wa: cannot write the seal check; nothing was saved: %w", err)
	}
	back, err := store.LoadDataKey(f.kr, false)
	if err != nil {
		return fmt.Errorf("wa: cannot read the data key back; nothing was saved: %w", err)
	}
	defer clear(back)
	if !bytes.Equal(back, f.key) {
		return errors.New("wa: the keyring no longer holds the data key the device would be sealed under; nothing was saved")
	}
	return nil
}

// OpenSession builds the daemon's session over the database at path, its device sealed under the
// data key in kr. A missing database means unlinked, and nothing is created: merely starting the
// daemon must leave no data dir behind. On an error the database is closed again.
func OpenSession(ctx context.Context, path string, kr store.Keyring) (*Session, error) {
	if _, err := os.Stat(path); errors.Is(err, fs.ErrNotExist) {
		return NewSession(nil, nil, nil), nil
	} else if err != nil {
		return nil, fmt.Errorf("wa: %w", err)
	}
	db, err := store.Open(path)
	if err != nil {
		return nil, err
	}
	s, err := OpenSessionOn(ctx, db, path, kr)
	if err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

// OpenSessionOn is OpenSession over db, already opened at path, which it leaves open on an error
// (store.ErrKeyringUnavailable, ErrCleanupPending, ErrDeviceUnreadable or another), so the caller
// can serve what the store holds and try again. Without a stored device the session is unlinked
// over db's message store, and the keyring is never asked. The returned session closes db.
func OpenSessionOn(ctx context.Context, db *sql.DB, path string, kr store.Keyring) (*Session, error) {
	device, res, err := OpenDevice(ctx, db, path, kr, false)
	logMigration(res)
	if err != nil {
		return nil, err
	}
	if err := store.EnsureSchema(db); err != nil {
		return nil, err
	}
	if device == nil {
		return NewSession(nil, db, db.Close), nil
	}
	client := whatsmeow.NewClient(device, nil)
	// whatsmeow reconnects on its own by default, with no ceiling and no stop on bans; the
	// Session's capped Backoff replaces that loop. whatsmeow's Connect sends no presence by itself
	// (only an explicit SendPresence does, and Client has no such call), so the account is never
	// marked online on connect.
	client.EnableAutoReconnect = false
	// Normal delivery receipts (two grey ticks for the sender) without marking the account online.
	// By default a client never marked online sends type="inactive" delivery receipts, which the
	// official apps do not render (receipt.go:240-254, :287-288). SetForceActiveDeliveryReceipts
	// only stores 2 in the sendActiveReceipts atomic (receipt.go:255-264); that atomic is read only
	// to pick the receipt type (receipt.go:287) and otherwise touched only by SendPresence
	// (presence.go:70-75), which nuncio never calls. It sends no presence node or anything else, and
	// read (blue) receipts still go only through MarkRead.
	client.SetForceActiveDeliveryReceipts(true)
	// whatsmeow keeps the pins, archives and mutes of the initial full app-state sync in its own
	// tables and emits no event for them unless asked; nuncio applies them like any later change.
	client.EmitAppStateEventsOnFullSync = true
	s := NewSession(whatsmeowClient{client}, db, db.Close)
	// Attachments are downloaded only on a download_media request, by whatsmeow's Download
	// (which fetches, decrypts and checks the hashes), into $XDG_DATA_HOME/nuncio/media.
	s.SetMedia(filepath.Join(filepath.Dir(path), store.MediaDir), client)
	return s, nil
}

// logMigration logs what a migration did, in fixed words: the backup by its file name only (a
// time, no user data), never its directory, which names the user's home.
func logMigration(res store.MigrationResult) {
	if res.Migrated {
		log.Printf("sealed the WhatsApp keys in nuncio.db; the backup %s next to it still holds them in plain: delete it once nuncio works", filepath.Base(res.BackupPath))
	}
	if res.Cleaned {
		log.Print("wiped the old plain keys from nuncio.db's free pages")
	}
	if res.CleanupPending {
		log.Print("the old plain keys may still be in nuncio.db's free pages; the wipe is tried again before the device opens")
	}
}

// whatsmeowClient adapts *whatsmeow.Client to Client, LIDMapper and Contacts.
type whatsmeowClient struct{ c *whatsmeow.Client }

var (
	_ LIDMapper = whatsmeowClient{}
	_ Contacts  = whatsmeowClient{}
)

func (w whatsmeowClient) HasDevice() bool                  { return w.c.Store.ID != nil }
func (w whatsmeowClient) Connect() error                   { return w.c.Connect() }
func (w whatsmeowClient) Disconnect()                      { w.c.Disconnect() }
func (w whatsmeowClient) AddEventHandler(h func(any))      { w.c.AddEventHandler(h) }
func (w whatsmeowClient) Logout(ctx context.Context) error { return w.c.Logout(ctx) }

func (w whatsmeowClient) NewMessageID() string { return w.c.GenerateMessageID() }

// SendMessage sends msg under the message id the outbox chose.
func (w whatsmeowClient) SendMessage(ctx context.Context, jid string, msg *waE2E.Message, id string) (time.Time, error) {
	to, err := types.ParseJID(jid)
	if err != nil {
		return time.Time{}, fmt.Errorf("wa: %w", err)
	}
	resp, err := w.c.SendMessage(ctx, to, msg, whatsmeow.SendRequestExtra{ID: id})
	if err != nil {
		return time.Time{}, fmt.Errorf("wa: sending a message: %w", err)
	}
	return resp.Timestamp, nil
}

// BuildReaction is whatsmeow's BuildReaction over parsed JIDs; an empty sender is me. A JID that
// does not parse answers a fixed error: ParseJID's text can quote part of the JID, and this error
// reaches the client.
func (w whatsmeowClient) BuildReaction(chat, sender, id, emoji string) (*waE2E.Message, error) {
	c, err := types.ParseJID(chat)
	if err != nil {
		return nil, errors.New("wa: the chat JID does not parse")
	}
	var from types.JID
	if sender != "" {
		if from, err = types.ParseJID(sender); err != nil {
			return nil, errors.New("wa: the sender JID does not parse")
		}
	}
	return w.c.BuildReaction(c, from, id, emoji), nil
}

// ReadReceiptsPrivacy uses TryFetchPrivacySettings, not GetPrivacySettings: the latter answers an
// empty setting when the fetch fails, and whatsmeow's MarkRead would then send blue ticks the user
// turned off.
func (w whatsmeowClient) ReadReceiptsPrivacy(ctx context.Context) (string, error) {
	settings, err := w.c.TryFetchPrivacySettings(ctx, false)
	if err != nil {
		return "", fmt.Errorf("wa: reading the privacy settings: %w", err)
	}
	return string(settings.ReadReceipts), nil
}

// MarkRead is whatsmeow's MarkRead over parsed JIDs, with the receipt type given explicitly. A JID
// that does not parse answers a fixed error, which names no JID.
func (w whatsmeowClient) MarkRead(ctx context.Context, chat, sender string, ids []string, at time.Time, receiptType string) error {
	c, err := types.ParseJID(chat)
	if err != nil {
		return errors.New("wa: the chat JID does not parse")
	}
	from, err := types.ParseJID(sender)
	if err != nil {
		return errors.New("wa: the sender JID does not parse")
	}
	msgIDs := make([]types.MessageID, len(ids))
	for i, id := range ids {
		msgIDs[i] = id
	}
	if err := w.c.MarkRead(ctx, msgIDs, at, c, from, types.ReceiptType(receiptType)); err != nil {
		return fmt.Errorf("wa: sending a read receipt: %w", err)
	}
	return nil
}

func (w whatsmeowClient) OwnJID() string {
	if w.c.Store.ID == nil {
		return ""
	}
	return w.c.Store.ID.ToNonAD().String()
}

// LIDForPN and PNForLID read whatsmeow's LID map (Store.LIDs: GetLIDForPN, GetPNForLID), which
// whatsmeow fills from incoming messages, usync answers and notifications and keeps in
// whatsmeow_lid_map. An empty JID means the pair is not known.
func (w whatsmeowClient) LIDForPN(ctx context.Context, pn string) (string, error) {
	jid, err := types.ParseJID(pn)
	if err != nil {
		return "", errors.New("wa: the chat JID does not parse")
	}
	lid, err := w.c.Store.LIDs.GetLIDForPN(ctx, jid.ToNonAD())
	if err != nil {
		return "", fmt.Errorf("wa: reading the LID of a phone number: %w", err)
	}
	if lid.IsEmpty() {
		return "", nil
	}
	return lid.ToNonAD().String(), nil
}

func (w whatsmeowClient) PNForLID(ctx context.Context, lid string) (string, error) {
	jid, err := types.ParseJID(lid)
	if err != nil {
		return "", errors.New("wa: the chat JID does not parse")
	}
	pn, err := w.c.Store.LIDs.GetPNForLID(ctx, jid.ToNonAD())
	if err != nil {
		return "", fmt.Errorf("wa: reading the phone number of a LID: %w", err)
	}
	if pn.IsEmpty() {
		return "", nil
	}
	return pn.ToNonAD().String(), nil
}

func (w whatsmeowClient) OwnLID() string {
	lid := w.c.Store.GetLID()
	if lid.IsEmpty() {
		return ""
	}
	return lid.ToNonAD().String()
}

// Contact reads whatsmeow's contact store (Store.Contacts.GetContact: its cache, else one SQLite
// row of whatsmeow_contacts; never the network). An unknown contact is the zero Contact.
func (w whatsmeowClient) Contact(ctx context.Context, jid string) (Contact, error) {
	user, err := types.ParseJID(jid)
	if err != nil {
		return Contact{}, errors.New("wa: the contact JID does not parse")
	}
	if w.c.Store.Contacts == nil {
		return Contact{}, nil
	}
	info, err := w.c.Store.Contacts.GetContact(ctx, user.ToNonAD())
	if err != nil {
		return Contact{}, fmt.Errorf("wa: reading a contact: %w", err)
	}
	return Contact{FullName: info.FullName, FirstName: info.FirstName, BusinessName: info.BusinessName,
		PushName: info.PushName}, nil
}

// JoinedGroups is whatsmeow's GetJoinedGroups: one query for every group the account is in, read
// here for its subjects only.
func (w whatsmeowClient) JoinedGroups(ctx context.Context) (map[string]string, error) {
	groups, err := w.c.GetJoinedGroups(ctx)
	if err != nil {
		return nil, fmt.Errorf("wa: querying the joined groups: %w", err)
	}
	out := make(map[string]string, len(groups))
	for _, g := range groups {
		out[g.JID.String()] = g.Name
	}
	return out, nil
}
