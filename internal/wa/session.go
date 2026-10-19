package wa

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"

	"go.mau.fi/whatsmeow"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"

	"github.com/pwn27sbx/nuncio/internal/ipc"
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
}

// Session is the daemon's one WhatsApp session. It connects only a device that is already linked
// and never starts a link: with no device it stays unlinked and Connect is never called.
type Session struct {
	client     Client
	db         *sql.DB
	closeStore func() error

	mu      sync.Mutex
	state   State
	started bool
}

// NewSession wraps client, which may be nil for "no device store at all". db is the message
// store that incoming messages are saved to, nil for none. closeStore, when not nil, runs on Close
// after the client is disconnected.
func NewSession(client Client, db *sql.DB, closeStore func() error) *Session {
	return &Session{client: client, db: db, closeStore: closeStore, state: StateUnlinked}
}

// Store is the message store, or nil when there is no database (unlinked).
func (s *Session) Store() *sql.DB { return s.db }

// Start connects a stored device. It returns the connect error, if any, and leaves the session
// offline; reconnecting is not its job.
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
		Connected:    func() { s.set(StateLinked) },
		Disconnected: func() { s.set(StateOffline) },
		LoggedOut:    func() { s.set(StateUnlinked) },
		Dropped:      func() { s.set(StateOffline) },
	}
	if s.db != nil {
		events.Message = s.storeMessage
	}
	s.client.AddEventHandler(events.Handle)
	s.set(StateOffline)
	if err := s.client.Connect(); err != nil {
		s.set(StateOffline)
		return fmt.Errorf("wa: connecting the linked device: %w", err)
	}
	return nil
}

// State is the current session state; safe for concurrent use.
func (s *Session) State() State {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state
}

func (s *Session) set(st State) {
	s.mu.Lock()
	s.state = st
	s.mu.Unlock()
}

// Close disconnects a started session and closes the device store.
func (s *Session) Close() {
	s.mu.Lock()
	started := s.started
	s.started = false
	s.mu.Unlock()
	if started {
		s.client.Disconnect()
	}
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
	// whatsmeow reconnects on its own by default; nuncio's capped backoff replaces that, and until
	// it exists a dropped session stays offline instead of retrying silently.
	client.EnableAutoReconnect = false
	return NewSession(whatsmeowClient{client}, db, db.Close), nil
}

// whatsmeowClient adapts *whatsmeow.Client to Client.
type whatsmeowClient struct{ c *whatsmeow.Client }

func (w whatsmeowClient) HasDevice() bool             { return w.c.Store.ID != nil }
func (w whatsmeowClient) Connect() error              { return w.c.Connect() }
func (w whatsmeowClient) Disconnect()                 { w.c.Disconnect() }
func (w whatsmeowClient) AddEventHandler(h func(any)) { w.c.AddEventHandler(h) }
