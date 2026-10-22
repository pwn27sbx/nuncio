package main

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"sync"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// The device's private material is sealed under a data key in the Secret Service (ADR 0003). The
// daemon never creates that key over a stored device: only `nuncio link` creates it, when pairing
// saves a new device, and the migration of a database an older build wrote, which holds no
// sealed data yet. The daemon serves the socket before it first asks the keyring for a stored
// device (on Linux the Secret Service may wait on an unlock prompt for as long as it likes), and
// while the device is not open it reports the session offline and opens nothing: no WhatsApp
// connection and no write.

// The fixed answers of a daemon whose device is not open: what a request that needs the session
// gets, and the start log says.
var (
	errOpening       = errors.New("nunciod is still opening the WhatsApp device (the keyring may be asking to be unlocked); nothing was sent or stored")
	errKeyringLocked = errors.New("the keyring (Secret Service) is locked or unavailable: nunciod keeps the WhatsApp device closed and tries again; nothing was sent or stored")
	errCleanupWait   = errors.New("nunciod sealed the WhatsApp keys but has not yet wiped their old plain copies from nuncio.db; it tries again before it connects")
	errStoreFailed   = errors.New("nunciod cannot open the WhatsApp device store; see its log")
)

// daemonSession is what run serves and starts: a *wa.Session, or a gate while the device is not
// open.
type daemonSession interface {
	sessionState
	SetEvents(*ipc.Broker)
	Start() error
}

// openSessionOn is wa.OpenSessionOn; tests replace it.
var openSessionOn = wa.OpenSessionOn

// openSession opens the daemon's session over the database at dbPath with the data key in kr. A
// missing database is unlinked and nothing is created. Without a stored device the session is
// unlinked and the keyring is never asked. With a stored device it does not open it: it returns a
// gate over the store that answers offline and opens nothing, whose Start makes the first attempt
// in the background, so the socket answers even while the keyring blocks (L9 A1). When an attempt
// fails (the keyring is locked or absent, the migration's cleanup is pending, the key is missing
// or another one, a sealed row is altered) the gate keeps answering offline. db is the message
// store the daemon reads, nil without a database.
func openSession(ctx context.Context, dbPath string, kr store.Keyring) (daemonSession, *sql.DB, error) {
	if _, err := os.Stat(dbPath); errors.Is(err, fs.ErrNotExist) {
		return wa.NewSession(nil, nil, nil), nil, nil
	} else if err != nil {
		return nil, nil, fmt.Errorf("stat the database: %w", err)
	}
	db, err := store.Open(dbPath)
	if err != nil {
		return nil, nil, err
	}
	g := &gate{db: db, dbPath: dbPath, kr: kr, backoff: wa.NewBackoff(), after: afterFunc, start: (*wa.Session).Start, logf: log.Printf,
		reason: errOpening, retry: true}
	if stored, err := wa.HasStoredDevice(ctx, db); err != nil || stored {
		return g, db, nil
	}
	s, err := openSessionOn(ctx, db, dbPath, kr)
	if err == nil {
		return s, db, nil
	}
	g.fail(err)
	return g, db, nil
}

func afterFunc(d time.Duration, f func()) func() bool { return time.AfterFunc(d, f).Stop }

// gate is the daemon's session while the stored device is not open. It reports offline (a device
// is stored: linked, not connected) and answers every request that needs the session with the
// fixed reason. Status and event_connection carry stop_reason keyring_locked while the keyring is
// locked or absent and device_unreadable once the device cannot be read; the gate publishes an
// event_connection each time that stop_reason changes, and one without it when the device opens,
// before the session publishes its own (as a Q6 stop clears). Start
// makes the first attempt at once, in the background; when the reason can pass (a locked
// keyring, a pending cleanup) it tries again on the session's capped backoff; once the device
// opens, the gate hands everything to that session and starts it. A reason that cannot pass
// (ErrDeviceUnreadable, another failure) is logged once and never retried. One attempt runs at a
// time: while one waits on the keyring no other is scheduled, so a prompt is never stacked.
type gate struct {
	db      *sql.DB
	dbPath  string
	kr      store.Keyring
	backoff *wa.Backoff
	// after schedules an attempt (time.AfterFunc outside tests); start starts the opened session
	// ((*wa.Session).Start outside tests, which connects).
	after func(d time.Duration, f func()) (cancel func() bool)
	start func(*wa.Session) error
	logf  func(format string, args ...any)

	mu       sync.Mutex
	live     *wa.Session // the opened session, nil until then
	reason   error       // what the gate answers while live is nil
	retry    bool
	started  bool
	closed   bool
	inflight bool        // an attempt is running; it closes what it opened when Close came meanwhile
	cancel   func() bool // the pending attempt
	events   *ipc.Broker
	// published is the stop_reason of the last event_connection the gate published ("" for none).
	published string
	unread    func()
}

// fail records why the device is not open and logs it once, in fixed words: the error's type, never
// its text.
func (g *gate) fail(err error) {
	reason, retry := errKeyringLocked, true
	switch {
	case errors.Is(err, store.ErrKeyringUnavailable):
	case errors.Is(err, wa.ErrCleanupPending):
		reason = errCleanupWait
	case errors.Is(err, wa.ErrDeviceUnreadable):
		reason, retry = wa.ErrDeviceUnreadable, false
	default:
		reason, retry = errStoreFailed, false
		g.logf("cannot open the WhatsApp device store (%T)", err)
	}
	g.mu.Lock()
	g.reason, g.retry = reason, retry
	g.mu.Unlock()
	if !retry {
		g.logf("the WhatsApp device stays closed: %v", reason)
	}
	g.publishStop(gateStopKind(reason))
}

// gateStopKind is the stop_reason of a gate's reason: only a locked keyring and an unreadable
// device have one.
func gateStopKind(reason error) string {
	switch reason {
	case errKeyringLocked:
		return ipc.StopKeyringLocked
	case wa.ErrDeviceUnreadable:
		return ipc.StopDeviceUnreadable
	}
	return ""
}

// publishStop publishes an offline event_connection with stop_reason kind when it differs from the
// last one the gate published. Attempts run one at a time, so their frames never cross.
func (g *gate) publishStop(kind string) {
	g.mu.Lock()
	if kind == g.published {
		g.mu.Unlock()
		return
	}
	g.published = kind
	b := g.events
	g.mu.Unlock()
	if b != nil {
		b.Publish(ipc.EventConnection{V: ipc.Version, Op: ipc.OpEventConnection, Session: string(wa.StateOffline), StopReason: kind})
	}
}

// schedule arms the next attempt on the backoff, or the first one at once, unless one is pending
// or running, the gate is closed or the device is open.
func (g *gate) schedule(first bool) {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.closed || g.live != nil || !g.retry || g.cancel != nil || g.inflight {
		return
	}
	if first {
		g.cancel = g.after(0, g.attempt)
		return
	}
	delay, ok := g.backoff.Next()
	if !ok {
		return
	}
	g.logf("%v (next attempt in %s)", g.reason, delay.Round(time.Second))
	g.cancel = g.after(delay, g.attempt)
}

// attempt tries to open the device once; on success the session takes over and is started. It
// does not hold the gate's lock while it opens, so the gate keeps answering and Close does not
// wait for a keyring that blocks: an attempt that returns after Close closes what it opened.
func (g *gate) attempt() {
	g.mu.Lock()
	g.cancel = nil
	skip := g.closed || g.live != nil || g.inflight
	if !skip {
		g.inflight = true
	}
	g.mu.Unlock()
	if skip {
		return
	}
	s, err := openSessionOn(context.Background(), g.db, g.dbPath, g.kr)
	g.mu.Lock()
	g.inflight = false
	if g.closed {
		g.mu.Unlock()
		if err == nil {
			s.Close()
		} else {
			_ = g.db.Close()
		}
		return
	}
	if err != nil {
		g.mu.Unlock()
		g.fail(err)
		g.schedule(false)
		return
	}
	s.SetEvents(g.events)
	if g.unread != nil {
		s.SetUnreadChanged(g.unread)
	}
	g.live = s
	g.mu.Unlock()
	g.logf("opened the WhatsApp device")
	g.publishStop("")
	if err := g.start(s); err != nil {
		// The cause's type only: whatsmeow's text can carry the server or proxy address.
		g.logf("cannot connect the linked device (%s)", wa.ErrKind(err))
	}
}

// session is the opened session, or nil while the gate answers itself.
func (g *gate) session() *wa.Session {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.live
}

func (g *gate) why() error {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.reason
}

// Start begins the attempts, once, the first one at once; with a reason that cannot pass it does
// nothing.
func (g *gate) Start() error {
	g.mu.Lock()
	first := !g.started
	g.started = true
	g.mu.Unlock()
	if first {
		g.schedule(true)
	}
	return nil
}

func (g *gate) SetEvents(b *ipc.Broker) {
	g.mu.Lock()
	g.events = b
	live := g.live
	g.mu.Unlock()
	if live != nil {
		live.SetEvents(b)
	}
}

func (g *gate) SetUnreadChanged(f func()) {
	g.mu.Lock()
	g.unread = f
	live := g.live
	g.mu.Unlock()
	if live != nil {
		live.SetUnreadChanged(f)
	}
}

func (g *gate) State() wa.State {
	if s := g.session(); s != nil {
		return s.State()
	}
	return wa.StateOffline
}

func (g *gate) StopKind() string {
	if s := g.session(); s != nil {
		return s.StopKind()
	}
	return gateStopKind(g.why())
}

func (g *gate) Send(jid, text, clientID string) (ipc.Sent, error) {
	if s := g.session(); s != nil {
		return s.Send(jid, text, clientID)
	}
	return ipc.Sent{}, g.why()
}

func (g *gate) Retry(clientID string) (ipc.Sent, error) {
	if s := g.session(); s != nil {
		return s.Retry(clientID)
	}
	return ipc.Sent{}, g.why()
}

func (g *gate) SendReply(jid, text, clientID, replyTo string) (ipc.Sent, error) {
	if s := g.session(); s != nil {
		return s.SendReply(jid, text, clientID, replyTo)
	}
	return ipc.Sent{}, g.why()
}

func (g *gate) React(jid, id, emoji string) error {
	if s := g.session(); s != nil {
		return s.React(jid, id, emoji)
	}
	return g.why()
}

func (g *gate) MarkRead(jid string) (int, error) {
	if s := g.session(); s != nil {
		return s.MarkRead(jid)
	}
	return 0, g.why()
}

// Logout needs the session: without it nothing reaches WhatsApp and nothing is deleted (the
// daemon's Wipe stops at this error); `nuncio purge --confirm` still works.
func (g *gate) Logout(ctx context.Context) error {
	if s := g.session(); s != nil {
		return s.Logout(ctx)
	}
	return fmt.Errorf("%w; to delete the local data without contacting WhatsApp use nuncio purge --confirm", g.why())
}

func (g *gate) DownloadMedia(jid, id string) (string, string, error) {
	if s := g.session(); s != nil {
		return s.DownloadMedia(jid, id)
	}
	return "", "", g.why()
}

func (g *gate) ChatAliases(canonical string) []string {
	if s := g.session(); s != nil {
		return s.ChatAliases(canonical)
	}
	return nil
}

func (g *gate) ChatPhone(jid string) string {
	if s := g.session(); s != nil {
		return s.ChatPhone(jid)
	}
	return ""
}

func (g *gate) Senders() func(string) string {
	if s := g.session(); s != nil {
		return s.Senders()
	}
	return nil
}

func (g *gate) CanonicalJID(jid string) string {
	if s := g.session(); s != nil {
		return s.CanonicalJID(jid)
	}
	return jid
}

// Close stops the attempts and closes the session, or the store while there is none. It never
// waits for a running attempt (which may be blocked on the keyring): that attempt closes what it
// opened, or the store, when it returns.
func (g *gate) Close() {
	g.mu.Lock()
	if g.closed {
		g.mu.Unlock()
		return
	}
	g.closed = true
	cancel := g.cancel
	g.cancel = nil
	live := g.live
	inflight := g.inflight
	g.mu.Unlock()
	if cancel != nil {
		cancel()
	}
	switch {
	case live != nil:
		live.Close()
	case !inflight:
		_ = g.db.Close()
	}
}
