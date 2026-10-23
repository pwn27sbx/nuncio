package wa

import (
	"bytes"
	"context"
	"database/sql"
	"errors"
	"math/rand/v2"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waAdv"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// testKeyring is an in-memory keyring; tests never touch the real one. fail makes every call
// fail as a locked or absent Secret Service does; dropSets makes Set succeed without keeping
// anything; onGet, when set, may replace what a Get answers.
type testKeyring struct {
	mu        sync.Mutex
	items     map[string]string
	fail      bool
	dropSets  bool
	calls     int
	sets      int
	onSet     func()
	beforeSet func() // runs before a Set keeps anything, outside the lock: a slow keyring
	onGet     func(n int, value string) string
	getsSet   int // Gets since the last Set
}

var errLocked = errors.New("test: the keyring is locked")

func newTestKeyring() *testKeyring { return &testKeyring{items: map[string]string{}} }

func (k *testKeyring) Set(service, user, secret string) error {
	k.mu.Lock()
	before := k.beforeSet
	k.mu.Unlock()
	if before != nil {
		before()
	}
	k.mu.Lock()
	k.calls++
	if k.fail {
		k.mu.Unlock()
		return errLocked
	}
	k.sets++
	k.getsSet = 0
	if !k.dropSets {
		k.items[service+"/"+user] = secret
	}
	onSet := k.onSet
	k.mu.Unlock()
	if onSet != nil {
		onSet()
	}
	return nil
}

func (k *testKeyring) Get(service, user string) (string, error) {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.calls++
	if k.fail {
		return "", errLocked
	}
	v, ok := k.items[service+"/"+user]
	if !ok {
		return "", store.ErrKeyNotFound
	}
	k.getsSet++
	if k.onGet != nil {
		v = k.onGet(k.getsSet, v)
	}
	return v, nil
}

func (k *testKeyring) Delete(service, user string) error {
	k.mu.Lock()
	defer k.mu.Unlock()
	k.calls++
	if k.fail {
		return errLocked
	}
	if _, ok := k.items[service+"/"+user]; !ok {
		return store.ErrKeyNotFound
	}
	delete(k.items, service+"/"+user)
	return nil
}

func (k *testKeyring) dataKey() (string, bool) {
	k.mu.Lock()
	defer k.mu.Unlock()
	v, ok := k.items[store.KeyringService+"/store.data_key"]
	return v, ok
}

func (k *testKeyring) setFail(fail bool) {
	k.mu.Lock()
	k.fail = fail
	k.mu.Unlock()
}

func (k *testKeyring) callCount() int {
	k.mu.Lock()
	defer k.mu.Unlock()
	return k.calls
}

var sealedTestJID = types.JID{User: "15551230001", Server: types.DefaultUserServer, Device: 7}

// pairAs fills in what whatsmeow's pairing sets on a device before it saves it.
func pairAs(d *wastore.Device, jid types.JID) {
	d.ID = &jid
	d.LID = types.JID{User: "100000000000001", Server: types.HiddenUserServer, Device: jid.Device}
	d.Account = &waAdv.ADVSignedDeviceIdentity{
		Details: []byte("synthetic account details"), AccountSignature: bytes.Repeat([]byte{1}, 64),
		AccountSignatureKey: bytes.Repeat([]byte{2}, 32), DeviceSignature: bytes.Repeat([]byte{3}, 64),
	}
	d.Platform = "nuncio-test"
}

// privateOf is every private byte string of a device.
func privateOf(d *wastore.Device) map[string][]byte {
	return map[string][]byte{
		"noise private key":          append([]byte(nil), d.NoiseKey.Priv[:]...),
		"identity private key":       append([]byte(nil), d.IdentityKey.Priv[:]...),
		"signed pre-key private key": append([]byte(nil), d.SignedPreKey.Priv[:]...),
		"advertisement secret":       append([]byte(nil), d.AdvSecretKey...),
	}
}

// plainDatabase writes a nuncio.db the way builds before T17 left it: whatsmeow's stock store
// holding a linked device with its private keys in plain. It returns the path and those keys.
func plainDatabase(t *testing.T) (string, map[string][]byte) {
	t.Helper()
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "nuncio", store.DBFile)
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	defer func() { _ = db.Close() }()
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		t.Fatalf("Upgrade: %v", err)
	}
	device := container.NewDevice()
	pairAs(device, sealedTestJID)
	if err := container.PutDevice(ctx, device); err != nil {
		t.Fatalf("stock PutDevice: %v", err)
	}
	if err := device.Sessions.PutSession(ctx, "15551230002:0", bytes.Repeat([]byte{0x5e}, 48)); err != nil {
		t.Fatalf("PutSession: %v", err)
	}
	return path, privateOf(device)
}

// linkedDatabase links a device the way `nuncio link` does (OpenDevice for a link, then the save
// pairing makes) and returns the path, the keyring holding its data key and its private keys.
func linkedDatabase(t *testing.T) (string, *testKeyring, map[string][]byte) {
	t.Helper()
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "nuncio", store.DBFile)
	kr := newTestKeyring()
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	defer func() { _ = db.Close() }()
	device, _, err := OpenDevice(ctx, db, path, kr, true)
	if err != nil {
		t.Fatalf("OpenDevice for a link: %v", err)
	}
	pairAs(device, sealedTestJID)
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	return path, kr, privateOf(device)
}

func openDB(t *testing.T, path string) *sql.DB {
	t.Helper()
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	return db
}

// dirFiles reads every file of dir by name.
func dirFiles(t *testing.T, dir string) map[string][]byte {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	files := map[string][]byte{}
	for _, e := range entries {
		data, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			t.Fatal(err)
		}
		files[e.Name()] = data
	}
	return files
}

func backupsOf(t *testing.T, path string) []string {
	t.Helper()
	found, err := filepath.Glob(path + ".pre-t17-*.bak")
	if err != nil {
		t.Fatal(err)
	}
	return found
}

func rowCount(t *testing.T, db *sql.DB, table string) int {
	t.Helper()
	var tables int
	if err := db.QueryRow(`SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?`, table).Scan(&tables); err != nil {
		t.Fatal(err)
	}
	if tables == 0 {
		return 0
	}
	var n int
	if err := db.QueryRow(`SELECT COUNT(*) FROM ` + table).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func assertSameKeys(t *testing.T, got *wastore.Device, want map[string][]byte) {
	t.Helper()
	if got == nil || got.ID == nil || *got.ID != sealedTestJID {
		t.Fatalf("device = %+v, want the linked one", got)
	}
	for name, value := range privateOf(got) {
		if !bytes.Equal(value, want[name]) {
			t.Errorf("the opened device's %s differs from the stored one", name)
		}
	}
}

// The daemon's start over a database written before T17 migrates it (a backup first), and then
// opens the device sealed: no private key byte is left in nuncio.db or its WAL.
func TestOpenDeviceMigratesAPlainDatabaseThenOpensItSealed(t *testing.T) {
	path, private := plainDatabase(t)
	kr := newTestKeyring()
	db := openDB(t, path)

	device, res, err := OpenDevice(context.Background(), db, path, kr, false)
	if err != nil {
		t.Fatalf("OpenDevice: %v", err)
	}
	if !res.Migrated || res.CleanupPending {
		t.Errorf("migration result = %+v, want migrated and cleaned up", res)
	}
	assertSameKeys(t, device, private)
	if got := backupsOf(t, path); len(got) != 1 {
		t.Errorf("backups = %v, want exactly one", got)
	}
	if _, ok := kr.dataKey(); !ok {
		t.Error("no data key in the keyring after the migration")
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	for name, data := range dirFiles(t, filepath.Dir(path)) {
		if strings.Contains(name, ".pre-t17-") {
			continue // the backup holds them in plain, and says so
		}
		for what, secret := range private {
			if bytes.Contains(data, secret) {
				t.Errorf("%s still holds the %s in plain", name, what)
			}
		}
	}
}

// With the keyring locked or absent the device stays closed and nothing is written: not the
// database (plain or already sealed), not a backup, not the keyring.
func TestOpenDeviceWithTheKeyringUnavailableWritesNothing(t *testing.T) {
	plain, _ := plainDatabase(t)
	sealed, _, _ := linkedDatabase(t)
	for name, path := range map[string]string{"plain": plain, "sealed": sealed} {
		t.Run(name, func(t *testing.T) {
			before := dirFiles(t, filepath.Dir(path))
			kr := newTestKeyring()
			kr.fail = true
			db, err := store.Open(path)
			if err != nil {
				t.Fatal(err)
			}
			device, _, err := OpenDevice(context.Background(), db, path, kr, false)
			_ = db.Close()
			if !errors.Is(err, store.ErrKeyringUnavailable) || device != nil {
				t.Fatalf("OpenDevice = %v, %v; want no device and ErrKeyringUnavailable", device, err)
			}
			after := dirFiles(t, filepath.Dir(path))
			if len(after) != len(before) {
				t.Errorf("files %d before, %d after; want the same", len(before), len(after))
			}
			if !bytes.Equal(after[store.DBFile], before[store.DBFile]) {
				t.Error("nuncio.db changed while the keyring was unavailable")
			}
			if got := backupsOf(t, path); len(got) != 0 && name == "plain" {
				t.Errorf("a backup was written while the keyring was unavailable: %v", got)
			}
		})
	}
}

// A data key that is gone while sealed data exists is never replaced: the device cannot be read,
// for the daemon and for a link alike, and the keyring stays empty.
func TestOpenDeviceWithTheKeyMissingOverSealedDataCreatesNoKey(t *testing.T) {
	path, kr, _ := linkedDatabase(t)
	if err := kr.Delete(store.KeyringService, "store.data_key"); err != nil {
		t.Fatal(err)
	}
	for _, forLink := range []bool{false, true} {
		db := openDB(t, path)
		device, _, err := OpenDevice(context.Background(), db, path, kr, forLink)
		if !errors.Is(err, ErrDeviceUnreadable) || device != nil {
			t.Errorf("OpenDevice(link %t) = %v, %v; want ErrDeviceUnreadable", forLink, device, err)
		}
		if _, ok := kr.dataKey(); ok {
			t.Errorf("OpenDevice(link %t) created a data key over sealed data", forLink)
		}
		_ = db.Close()
	}
	const want = "the stored device cannot be read; run nuncio purge --confirm, then nuncio link"
	if ErrDeviceUnreadable.Error() != want {
		t.Errorf("ErrDeviceUnreadable = %q, want %q", ErrDeviceUnreadable, want)
	}
}

// Another data key in the keyring opens nothing and is left as it is.
func TestOpenDeviceWithAnotherDataKeyCannotReadTheDevice(t *testing.T) {
	path, _, _ := linkedDatabase(t)
	other := newTestKeyring()
	if _, err := store.LoadDataKey(other, true); err != nil {
		t.Fatal(err)
	}
	otherKey, _ := other.dataKey()
	db := openDB(t, path)
	if _, _, err := OpenDevice(context.Background(), db, path, other, false); !errors.Is(err, ErrDeviceUnreadable) {
		t.Errorf("OpenDevice with another key = %v, want ErrDeviceUnreadable", err)
	}
	if got, _ := other.dataKey(); got != otherKey {
		t.Error("the keyring's data key was replaced")
	}
}

// A tampered sealed device row fails to open; it is never read as plain.
func TestOpenDeviceRefusesATamperedDeviceRow(t *testing.T) {
	path, kr, _ := linkedDatabase(t)
	db := openDB(t, path)
	var sealed []byte
	if err := db.QueryRow(`SELECT sealed FROM nuncio_device`).Scan(&sealed); err != nil {
		t.Fatal(err)
	}
	sealed[len(sealed)-1] ^= 0x01
	if _, err := db.Exec(`UPDATE nuncio_device SET sealed = ?`, sealed); err != nil {
		t.Fatal(err)
	}
	device, _, err := OpenDevice(context.Background(), db, path, kr, false)
	if !errors.Is(err, ErrDeviceUnreadable) || !errors.Is(err, store.ErrUnseal) || device != nil {
		t.Errorf("OpenDevice over a tampered row = %v, %v; want ErrDeviceUnreadable wrapping ErrUnseal", device, err)
	}
}

// A database with no device needs no key: the daemon opens nothing and never asks the keyring,
// so it never creates a key either.
func TestOpenDeviceForTheDaemonWithoutADeviceNeverTouchesTheKeyring(t *testing.T) {
	path := filepath.Join(t.TempDir(), store.DBFile)
	db := openDB(t, path)
	if err := store.EnsureSchema(db); err != nil {
		t.Fatal(err)
	}
	kr := newTestKeyring()
	device, _, err := OpenDevice(context.Background(), db, path, kr, false)
	if err != nil || device != nil {
		t.Errorf("OpenDevice = %v, %v; want no device and no error", device, err)
	}
	if n := kr.callCount(); n != 0 {
		t.Errorf("the keyring was called %d times, want never", n)
	}
}

// A link creates the data key only when pairing saves the device: before that nothing is in the
// keyring or the database's rows, so a link that never pairs leaves nothing. On the save it writes
// the key and reads it back before the device row exists, and the device then opens sealed.
func TestOpenDeviceForALinkCreatesTheKeyOnlyWhenTheDeviceIsSaved(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), store.DBFile)
	db := openDB(t, path)
	kr := newTestKeyring()
	devicesAtSet := -1
	kr.onSet = func() { devicesAtSet = rowCount(t, db, "whatsmeow_device") }

	device, _, err := OpenDevice(ctx, db, path, kr, true)
	if err != nil || device == nil || device.ID != nil {
		t.Fatalf("OpenDevice for a link = %v, %v; want a new unpaired device", device, err)
	}
	if _, ok := kr.dataKey(); ok {
		t.Error("a data key exists before the device was paired")
	}
	for _, table := range []string{"whatsmeow_device", "nuncio_device", "nuncio_seal_meta"} {
		if n := rowCount(t, db, table); n != 0 {
			t.Errorf("%s holds %d rows before pairing, want 0", table, n)
		}
	}
	// A save before pairing (no JID yet) is refused before anything is written.
	if err := device.Save(ctx); err == nil {
		t.Error("Save of an unpaired device succeeded")
	}
	if _, ok := kr.dataKey(); ok {
		t.Error("a refused save created the data key")
	}

	pairAs(device, sealedTestJID)
	private := privateOf(device)
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	if devicesAtSet != 0 {
		t.Errorf("the device table held %d rows when the key was written, want 0", devicesAtSet)
	}
	if rowCount(t, db, "nuncio_seal_meta") != 1 {
		t.Error("no seal check row after the first save")
	}
	opened, _, err := OpenDevice(ctx, db, path, kr, false)
	if err != nil {
		t.Fatalf("OpenDevice after the link: %v", err)
	}
	assertSameKeys(t, opened, private)
}

// A keyring that drops the key (the write succeeds, the read back finds nothing) fails the save,
// and no device row is written under a key nobody holds.
func TestOpenDeviceForALinkSavesNothingWhenTheKeyDoesNotReadBack(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), store.DBFile)
	db := openDB(t, path)
	kr := newTestKeyring()
	kr.dropSets = true
	device, _, err := OpenDevice(ctx, db, path, kr, true)
	if err != nil {
		t.Fatalf("OpenDevice: %v", err)
	}
	pairAs(device, sealedTestJID)
	if err := device.Save(ctx); !errors.Is(err, store.ErrKeyringUnavailable) {
		t.Errorf("Save = %v, want ErrKeyringUnavailable", err)
	}
	for _, table := range []string{"whatsmeow_device", "nuncio_device", "nuncio_seal_meta"} {
		if n := rowCount(t, db, table); n != 0 {
			t.Errorf("%s holds %d rows after a failed save, want 0", table, n)
		}
	}
}

// After the seal check row is written the keyring is read again (L5 A1): a key another process
// put there meanwhile fails the save before the device is written under the wrong one.
func TestOpenDeviceForALinkRereadsTheKeyAfterTheSealCheck(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), store.DBFile)
	db := openDB(t, path)
	kr := newTestKeyring()
	otherKr := newTestKeyring()
	if _, err := store.LoadDataKey(otherKr, true); err != nil {
		t.Fatal(err)
	}
	other, _ := otherKr.dataKey()
	// The read back right after the write sees our key; the read after the check row sees another.
	kr.onGet = func(n int, v string) string {
		if n >= 2 {
			return other
		}
		return v
	}
	device, _, err := OpenDevice(ctx, db, path, kr, true)
	if err != nil {
		t.Fatalf("OpenDevice: %v", err)
	}
	pairAs(device, sealedTestJID)
	if err := device.Save(ctx); err == nil {
		t.Fatal("Save succeeded although the keyring no longer holds the key")
	}
	if n := rowCount(t, db, "whatsmeow_device"); n != 0 {
		t.Errorf("whatsmeow_device holds %d rows, want 0", n)
	}
}

// A link over a device that is already linked opens it with the key in the keyring and creates
// none.
func TestOpenDeviceForALinkReopensALinkedDevice(t *testing.T) {
	path, kr, private := linkedDatabase(t)
	before, _ := kr.dataKey()
	db := openDB(t, path)
	device, _, err := OpenDevice(context.Background(), db, path, kr, true)
	if err != nil {
		t.Fatalf("OpenDevice: %v", err)
	}
	assertSameKeys(t, device, private)
	if after, _ := kr.dataKey(); after != before {
		t.Error("the data key changed")
	}
}

// migrateTwice runs, on two handles of the plain database at path, the daemon's OpenDevice and a
// link's at the same time, as nunciod (its start or a gate retry) and `nuncio link` can; returned,
// when set, is called as each of them returns. It then checks the end state: neither failed (the
// one that came second finds the database sealed, at most with the other's cleanup still
// pending), a fresh daemon OpenDevice reads the device, and the keyring holds the one key ever
// written, the one the database is sealed under.
func migrateTwice(t *testing.T, path string, private map[string][]byte, kr *testKeyring, returned func()) {
	t.Helper()
	ctx := context.Background()
	dbA, dbB := openDB(t, path), openDB(t, path)
	var wg sync.WaitGroup
	errs := make([]error, 2)
	for i, forLink := range []bool{false, true} {
		db := []*sql.DB{dbA, dbB}[i]
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _, errs[i] = OpenDevice(ctx, db, path, kr, forLink)
			if returned != nil {
				returned()
			}
		}()
	}
	wg.Wait()
	for i, err := range errs {
		if err != nil && !errors.Is(err, ErrCleanupPending) {
			t.Errorf("OpenDevice %d: %v, want it to open or wait for the cleanup", i, err)
		}
	}
	device, _, err := OpenDevice(ctx, openDB(t, path), path, kr, false)
	if err != nil {
		t.Fatalf("OpenDevice after both migrations: %v", err)
	}
	assertSameKeys(t, device, private)
	kr.mu.Lock()
	sets := kr.sets
	kr.mu.Unlock()
	if sets != 1 {
		t.Errorf("the data key was written %d times, want once: a second write replaces the sealing key", sets)
	}
}

// L9 B1: two processes migrating the same plain database never leave the keyring holding a key
// other than the one the committed data is sealed under, whatever the keyring's latency.
func TestConcurrentMigrationsKeepTheSealingKey(t *testing.T) {
	for i := range 40 {
		path, private := plainDatabase(t)
		kr := newTestKeyring()
		kr.beforeSet = func() { time.Sleep(time.Duration(rand.IntN(5001)) * time.Microsecond) }
		migrateTwice(t, path, private, kr, nil)
		if t.Failed() {
			t.Fatalf("iteration %d", i)
		}
	}
}

// The deterministic interleaving: the first write of the data key is held until the other
// process's OpenDevice has returned (or a second has passed). The other process must not seal
// under a key of its own meanwhile.
func TestConcurrentMigrationsWithTheFirstKeyWriteHeld(t *testing.T) {
	path, private := plainDatabase(t)
	kr := newTestKeyring()
	returned := make(chan struct{})
	var first, closeOnce sync.Once
	kr.beforeSet = func() {
		held := false
		first.Do(func() { held = true })
		if held {
			select {
			case <-returned:
			case <-time.After(time.Second):
			}
		}
	}
	migrateTwice(t, path, private, kr, func() { closeOnce.Do(func() { close(returned) }) })
}
