package main

import (
	"bytes"
	"context"
	"database/sql"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

const dataKeyItem = "store.data_key"

// recordingKeyring is a memKeyring that records, in order, every call on the data key item, and
// when the key is written how many device rows the database held.
type recordingKeyring struct {
	*memKeyring
	mu           sync.Mutex
	ops          []string
	devicesAtSet int
}

func (k *recordingKeyring) record(op, user string) {
	if user == dataKeyItem {
		k.mu.Lock()
		k.ops = append(k.ops, op)
		k.mu.Unlock()
	}
}

func (k *recordingKeyring) Set(service, user, secret string) error {
	k.record("set", user)
	if user == dataKeyItem {
		k.devicesAtSet = deviceRows(nil)
	}
	return k.memKeyring.Set(service, user, secret)
}

func (k *recordingKeyring) Get(service, user string) (string, error) {
	k.record("get", user)
	return k.memKeyring.Get(service, user)
}

// useKeyring makes localKeyring kr for this test.
func useKeyring(t *testing.T, kr store.Keyring) {
	t.Helper()
	prev := localKeyring
	localKeyring = kr
	t.Cleanup(func() { localKeyring = prev })
}

// deviceRows counts the device rows of the isolated nuncio.db, opened read-only so the count
// writes nothing; t may be nil (then a failure counts as -1).
func deviceRows(t *testing.T) int {
	path, err := wa.DBPath()
	if err != nil {
		return -1
	}
	db, err := sql.Open("sqlite", "file:"+path+"?mode=ro")
	if err != nil {
		return -1
	}
	defer func() { _ = db.Close() }()
	var n int
	if err := db.QueryRow(`SELECT COUNT(*) FROM whatsmeow_device`).Scan(&n); err != nil {
		if t != nil {
			t.Logf("count the device rows: %v", err)
		}
		return -1
	}
	return n
}

// rowsIn counts the rows of each table of the isolated nuncio.db (0 for a missing table).
func rowsIn(t *testing.T, tables ...string) map[string]int {
	t.Helper()
	path, err := wa.DBPath()
	if err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	got := map[string]int{}
	for _, table := range tables {
		var exists int
		if err := db.QueryRow(`SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?`, table).Scan(&exists); err != nil {
			t.Fatal(err)
		}
		if exists == 0 {
			got[table] = 0
			continue
		}
		var n int
		if err := db.QueryRow(`SELECT COUNT(*) FROM ` + table).Scan(&n); err != nil {
			t.Fatal(err)
		}
		got[table] = n
	}
	return got
}

// pairingLinker is a QR Linker that pairs the way whatsmeow's pair handler does: it fills in the
// device's JID and account and saves it, then reports success, or, with abort, reports a timeout
// before anything was paired. It never touches the network.
type pairingLinker struct {
	client *whatsmeow.Client
	abort  bool
	// saveErr is what the pairing save returned.
	saveErr error
}

func (l *pairingLinker) QR(ctx context.Context) (<-chan wa.Code, error) {
	codes := make(chan wa.Code, 1)
	if l.abort {
		codes <- wa.Code{Event: "timeout"}
		close(codes)
		return codes, nil
	}
	jid := types.JID{User: "15550009999", Device: 1, Server: types.DefaultUserServer}
	l.client.Store.ID = &jid
	l.client.Store.Account = &waAdv.ADVSignedDeviceIdentity{Details: []byte{1}, AccountSignature: make([]byte, 64),
		AccountSignatureKey: make([]byte, 32), DeviceSignature: make([]byte, 64)}
	if l.saveErr = l.client.Store.Save(ctx); l.saveErr != nil {
		codes <- wa.Code{Event: "error"}
	} else {
		codes <- wa.Code{Event: "success"}
	}
	close(codes)
	return codes, nil
}

func (l *pairingLinker) Phone(context.Context, string) (string, error) {
	return "", errors.New("pairingLinker: no phone flow")
}

// fakePairing replaces prepareLink with a pairingLinker over the link's own client.
func fakePairing(t *testing.T, abort bool) *pairingLinker {
	t.Helper()
	l := &pairingLinker{abort: abort}
	prepare := prepareLink
	prepareLink = func(c wa.Consent, client *whatsmeow.Client) (wa.Prepared, error) {
		l.client = client
		return wa.Prepared{Linker: l, Consent: c}, nil
	}
	t.Cleanup(func() { prepareLink = prepare })
	return l
}

func runLink(t *testing.T) (int, string, string) {
	t.Helper()
	type result struct {
		code     int
		out, err string
	}
	done := make(chan result, 1)
	go func() {
		code, out, errOut := runCLI("link", "--qr", "--consent")
		done <- result{code, out, errOut}
	}()
	select {
	case r := <-done:
		return r.code, r.out, r.err
	case <-time.After(10 * time.Second):
		t.Fatal("nuncio link still running after 10 s")
		return 0, "", ""
	}
}

// link creates the data key when pairing saves the device, reads it back, and only then saves the
// device: when the key is written there is no device row yet, and the device opens sealed after.
func TestLinkCreatesTheKeyAndReadsItBackBeforeSavingTheDevice(t *testing.T) {
	isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir()) // no core.sock there
	kr := &recordingKeyring{memKeyring: &memKeyring{items: map[string]string{}}, devicesAtSet: -2}
	useKeyring(t, kr)
	l := fakePairing(t, false)

	code, out, errOut := runLink(t)
	if code != 0 || l.saveErr != nil {
		t.Fatalf("exit %d, save %v, stderr %q; want 0", code, l.saveErr, errOut)
	}
	if !strings.Contains(out, "nuncio: linked as") {
		t.Errorf("stdout %q, want the linked line", out)
	}
	if kr.devicesAtSet != 0 {
		t.Errorf("the database held %d device rows when the data key was written, want 0", kr.devicesAtSet)
	}
	kr.mu.Lock()
	ops := strings.Join(kr.ops, ",")
	kr.mu.Unlock()
	// The two reads before pairing, then on the save: absent, the write, its read back and the
	// read after the seal check.
	if !strings.Contains(ops, "get,set,get,get") || strings.Count(ops, "set") != 1 {
		t.Errorf("data key calls %s, want one write followed by its read back and a second read", ops)
	}
	if got := rowsIn(t, "whatsmeow_device", "nuncio_device", "nuncio_seal_meta"); got["whatsmeow_device"] != 1 || got["nuncio_device"] != 1 || got["nuncio_seal_meta"] != 1 {
		t.Errorf("rows %v, want the device, its sealed keys and the seal check", got)
	}
	path, _ := wa.DBPath()
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	device, _, err := wa.OpenDevice(context.Background(), db, path, kr, false)
	if err != nil || device == nil || device.ID == nil || device.ID.User != "15550009999" {
		t.Errorf("the linked device does not open sealed: %v, %v", device, err)
	}
}

// A link that never pairs (the QR code timed out) leaves no data key and no rows.
func TestLinkAbortedBeforePairingLeavesNoKeyAndNoRows(t *testing.T) {
	isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())
	kr := &memKeyring{items: map[string]string{}}
	useKeyring(t, kr)
	fakePairing(t, true)

	code, _, errOut := runLink(t)
	if code != 1 || !strings.Contains(errOut, "nothing was linked") {
		t.Errorf("exit %d, stderr %q; want 1 and nothing linked", code, errOut)
	}
	if n := kr.count(); n != 0 {
		t.Errorf("the keyring holds %d items after an aborted link, want 0", n)
	}
	for table, n := range rowsIn(t, "whatsmeow_device", "nuncio_device", "nuncio_pre_keys", "nuncio_seal_meta") {
		if n != 0 {
			t.Errorf("%s holds %d rows after an aborted link, want 0", table, n)
		}
	}
}

// With the keyring locked, link says so and links nothing: no pairing is even prepared.
func TestLinkWithTheKeyringLockedLinksNothing(t *testing.T) {
	isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())
	fakeKeys(t)
	linkedDevice(t)
	useKeyring(t, refusingKeyring{})
	calls := fakeConnect(t)
	prepared := false
	prepare := prepareLink
	prepareLink = func(c wa.Consent, client *whatsmeow.Client) (wa.Prepared, error) {
		prepared = true
		return prepare(c, client)
	}
	t.Cleanup(func() { prepareLink = prepare })

	code, _, errOut := runLink(t)
	if code != 1 || !strings.Contains(errOut, "keyring") || !strings.Contains(errOut, "Nothing was linked") {
		t.Errorf("exit %d, stderr %q; want 1 and the keyring line", code, errOut)
	}
	if prepared || *calls != 0 {
		t.Errorf("prepared %t, connected %d times; want neither", prepared, *calls)
	}
}

// With the data key gone while a sealed device is stored, link creates no key and says how to
// start over.
func TestLinkWithTheKeyMissingOverASealedDeviceSaysPurge(t *testing.T) {
	isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())
	keys := fakeKeys(t)
	linkedDevice(t)
	if err := keys.Delete(store.KeyringService, dataKeyItem); err != nil {
		t.Fatal(err)
	}
	calls := fakeConnect(t)

	code, _, errOut := runLink(t)
	const want = "nuncio link: the stored device cannot be read; run nuncio purge --confirm, then nuncio link. Nothing was linked.\n"
	if code != 1 || errOut != want {
		t.Errorf("exit %d, stderr %q; want 1 and %q", code, errOut, want)
	}
	if _, err := keys.Get(store.KeyringService, dataKeyItem); !errors.Is(err, store.ErrKeyNotFound) {
		t.Errorf("a data key was created over sealed data (get: %v)", err)
	}
	if *calls != 0 {
		t.Errorf("connected %d times, want never", *calls)
	}
}

// link over a database an older build wrote (whatsmeow's stock store, keys in plain) backs it up,
// seals it, says where the backup is, and then connects the device as before.
func TestLinkMigratesAPlainDatabaseFirst(t *testing.T) {
	isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir())
	keys := fakeKeys(t)
	path, noise := plainLinkedDatabase(t)
	calls := fakeConnect(t)

	code, out, _ := runLink(t)
	if code != 1 || *calls != 1 {
		t.Errorf("exit %d after %d connects, want 1 (the fake connect fails) after exactly 1", code, *calls)
	}
	if !strings.Contains(out, ".pre-t17-") || !strings.Contains(out, "still holds them in plain") {
		t.Errorf("stdout %q, want the migration's notice naming the backup", out)
	}
	if _, err := keys.Get(store.KeyringService, dataKeyItem); err != nil {
		t.Errorf("no data key after the migration: %v", err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(data, noise) {
		t.Error("nuncio.db still holds the noise key in plain")
	}
}

// L9 B1: while nunciod answers, link refuses before it opens the store, so it never migrates a
// plain database the daemon may be migrating too: no backup, no keyring call, nuncio.db unchanged,
// nothing connected.
func TestLinkWhileTheDaemonAnswersNeverMigrates(t *testing.T) {
	isolateData(t)
	path, _ := plainLinkedDatabase(t)
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	kr := &recordingKeyring{memKeyring: &memKeyring{items: map[string]string{}}}
	useKeyring(t, kr)
	serveFakeDaemon(t, &fakeDaemon{session: ipc.SessionOffline})
	calls := fakeConnect(t)

	code, _, errOut := runLink(t)
	const want = "nuncio link: nunciod is running and owns this device's session; stop it first. Nothing was linked.\n"
	if code != 1 || errOut != want {
		t.Errorf("exit %d, stderr %q; want 1 and %q", code, errOut, want)
	}
	if backups, _ := filepath.Glob(path + ".pre-t17-*.bak"); len(backups) != 0 {
		t.Errorf("backups %v, want none", backups)
	}
	kr.mu.Lock()
	ops := kr.ops
	kr.mu.Unlock()
	if len(ops) != 0 || *calls != 0 {
		t.Errorf("data key calls %v, %d connects; want none", ops, *calls)
	}
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(before, after) {
		t.Error("nuncio.db changed")
	}
}

// plainLinkedDatabase writes, at the isolated data dir, a nuncio.db the way builds before T17 left
// it (whatsmeow's stock store, the device's keys in plain) and returns its path and the noise
// private key.
func plainLinkedDatabase(t *testing.T) (string, []byte) {
	t.Helper()
	path, err := wa.DBPath()
	if err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(context.Background()); err != nil {
		t.Fatal(err)
	}
	device := container.NewDevice()
	device.ID = &types.JID{User: "15550009999", Device: 1, Server: types.DefaultUserServer}
	device.Account = &waAdv.ADVSignedDeviceIdentity{Details: []byte{1}, AccountSignature: make([]byte, 64), AccountSignatureKey: make([]byte, 32), DeviceSignature: make([]byte, 64)}
	if err := container.PutDevice(context.Background(), device); err != nil {
		t.Fatal(err)
	}
	noise := append([]byte(nil), device.NoiseKey.Priv[:]...)
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	return path, noise
}

// purge without --confirm lists the migration's backups and the data key among what it deletes;
// with it (no daemon) it deletes every backup.
func TestPurgeListsAndDeletesTheMigrationBackups(t *testing.T) {
	_, dataDir := populatedData(t)
	fakeKeys(t)
	t.Setenv("XDG_RUNTIME_DIR", t.TempDir()) // no core.sock there
	backup := filepath.Join(dataDir, store.DBFile+".pre-t17-20261021T133045Z.bak")
	if err := os.WriteFile(backup, []byte("synthetic"), 0o600); err != nil {
		t.Fatal(err)
	}

	code, _, errOut := runCLI("purge")
	if code != 2 || !strings.Contains(errOut, filepath.Join(dataDir, store.DBFile)+".pre-t17-*.bak") || !strings.Contains(errOut, "data key") {
		t.Errorf("exit %d, stderr %q; want 2 and the backups and the data key listed", code, errOut)
	}
	if _, err := os.Stat(backup); err != nil {
		t.Errorf("purge without --confirm touched the backup: %v", err)
	}
	code, _, errOut = runCLI("purge", "--confirm")
	if code != 0 {
		t.Fatalf("purge --confirm: exit %d, stderr %q", code, errOut)
	}
	if _, err := os.Stat(backup); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("the backup survived the purge (stat: %v)", err)
	}
}
