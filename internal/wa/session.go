package wa

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
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
// for good, and StopReason says why; the daemon logs that reason once. Surfacing it to clients (a
// wire field) is a follow-up.
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
	// published is, under mu, the state of the last event_connection published; publishing is set
	// while one caller publishes state frames (publishState).
	published  State
	publishing bool
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
		backoff: NewBackoff(), after: afterFunc, logf: log.Printf, markReadSem: make(chan struct{}, 1)}
}

func afterFunc(d time.Duration, f func()) func() bool { return time.AfterFunc(d, f).Stop }

// StopReason is why the session stopped reconnecting for good (logged out, banned, stream
// replaced, client out of date), or "" while it reconnects.
func (s *Session) StopReason() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.backoff.Stopped()
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
	for s.published != s.state {
		st := s.state
		s.published = st
		b := s.events
		s.mu.Unlock()
		if b != nil {
			b.Publish(ipc.EventConnection{V: ipc.Version, Op: ipc.OpEventConnection, Session: string(st)})
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

// OpenDevice prepares whatsmeow's device store inside db and returns its first device, which has
// a nil ID when nothing is linked yet.
//
// whatsmeow's own sqlstore keeps the full Signal state (sessions, pre-keys, sender keys, app
// state) a linked device needs, inside nuncio.db in the 0700 data directory, until the
// keyring-backed device store replaces it.
func OpenDevice(ctx context.Context, db *sql.DB) (*wastore.Device, error) {
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		return nil, fmt.Errorf("wa: cannot prepare the device store: %w", err)
	}
	device, err := container.GetFirstDevice(ctx)
	if err != nil {
		return nil, fmt.Errorf("wa: cannot read the device store: %w", err)
	}
	return device, nil
}

// OpenSession builds the daemon's session over the database at path. A missing database means
// unlinked, and nothing is created: merely starting the daemon must leave no data dir behind.
func OpenSession(ctx context.Context, path string) (*Session, error) {
	if _, err := os.Stat(path); errors.Is(err, fs.ErrNotExist) {
		return NewSession(nil, nil, nil), nil
	} else if err != nil {
		return nil, fmt.Errorf("wa: %w", err)
	}
	db, err := store.Open(path)
	if err != nil {
		return nil, err
	}
	if err := store.EnsureSchema(db); err != nil {
		_ = db.Close()
		return nil, err
	}
	device, err := OpenDevice(ctx, db)
	if err != nil {
		_ = db.Close()
		return nil, err
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
