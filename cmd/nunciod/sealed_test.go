package main

import (
	"bufio"
	"bytes"
	"context"
	"database/sql"
	"errors"
	"io"
	"log"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// lockableKeyring is a fakeKeyring that fails every call while locked, as a locked or absent
// Secret Service does.
type lockableKeyring struct {
	fakeKeyring
	locked atomic.Bool
	calls  atomic.Int64
}

var errTestLocked = errors.New("test: the keyring is locked")

func newLockableKeyring() *lockableKeyring {
	return &lockableKeyring{fakeKeyring: fakeKeyring{items: map[string]string{}}}
}

func (k *lockableKeyring) Set(service, user, secret string) error {
	k.calls.Add(1)
	if k.locked.Load() {
		return errTestLocked
	}
	return k.fakeKeyring.Set(service, user, secret)
}

func (k *lockableKeyring) Get(service, user string) (string, error) {
	k.calls.Add(1)
	if k.locked.Load() {
		return "", errTestLocked
	}
	return k.fakeKeyring.Get(service, user)
}

func (k *lockableKeyring) Delete(service, user string) error {
	k.calls.Add(1)
	if k.locked.Load() {
		return errTestLocked
	}
	return k.fakeKeyring.Delete(service, user)
}

func (k *lockableKeyring) hasDataKey() bool {
	_, err := k.fakeKeyring.Get(store.KeyringService, "store.data_key")
	return err == nil
}

var sealedJID = types.JID{User: "15551230001", Server: types.DefaultUserServer, Device: 7}

// testAccount is the account signature pairing gives a device, in placeholder bytes.
func testAccount() *waAdv.ADVSignedDeviceIdentity {
	return &waAdv.ADVSignedDeviceIdentity{Details: []byte("synthetic"), AccountSignature: bytes.Repeat([]byte{1}, 64),
		AccountSignatureKey: bytes.Repeat([]byte{2}, 32), DeviceSignature: bytes.Repeat([]byte{3}, 64)}
}

// plainStore writes a nuncio.db the way builds before T17 left it (whatsmeow's stock store, the
// device's keys in plain) and returns its path and the noise private key.
func plainStore(t *testing.T) (string, []byte) {
	t.Helper()
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "nuncio", store.DBFile)
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	if err := store.EnsureSchema(db); err != nil {
		t.Fatal(err)
	}
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		t.Fatal(err)
	}
	device := container.NewDevice()
	device.ID = &sealedJID
	device.Account = testAccount()
	if err := container.PutDevice(ctx, device); err != nil {
		t.Fatal(err)
	}
	return path, append([]byte(nil), device.NoiseKey.Priv[:]...)
}

// sealedStore links a device the way `nuncio link` does and returns the path and the keyring that
// holds its data key.
func sealedStore(t *testing.T) (string, *lockableKeyring) {
	t.Helper()
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "nuncio", store.DBFile)
	kr := newLockableKeyring()
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	if err := store.EnsureSchema(db); err != nil {
		t.Fatal(err)
	}
	device, _, err := wa.OpenDevice(ctx, db, path, kr, true)
	if err != nil {
		t.Fatal(err)
	}
	device.ID = &sealedJID
	device.Account = testAccount()
	if err := device.Save(ctx); err != nil {
		t.Fatal(err)
	}
	return path, kr
}

// captureLog sends the standard logger to a buffer for this test.
func captureLog(t *testing.T) *syncBuffer {
	t.Helper()
	buf := &syncBuffer{}
	log.SetOutput(buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	return buf
}

type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

// manualTimers collects the attempts a gate schedules, to run them by hand.
type manualTimers struct {
	mu     sync.Mutex
	queued []func()
	delays []time.Duration
}

func (m *manualTimers) after(d time.Duration, f func()) func() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.queued = append(m.queued, f)
	m.delays = append(m.delays, d)
	return func() bool { return true }
}

// fire runs the oldest scheduled attempt; it reports false when none is scheduled.
func (m *manualTimers) fire() bool {
	m.mu.Lock()
	if len(m.queued) == 0 {
		m.mu.Unlock()
		return false
	}
	f := m.queued[0]
	m.queued = m.queued[1:]
	m.mu.Unlock()
	f()
	return true
}

func (m *manualTimers) count() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return len(m.delays)
}

// asGate returns the gate openSession answered, with fake timers and a start that only counts,
// so no test can reach WhatsApp.
func asGate(t *testing.T, s daemonSession) (*gate, *manualTimers, *atomic.Int64) {
	t.Helper()
	g, ok := s.(*gate)
	if !ok {
		t.Fatalf("openSession answered %T, want the gate", s)
	}
	timers := &manualTimers{}
	var starts atomic.Int64
	g.after = timers.after
	g.start = func(*wa.Session) error { starts.Add(1); return nil }
	return g, timers, &starts
}

// The daemon's start over a database an older build wrote migrates it (a plain backup next to
// it, no device key left in nuncio.db), then serves it with the device open.
func TestDaemonStartMigratesAPlainDatabaseThenServes(t *testing.T) {
	captureLog(t)
	path, noise := plainStore(t)
	kr := newLockableKeyring()

	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openSession: %v", err)
	}
	g, timers, starts := asGate(t, s)
	if backups, _ := filepath.Glob(path + ".pre-t17-*.bak"); len(backups) != 0 || kr.calls.Load() != 0 {
		t.Errorf("openSession migrated before the first attempt (backups %v, %d keyring calls)", backups, kr.calls.Load())
	}
	_ = g.Start()
	if !timers.fire() || g.session() == nil || starts.Load() != 1 {
		t.Fatalf("the first attempt did not open and start the device (%d starts)", starts.Load())
	}
	backups, _ := filepath.Glob(path + ".pre-t17-*.bak")
	if len(backups) != 1 {
		t.Errorf("backups %v, want one", backups)
	}
	if !kr.hasDataKey() {
		t.Error("no data key after the migration")
	}
	d, err := newDaemon(s, db)
	if err != nil {
		t.Fatal(err)
	}
	if chats := d.Chats(); len(chats) != 0 {
		t.Errorf("Chats() = %d chats, want the empty store, not the fixture list", len(chats))
	}
	s.Close()
	for _, name := range []string{store.DBFile, store.DBFile + "-wal"} {
		data, err := os.ReadFile(filepath.Join(filepath.Dir(path), name))
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			t.Fatal(err)
		}
		if bytes.Contains(data, noise) {
			t.Errorf("%s still holds the noise key in plain", name)
		}
	}
}

// With the keyring locked the daemon stays up: status says offline (linked, no stop_reason), a
// send is refused with the fixed reason, nothing is written (no backup, nuncio.db unchanged) and
// nothing connects; it tries again on the backoff and, once the keyring opens, opens the device
// and starts it, once.
func TestDaemonWithTheKeyringLockedServesOfflineWritesNothingAndRetries(t *testing.T) {
	log := captureLog(t)
	path, _ := plainStore(t)
	kr := newLockableKeyring()
	kr.locked.Store(true)

	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openSession: %v", err)
	}
	g, timers, starts := asGate(t, s)
	d, err := newDaemon(s, db)
	if err != nil {
		t.Fatal(err)
	}
	want := ipc.Status{Daemon: daemonVersion, Linked: true, Session: ipc.SessionOffline}
	if got := d.Status(); got != want {
		t.Errorf("Status() = %+v, want %+v", got, want)
	}
	if _, err := d.Send("15551230002@s.whatsapp.net", "hello", "c-1"); !errors.Is(err, errOpening) {
		t.Errorf("Send before the first attempt = %v, want the opening reason", err)
	}

	if err := g.Start(); err != nil {
		t.Fatal(err)
	}
	if !timers.fire() {
		t.Fatal("Start scheduled no attempt")
	}
	if timers.delays[0] != 0 {
		t.Errorf("the first attempt waits %s, want none", timers.delays[0])
	}
	if _, err := d.Send("15551230002@s.whatsapp.net", "hello", "c-1"); !errors.Is(err, errKeyringLocked) {
		t.Errorf("Send = %v, want the keyring reason", err)
	}
	if timers.count() != 2 {
		t.Errorf("%d attempts scheduled after a failed one, want the next one too", timers.count())
	}
	if n := starts.Load(); n != 0 {
		t.Errorf("started %d times while the keyring was locked, want never", n)
	}
	if backups, _ := filepath.Glob(path + ".pre-t17-*.bak"); len(backups) != 0 {
		t.Errorf("a backup was written while the keyring was locked: %v", backups)
	}
	if !strings.Contains(log.String(), "the keyring (Secret Service) is locked or unavailable") {
		t.Errorf("log %q, want the keyring line", log.String())
	}

	kr.locked.Store(false)
	if !timers.fire() {
		t.Fatal("no next attempt scheduled")
	}
	if n := starts.Load(); n != 1 {
		t.Errorf("started %d times once the keyring opened, want 1", n)
	}
	if g.session() == nil {
		t.Error("the gate did not hand over to the opened session")
	}
	if timers.fire() {
		t.Error("an attempt was still scheduled after the device opened")
	}
	g.Close()
}

// While locked nothing is written to nuncio.db: its bytes are the same after the daemon served
// and closed.
func TestDaemonWithTheKeyringLockedLeavesTheDatabaseBytesUnchanged(t *testing.T) {
	captureLog(t)
	plain, _ := plainStore(t)
	sealed, kr := sealedStore(t)
	for name, c := range map[string]struct {
		path string
		kr   *lockableKeyring
	}{"plain": {plain, newLockableKeyring()}, "sealed": {sealed, kr}} {
		t.Run(name, func(t *testing.T) {
			before, err := os.ReadFile(c.path)
			if err != nil {
				t.Fatal(err)
			}
			c.kr.locked.Store(true)
			defer c.kr.locked.Store(false)
			s, db, err := openSession(context.Background(), c.path, c.kr)
			if err != nil {
				t.Fatalf("openSession: %v", err)
			}
			g, timers, _ := asGate(t, s)
			d, _ := newDaemon(s, db)
			_ = d.Status()
			_ = d.Chats()
			_ = g.Start()
			timers.fire()
			g.Close()
			after, err := os.ReadFile(c.path)
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(before, after) {
				t.Error("nuncio.db changed while the keyring was locked")
			}
		})
	}
}

// With the data key gone while a sealed device is stored, the daemon creates no key, says the
// device cannot be read and how to start over, and never tries again on its own.
func TestDaemonWithTheKeyMissingOverSealedDataCreatesNoKey(t *testing.T) {
	log := captureLog(t)
	path, kr := sealedStore(t)
	if err := kr.Delete(store.KeyringService, "store.data_key"); err != nil {
		t.Fatal(err)
	}

	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openSession: %v", err)
	}
	g, timers, starts := asGate(t, s)
	defer g.Close()
	_ = g.Start()
	timers.fire()
	d, _ := newDaemon(s, db)
	if got := d.Status(); got.Session != ipc.SessionOffline || !got.Linked || got.StopReason != ipc.StopDeviceUnreadable {
		t.Errorf("Status() = %+v, want offline, linked, stop_reason device_unreadable", got)
	}
	const want = "the stored device cannot be read; run nuncio purge --confirm, then nuncio link"
	if _, err := d.Send("15551230002@s.whatsapp.net", "hello", "c-1"); err == nil || err.Error() != want {
		t.Errorf("Send = %v, want %q", err, want)
	}
	if err := d.React("15551230002@s.whatsapp.net", "3EB0", "👍"); err == nil || err.Error() != want {
		t.Errorf("React = %v, want %q", err, want)
	}
	if !strings.Contains(log.String(), want) {
		t.Errorf("log %q, want %q", log.String(), want)
	}
	_ = g.Start()
	if timers.count() != 1 || starts.Load() != 0 {
		t.Errorf("%d attempts scheduled, %d starts; want only the first attempt and no start", timers.count(), starts.Load())
	}
	if kr.hasDataKey() {
		t.Error("a data key was created over sealed data")
	}
}

// Without a stored device the daemon is unlinked over the store, as before, and never asks the
// keyring.
func TestDaemonWithoutADeviceNeverAsksTheKeyring(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nuncio", store.DBFile)
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	_ = db.Close()
	kr := newLockableKeyring()
	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openSession: %v", err)
	}
	defer s.Close()
	if _, ok := s.(*wa.Session); !ok {
		t.Fatalf("openSession answered %T, want a plain session", s)
	}
	d, _ := newDaemon(s, db)
	want := ipc.Status{Daemon: daemonVersion, Session: ipc.SessionUnlinked}
	if got := d.Status(); got != want {
		t.Errorf("Status() = %+v, want %+v", got, want)
	}
	if n := kr.calls.Load(); n != 0 {
		t.Errorf("the keyring was called %d times, want never", n)
	}
}

// rawFrames dials sock, sends hello then status and returns both replies as the bytes on the wire.
func rawFrames(t *testing.T, sock string) (hello, status string) {
	t.Helper()
	conn, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatalf("dial %s: %v", sock, err)
	}
	defer func() { _ = conn.Close() }()
	br := bufio.NewReader(conn)
	var replies []string
	for _, send := range []string{`{"v":1,"op":"hello","client":"test/0.1.0"}`, `{"v":1,"op":"status"}`} {
		if _, err := io.WriteString(conn, send+"\n"); err != nil {
			t.Fatal(err)
		}
		replies = append(replies, readFrame(t, conn, br, 5*time.Second))
	}
	return replies[0], replies[1]
}

// A normally linked daemon answers the same hello and status frames as before, byte for byte, and
// so does a gate once it has handed over to the opened session. A gate still opening its device
// answers the existing offline frames.
func TestStatusAndHelloFramesAreUnchangedWhenLinkedNormally(t *testing.T) {
	fake := &fakeWAClient{sendID: "3EB0BBB1"}
	live := wa.NewSession(fake, nil, nil)
	if err := live.Start(); err != nil {
		t.Fatal(err)
	}
	defer live.Close()
	const (
		linkedHello   = `{"v":1,"op":"hello_ok","daemon":"nunciod/0.1.0","session":"linked","unread":0}`
		linkedStatus  = `{"v":1,"op":"status","daemon":"nunciod/0.1.0","linked":true,"session":"linked","unread":0}`
		offlineHello  = `{"v":1,"op":"hello_ok","daemon":"nunciod/0.1.0","session":"offline","unread":0}`
		offlineStatus = `{"v":1,"op":"status","daemon":"nunciod/0.1.0","linked":true,"session":"offline","unread":0}`
		lockedStatus  = `{"v":1,"op":"status","daemon":"nunciod/0.1.0","linked":true,"session":"offline","unread":0,"stop_reason":"keyring_locked"}`
	)
	for _, c := range []struct {
		name          string
		session       sessionState
		hello, status string
	}{
		{"direct", live, linkedHello, linkedStatus},
		{"gate handed over", &gate{live: live}, linkedHello, linkedStatus},
		{"gate opening", &gate{reason: errOpening}, offlineHello, offlineStatus},
		// D4: a locked keyring adds stop_reason to status only; hello_ok never carries it.
		{"gate locked", &gate{reason: errKeyringLocked}, offlineHello, lockedStatus},
	} {
		d, err := newDaemon(c.session, nil)
		if err != nil {
			t.Fatal(err)
		}
		hello, status := rawFrames(t, serveSocket(t, d, nil))
		if hello != c.hello || status != c.status {
			t.Errorf("%s: frames\n %s\n %s\nwant\n %s\n %s", c.name, hello, status, c.hello, c.status)
		}
	}
}

// Close stops the attempts: one already scheduled opens nothing afterwards, and the store closes.
func TestGateCloseStopsTheAttempts(t *testing.T) {
	captureLog(t)
	path, _ := plainStore(t)
	kr := newLockableKeyring()
	kr.locked.Store(true)
	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatal(err)
	}
	g, timers, starts := asGate(t, s)
	var opens atomic.Int64
	prev := openSessionOn
	openSessionOn = func(ctx context.Context, db *sql.DB, path string, kr store.Keyring) (*wa.Session, error) {
		opens.Add(1)
		return prev(ctx, db, path, kr)
	}
	t.Cleanup(func() { openSessionOn = prev })
	_ = g.Start()
	kr.locked.Store(false)
	g.Close()
	timers.fire()
	if opens.Load() != 0 || starts.Load() != 0 || g.session() != nil {
		t.Errorf("after Close: %d opens, %d starts; want none", opens.Load(), starts.Load())
	}
	if err := db.Ping(); err == nil {
		t.Error("the store is still open after Close")
	}
}

// blockingKeyring is a lockableKeyring whose Get waits until release is closed, as go-keyring's
// Secret Service does while an unlock prompt is open (it waits on the prompt with no timeout).
type blockingKeyring struct {
	*lockableKeyring
	release chan struct{}
	waiting chan struct{} // gets a value each time a Get starts waiting
	gets    atomic.Int64
}

func newBlockingKeyring(kr *lockableKeyring) *blockingKeyring {
	return &blockingKeyring{lockableKeyring: kr, release: make(chan struct{}), waiting: make(chan struct{}, 16)}
}

func (k *blockingKeyring) Get(service, user string) (string, error) {
	k.gets.Add(1)
	k.waiting <- struct{}{}
	<-k.release
	return k.lockableKeyring.Get(service, user)
}

// openBlocked opens the daemon's session over a sealed store whose keyring blocks, with real
// timers and a start that only counts, and starts it: the first attempt is then waiting on the
// keyring. openSession itself must not wait on the keyring.
func openBlocked(t *testing.T) (*gate, *sql.DB, *blockingKeyring, *atomic.Int64) {
	t.Helper()
	captureLog(t)
	path, sealed := sealedStore(t)
	kr := newBlockingKeyring(sealed)
	t.Cleanup(func() {
		select {
		case <-kr.release:
		default:
			close(kr.release)
		}
	})
	type opened struct {
		s   daemonSession
		db  *sql.DB
		err error
	}
	done := make(chan opened, 1)
	go func() {
		s, db, err := openSession(context.Background(), path, kr)
		done <- opened{s, db, err}
	}()
	var o opened
	select {
	case o = <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("openSession waited on the keyring")
	}
	if o.err != nil {
		t.Fatalf("openSession: %v", o.err)
	}
	g, ok := o.s.(*gate)
	if !ok {
		t.Fatalf("openSession answered %T, want the gate", o.s)
	}
	var starts atomic.Int64
	g.start = func(*wa.Session) error { starts.Add(1); return nil }
	if err := g.Start(); err != nil {
		t.Fatal(err)
	}
	select {
	case <-kr.waiting:
	case <-time.After(5 * time.Second):
		t.Fatal("the first attempt never asked the keyring")
	}
	return g, o.db, kr, &starts
}

// L9 A1: while the keyring blocks the first attempt, the socket answers status offline, and no
// second attempt (no second prompt) starts; once the keyring answers, the device opens, once.
func TestDaemonServesOfflineWhileTheKeyringBlocks(t *testing.T) {
	g, db, kr, starts := openBlocked(t)
	defer g.Close()
	d, err := newDaemon(g, db)
	if err != nil {
		t.Fatal(err)
	}
	sock := serveSocket(t, d, nil)
	const offlineStatus = `{"v":1,"op":"status","daemon":"nunciod/0.1.0","linked":true,"session":"offline","unread":0}`
	if _, status := rawFrames(t, sock); status != offlineStatus {
		t.Errorf("status while the keyring blocks = %s, want %s", status, offlineStatus)
	}
	if _, err := d.Send("15551230002@s.whatsapp.net", "hello", "c-1"); !errors.Is(err, errOpening) {
		t.Errorf("Send = %v, want the opening reason", err)
	}
	// Neither a retry being armed nor a timer firing starts a second attempt meanwhile.
	g.schedule(false)
	go g.attempt()
	time.Sleep(100 * time.Millisecond)
	if n := kr.gets.Load(); n != 1 {
		t.Errorf("%d keyring reads while the first one waits, want 1", n)
	}

	close(kr.release)
	deadline := time.Now().Add(5 * time.Second)
	for g.session() == nil && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if g.session() == nil || starts.Load() != 1 {
		t.Fatalf("after the keyring answered: open %t, %d starts; want open and started once", g.session() != nil, starts.Load())
	}
}

// Close does not wait for an attempt blocked on the keyring; that attempt, once it returns, opens
// nothing and closes the store.
func TestGateCloseDoesNotWaitForABlockedAttempt(t *testing.T) {
	g, db, kr, starts := openBlocked(t)
	closed := make(chan struct{})
	go func() { g.Close(); close(closed) }()
	select {
	case <-closed:
	case <-time.After(5 * time.Second):
		t.Fatal("Close waited for the attempt blocked on the keyring")
	}
	close(kr.release)
	deadline := time.Now().Add(5 * time.Second)
	for db.Ping() == nil && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if err := db.Ping(); err == nil {
		t.Error("the store is still open after the blocked attempt returned")
	}
	if g.session() != nil || starts.Load() != 0 {
		t.Errorf("after Close: open %t, %d starts; want neither", g.session() != nil, starts.Load())
	}
}
