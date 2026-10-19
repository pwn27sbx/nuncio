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
	"google.golang.org/protobuf/proto"

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
	// SendText sends one text message to the chat jid under the WhatsApp message id and returns
	// WhatsApp's timestamp.
	SendText(ctx context.Context, jid, text, id string) (ts time.Time, err error)
	// Logout tells WhatsApp to unlink this device, then disconnects. It is the only call that
	// unlinks, and only `nuncio logout --confirm` reaches it.
	Logout(ctx context.Context) error
}

// Session is the daemon's one WhatsApp session. It connects only a device that is already linked
// and never starts a link: with no device it stays unlinked and Connect is never called.
//
// A dropped or failed connection reconnects after the Backoff delay, one pending attempt at a
// time. A logout, a temporary ban, a replaced stream or an outdated client stops reconnecting for
// good, and StopReason says why; the daemon logs that reason once. Surfacing it to clients (a
// wire field) is a follow-up.
type Session struct {
	client     Client
	db         *sql.DB
	closeStore func() error

	// connMu serialises a reconnect's Connect with Close's Disconnect, so nothing connects again
	// once Close has run.
	connMu sync.Mutex

	mu      sync.Mutex
	state   State
	started bool
	events  *ipc.Broker
	backoff *Backoff
	// after schedules f once after d and returns its cancel; time.AfterFunc outside tests.
	after  func(d time.Duration, f func()) (cancel func() bool)
	cancel func() bool // the pending reconnect, nil when none
	// logf reports a stop, once; log.Printf outside tests.
	logf func(format string, args ...any)
	// media serves DownloadMedia; nil until SetMedia.
	media *media.Cache

	// sendMu serialises the outbox's send attempts, so no entry is ever sent twice at once.
	sendMu sync.Mutex
	// drains counts the running outbox drains; Close waits for them before closing the store.
	drains sync.WaitGroup
}

// NewSession wraps client, which may be nil for "no device store at all". db is the message
// store that incoming messages are saved to, nil for none. closeStore, when not nil, runs on Close
// after the client is disconnected.
func NewSession(client Client, db *sql.DB, closeStore func() error) *Session {
	return &Session{client: client, db: db, closeStore: closeStore, state: StateUnlinked,
		backoff: NewBackoff(), after: afterFunc, logf: log.Printf}
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
	s.events = b
	s.mu.Unlock()
}

// publish sends frame to the subscribers, if the session has a broker.
func (s *Session) publish(frame any) {
	s.mu.Lock()
	b := s.events
	s.mu.Unlock()
	b.Publish(frame)
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
		Connected:    s.connected,
		Disconnected: s.dropped,
		LoggedOut:    func() { s.stop("logged out", StateUnlinked) },
		// A replaced stream means another connection of this same device (a second nunciod, or
		// `nuncio link` reusing the device) took the session over. Reconnecting would take it back,
		// the other side would do the same, and the two would replace each other in a loop: stop
		// and say why instead.
		Stopped: func(reason string) { s.stop(reason, StateOffline) },
	}
	if s.db != nil {
		events.Message = s.storeMessage
		events.Pin = s.applyPin
		events.Archive = s.applyArchive
		events.Mute = s.applyMute
		events.MarkChatAsRead = s.applyMarkChatAsRead
	}
	s.client.AddEventHandler(events.Handle)
	s.set(StateOffline)
	if err := s.client.Connect(); err != nil {
		s.set(StateOffline)
		s.reconnectLater()
		return fmt.Errorf("wa: connecting the linked device: %w", err)
	}
	return nil
}

// connected records a connection that came up; one that lasts resets the backoff. With a store,
// it starts the outbox's one drain pass for this connection, off whatsmeow's event goroutine.
func (s *Session) connected() {
	s.mu.Lock()
	s.backoff.Connected()
	s.mu.Unlock()
	s.set(StateLinked)
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
	s.mu.Lock()
	stopped := s.backoff.Stopped() != ""
	s.mu.Unlock()
	if stopped {
		return
	}
	s.set(StateOffline)
	s.reconnectLater()
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
// stopped, or already has one pending (one drop can arrive as several events).
func (s *Session) reconnectLater() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.started || s.cancel != nil {
		return
	}
	delay, ok := s.backoff.Next()
	if !ok {
		return
	}
	s.cancel = s.after(delay, s.reconnect)
}

// reconnect is the timer's attempt; a failed one schedules the next.
func (s *Session) reconnect() {
	s.connMu.Lock()
	defer s.connMu.Unlock()
	s.mu.Lock()
	s.cancel = nil
	run := s.started && s.backoff.Stopped() == ""
	s.mu.Unlock()
	if !run {
		return
	}
	if err := s.client.Connect(); err != nil {
		s.reconnectLater()
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
	changed := s.state != st
	s.state = st
	s.mu.Unlock()
	if changed {
		s.publish(ipc.EventConnection{V: ipc.Version, Op: ipc.OpEventConnection, Session: string(st)})
	}
}

// Close cancels a pending reconnect, disconnects a started session, waits for a running outbox
// drain and closes the device store.
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
	linked := s.state == StateLinked
	s.mu.Unlock()
	if linked {
		s.set(StateOffline)
	}
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
	// whatsmeow keeps the pins, archives and mutes of the initial full app-state sync in its own
	// tables and emits no event for them unless asked; nuncio applies them like any later change.
	client.EmitAppStateEventsOnFullSync = true
	s := NewSession(whatsmeowClient{client}, db, db.Close)
	// Attachments are downloaded only on a download_media request, by whatsmeow's Download
	// (which fetches, decrypts and checks the hashes), into $XDG_DATA_HOME/nuncio/media.
	s.SetMedia(filepath.Join(filepath.Dir(path), store.MediaDir), client)
	return s, nil
}

// whatsmeowClient adapts *whatsmeow.Client to Client.
type whatsmeowClient struct{ c *whatsmeow.Client }

func (w whatsmeowClient) HasDevice() bool                  { return w.c.Store.ID != nil }
func (w whatsmeowClient) Connect() error                   { return w.c.Connect() }
func (w whatsmeowClient) Disconnect()                      { w.c.Disconnect() }
func (w whatsmeowClient) AddEventHandler(h func(any))      { w.c.AddEventHandler(h) }
func (w whatsmeowClient) Logout(ctx context.Context) error { return w.c.Logout(ctx) }

func (w whatsmeowClient) NewMessageID() string { return w.c.GenerateMessageID() }

// SendText sends a plain conversation message, the simplest text message whatsmeow has, under the
// message id the outbox chose.
func (w whatsmeowClient) SendText(ctx context.Context, jid, text, id string) (time.Time, error) {
	to, err := types.ParseJID(jid)
	if err != nil {
		return time.Time{}, fmt.Errorf("wa: %w", err)
	}
	resp, err := w.c.SendMessage(ctx, to, &waE2E.Message{Conversation: proto.String(text)},
		whatsmeow.SendRequestExtra{ID: id})
	if err != nil {
		return time.Time{}, err
	}
	return resp.Timestamp, nil
}
