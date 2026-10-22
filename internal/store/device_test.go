package store

import (
	"bytes"
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/google/uuid"
	"github.com/zalando/go-keyring"
	"go.mau.fi/whatsmeow/proto/waAdv"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
)

// fakeKeyring is an in-memory Secret Service, so the device store can be
// tested without touching the owner's keyring.
type fakeKeyring struct {
	mu     sync.Mutex
	items  map[string]string
	setErr error
	getErr error
	delErr error
}

func newFakeKeyring() *fakeKeyring {
	return &fakeKeyring{items: map[string]string{}}
}

func itemKey(service, user string) string { return service + "\x00" + user }

func (f *fakeKeyring) Set(service, user, secret string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.setErr != nil {
		return f.setErr
	}
	f.items[itemKey(service, user)] = secret
	return nil
}

func (f *fakeKeyring) Get(service, user string) (string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.getErr != nil {
		return "", f.getErr
	}
	secret, ok := f.items[itemKey(service, user)]
	if !ok {
		return "", ErrKeyNotFound
	}
	return secret, nil
}

func (f *fakeKeyring) Delete(service, user string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.delErr != nil {
		return f.delErr
	}
	delete(f.items, itemKey(service, user))
	return nil
}

func (f *fakeKeyring) count() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.items)
}

// testSealer seals under a fresh random data key.
func testSealer(t *testing.T) *Sealer {
	t.Helper()
	key := make([]byte, dataKeySize)
	if _, err := rand.Read(key); err != nil {
		t.Fatalf("rand: %v", err)
	}
	s, err := NewSealer(key)
	if err != nil {
		t.Fatalf("NewSealer: %v", err)
	}
	return s
}

// openStoreDB opens dir/nuncio.db through the store's real Open (WAL, foreign keys).
func openStoreDB(t *testing.T, dir string) *sql.DB {
	t.Helper()
	db, err := Open(filepath.Join(dir, DBFile))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	return db
}

func newTestDeviceStore(t *testing.T, db *sql.DB, sealer *Sealer) *DeviceStore {
	t.Helper()
	s, err := NewDeviceStore(context.Background(), db, sealer)
	if err != nil {
		t.Fatalf("NewDeviceStore: %v", err)
	}
	return s
}

func randBytes(t *testing.T, n int) []byte {
	t.Helper()
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		t.Fatalf("rand: %v", err)
	}
	return buf
}

// pairedDevice is a device shaped like the one whatsmeow saves after a pairing: key material, an
// advertisement secret, an account signature and a JID. The numbers are fake.
func pairedDevice(t *testing.T, jid types.JID) *wastore.Device {
	t.Helper()
	identity := keys.NewKeyPair()
	return &wastore.Device{
		NoiseKey:       keys.NewKeyPair(),
		IdentityKey:    identity,
		SignedPreKey:   identity.CreateSignedPreKey(7),
		RegistrationID: 4242,
		AdvSecretKey:   randBytes(t, 32),
		ID:             &jid,
		LID:            types.JID{User: "100000000000001", Server: types.HiddenUserServer, Device: jid.Device},
		Account: &waAdv.ADVSignedDeviceIdentity{
			Details:             []byte("synthetic account details"),
			AccountSignature:    randBytes(t, 64),
			AccountSignatureKey: randBytes(t, 32),
			DeviceSignature:     randBytes(t, 64),
		},
		Platform: "nuncio-test",
		PushName: "synthetic",
	}
}

var ourTestJID = types.JID{User: "15551230001", Server: types.DefaultUserServer, Device: 7}

// devicePrivate is every private byte string of the device.
func devicePrivate(d *wastore.Device) map[string][]byte {
	return map[string][]byte{
		"noise private key":          d.NoiseKey.Priv[:],
		"identity private key":       d.IdentityKey.Priv[:],
		"signed pre-key private key": d.SignedPreKey.Priv[:],
		"advertisement secret":       d.AdvSecretKey,
	}
}

// dbFiles reads nuncio.db and whichever of its -wal and -shm exist in dir.
func dbFiles(t *testing.T, dir string) map[string][]byte {
	t.Helper()
	files := map[string][]byte{}
	for _, name := range []string{DBFile, DBFile + "-wal", DBFile + "-shm"} {
		data, err := os.ReadFile(filepath.Join(dir, name))
		if errors.Is(err, os.ErrNotExist) {
			continue
		}
		if err != nil {
			t.Fatalf("read %s: %v", name, err)
		}
		files[name] = data
	}
	if _, ok := files[DBFile]; !ok {
		t.Fatalf("no %s in %s to check", DBFile, dir)
	}
	return files
}

// assertNoSecretsOnDisk fails if any secret, raw or in base64, shows up in a database file.
func assertNoSecretsOnDisk(t *testing.T, dir string, secrets map[string][]byte) {
	t.Helper()
	for file, data := range dbFiles(t, dir) {
		for name, value := range secrets {
			if len(value) == 0 {
				t.Fatalf("secret %s is empty", name)
			}
			if bytes.Contains(data, value) {
				t.Errorf("%s appears raw in %s", name, file)
			}
			if bytes.Contains(data, []byte(base64.StdEncoding.EncodeToString(value))) {
				t.Errorf("%s appears in base64 in %s", name, file)
			}
		}
	}
}

func assertSameDevice(t *testing.T, got, want *wastore.Device) {
	t.Helper()
	if *got.NoiseKey.Priv != *want.NoiseKey.Priv || *got.NoiseKey.Pub != *want.NoiseKey.Pub {
		t.Errorf("noise key differs")
	}
	if *got.IdentityKey.Priv != *want.IdentityKey.Priv || *got.IdentityKey.Pub != *want.IdentityKey.Pub {
		t.Errorf("identity key differs")
	}
	if *got.SignedPreKey.Priv != *want.SignedPreKey.Priv || *got.SignedPreKey.Pub != *want.SignedPreKey.Pub ||
		got.SignedPreKey.KeyID != want.SignedPreKey.KeyID || *got.SignedPreKey.Signature != *want.SignedPreKey.Signature {
		t.Errorf("signed pre-key differs")
	}
	if !bytes.Equal(got.AdvSecretKey, want.AdvSecretKey) {
		t.Errorf("advertisement secret differs")
	}
	if got.RegistrationID != want.RegistrationID || got.Platform != want.Platform || got.PushName != want.PushName ||
		got.BusinessName != want.BusinessName || got.LIDMigrationTimestamp != want.LIDMigrationTimestamp ||
		got.CompanionMetaNonce != want.CompanionMetaNonce || got.FacebookUUID != want.FacebookUUID {
		t.Errorf("device metadata differs: got %+v, want %+v", got, want)
	}
	if got.ID == nil || got.ID.String() != want.ID.String() || got.LID != want.LID {
		t.Errorf("id = %v / %v, want %v / %v", got.ID, got.LID, want.ID, want.LID)
	}
	if !bytes.Equal(got.Account.Details, want.Account.Details) ||
		!bytes.Equal(got.Account.AccountSignature, want.Account.AccountSignature) ||
		!bytes.Equal(got.Account.AccountSignatureKey, want.Account.AccountSignatureKey) ||
		!bytes.Equal(got.Account.DeviceSignature, want.Account.DeviceSignature) {
		t.Errorf("account signature differs")
	}
}

func TestDeviceStoreRoundTripsThroughTheSealedTables(t *testing.T) {
	ctx := context.Background()
	s := newTestDeviceStore(t, openStoreDB(t, t.TempDir()), testSealer(t))

	device := pairedDevice(t, ourTestJID)
	device.FacebookUUID = uuid.MustParse("00000000-0000-4000-8000-000000000001")
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	if _, ok := device.Sessions.(*sealedStore); !ok || !device.Initialized {
		t.Fatalf("Save did not wire the sealed stores: %T", device.Sessions)
	}

	loaded, err := s.LoadDevice(ctx)
	if err != nil {
		t.Fatalf("LoadDevice: %v", err)
	}
	assertSameDevice(t, loaded, device)
	if _, ok := loaded.PreKeys.(*sealedStore); !ok {
		t.Errorf("loaded device's pre-keys are %T, want the sealed store", loaded.PreKeys)
	}
	if loaded.LIDs == nil || loaded.Container != s || !loaded.Initialized {
		t.Errorf("loaded device is not wired: LIDs %v, container %v, initialized %v", loaded.LIDs, loaded.Container, loaded.Initialized)
	}

	// Saving the loaded device again changes the public fields like whatsmeow's container does.
	loaded.PushName = "renamed"
	if err := loaded.Save(ctx); err != nil {
		t.Fatalf("Save on the loaded device: %v", err)
	}
	again, err := s.LoadDevice(ctx)
	if err != nil {
		t.Fatalf("second LoadDevice: %v", err)
	}
	device.PushName = "renamed"
	assertSameDevice(t, again, device)
}

// Like whatsmeow's container, saving a stored device again never replaces its keys.
func TestDeviceStoreKeepsTheFirstKeysOfADevice(t *testing.T) {
	ctx := context.Background()
	s := newTestDeviceStore(t, openStoreDB(t, t.TempDir()), testSealer(t))
	device := pairedDevice(t, ourTestJID)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	want := *device.NoiseKey.Priv
	device.NoiseKey = keys.NewKeyPair()
	if err := device.Save(ctx); err != nil {
		t.Fatalf("second Save: %v", err)
	}
	loaded, err := s.LoadDevice(ctx)
	if err != nil {
		t.Fatalf("LoadDevice: %v", err)
	}
	if *loaded.NoiseKey.Priv != want {
		t.Errorf("a second Save replaced the stored noise key")
	}
}

func TestDeviceStoreWritesAZeroedStubRow(t *testing.T) {
	ctx := context.Background()
	db := openStoreDB(t, t.TempDir())
	s := newTestDeviceStore(t, db, testSealer(t))
	device := pairedDevice(t, ourTestJID)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}

	var noise, identity, preKey, adv, sig []byte
	if err := db.QueryRow(`SELECT noise_key, identity_key, signed_pre_key, adv_key, signed_pre_key_sig
		FROM whatsmeow_device WHERE jid = ?`, ourTestJID.String()).Scan(&noise, &identity, &preKey, &adv, &sig); err != nil {
		t.Fatalf("read the stub row: %v", err)
	}
	zero := make([]byte, 32)
	for name, col := range map[string][]byte{"noise_key": noise, "identity_key": identity, "signed_pre_key": preKey, "adv_key": adv} {
		if !bytes.Equal(col, zero) {
			t.Errorf("whatsmeow_device.%s = %x, want 32 zero bytes", name, col)
		}
	}
	if !bytes.Equal(sig, device.SignedPreKey.Signature[:]) {
		t.Errorf("the public signed pre-key signature is not kept in the stub row")
	}
	var sealed []byte
	if err := db.QueryRow(`SELECT sealed FROM nuncio_device WHERE jid = ?`, ourTestJID.String()).Scan(&sealed); err != nil {
		t.Fatalf("read the sealed row: %v", err)
	}
	if len(sealed) == 0 || sealed[0] != sealVersion {
		t.Errorf("nuncio_device.sealed is not in the sealed format")
	}
}

func TestDeviceStoreKeepsTheKeysOutOfTheDBFiles(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	db := openStoreDB(t, dir)
	s := newTestDeviceStore(t, db, testSealer(t))
	device := pairedDevice(t, ourTestJID)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	assertNoSecretsOnDisk(t, dir, devicePrivate(device))
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	assertNoSecretsOnDisk(t, dir, devicePrivate(device))
}

func TestDeviceStoreRefusesAnotherDataKey(t *testing.T) {
	ctx := context.Background()
	db := openStoreDB(t, t.TempDir())
	device := pairedDevice(t, ourTestJID)
	device.Container = newTestDeviceStore(t, db, testSealer(t))
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}

	_, err := newTestDeviceStore(t, db, testSealer(t)).LoadDevice(ctx)
	if !errors.Is(err, ErrUnseal) {
		t.Fatalf("LoadDevice under another key = %v, want ErrUnseal", err)
	}
	if !strings.Contains(err.Error(), "nuncio_device") || strings.Contains(err.Error(), ourTestJID.User) {
		t.Errorf("error %q must name the table and nothing else", err)
	}
}

func TestDeviceStoreLoadWithoutADevice(t *testing.T) {
	s := newTestDeviceStore(t, openStoreDB(t, t.TempDir()), testSealer(t))
	if _, err := s.LoadDevice(context.Background()); !errors.Is(err, ErrNoDevice) {
		t.Fatalf("LoadDevice on an empty store = %v, want ErrNoDevice", err)
	}
}

// A device whatsmeow's own container wrote (plain keys, no sealed row) is reported, never read
// as an unlinked store.
func TestDeviceStoreReportsADeviceThatIsNotSealed(t *testing.T) {
	ctx := context.Background()
	db := openStoreDB(t, t.TempDir())
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		t.Fatalf("Upgrade: %v", err)
	}
	if err := container.PutDevice(ctx, pairedDevice(t, ourTestJID)); err != nil {
		t.Fatalf("stock PutDevice: %v", err)
	}
	s := newTestDeviceStore(t, db, testSealer(t))
	if _, err := s.LoadDevice(ctx); !errors.Is(err, ErrDeviceNotSealed) {
		t.Fatalf("LoadDevice on a plain device = %v, want ErrDeviceNotSealed", err)
	}
}

func TestDeviceStoreSaveNeedsAJID(t *testing.T) {
	ctx := context.Background()
	db := openStoreDB(t, t.TempDir())
	s := newTestDeviceStore(t, db, testSealer(t))
	device := s.NewDevice()
	if err := device.Save(ctx); !errors.Is(err, sqlstore.ErrDeviceIDMustBeSet) {
		t.Fatalf("Save without a JID = %v, want ErrDeviceIDMustBeSet", err)
	}
	var n int
	if err := db.QueryRow(`SELECT (SELECT COUNT(*) FROM whatsmeow_device) + (SELECT COUNT(*) FROM nuncio_device)`).Scan(&n); err != nil || n != 0 {
		t.Errorf("rows after a refused Save = %d (%v), want 0", n, err)
	}
}

func TestNewDeviceStoreRequiresASealer(t *testing.T) {
	if _, err := NewDeviceStore(context.Background(), openStoreDB(t, t.TempDir()), nil); err == nil {
		t.Fatal("NewDeviceStore without a sealer: got nil error")
	}
}

// TestSecretServiceAgainstTheRealKeyring exercises the real keyring adapter. It is off by default
// because it writes into the owner's keyring, and it always cleans its uniquely named item up.
func TestSecretServiceAgainstTheRealKeyring(t *testing.T) {
	if os.Getenv("NUNCIO_KEYRING_TESTS") != "1" {
		t.Skip("set NUNCIO_KEYRING_TESTS=1 to exercise the owner's real Secret Service")
	}
	service := fmt.Sprintf("nuncio-test-%d", os.Getpid())
	t.Cleanup(func() {
		if err := keyring.DeleteAll(service); err != nil {
			t.Errorf("clean up keyring service %s: %v", service, err)
		}
	})
	var kr Keyring = SecretService{}
	if err := kr.Set(service, "probe", "synthetic"); err != nil {
		t.Fatalf("Set: %v", err)
	}
	if got, err := kr.Get(service, "probe"); err != nil || got != "synthetic" {
		t.Fatalf("Get = %q, %v", got, err)
	}
	if err := kr.Delete(service, "probe"); err != nil {
		t.Fatalf("Delete: %v", err)
	}
	if _, err := kr.Get(service, "probe"); !errors.Is(err, ErrKeyNotFound) {
		t.Fatalf("Get after Delete = %v, want ErrKeyNotFound", err)
	}
}
