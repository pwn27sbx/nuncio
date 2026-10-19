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

	"github.com/zalando/go-keyring"
	wastore "go.mau.fi/whatsmeow/store"
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

// syntheticDevice is a device shaped like the one whatsmeow keeps before any
// pairing: key material and an advertisement secret, no account.
func syntheticDevice(t *testing.T, jid *types.JID) *wastore.Device {
	t.Helper()
	identity := keys.NewKeyPair()
	advSecret := make([]byte, 32)
	if _, err := rand.Read(advSecret); err != nil {
		t.Fatalf("rand: %v", err)
	}
	return &wastore.Device{
		NoiseKey:       keys.NewKeyPair(),
		IdentityKey:    identity,
		SignedPreKey:   identity.CreateSignedPreKey(7),
		RegistrationID: 4242,
		AdvSecretKey:   advSecret,
		ID:             jid,
		Platform:       "nuncio-test",
		PushName:       "unpaired spike",
		Initialized:    true,
	}
}

// keyMaterial is every byte of the device's key material, in the raw and the
// base64 form. None of it may reach the SQLite file.
func keyMaterial(t *testing.T, d *wastore.Device) map[string][]byte {
	t.Helper()
	raw := map[string][]byte{
		"noise private key":          d.NoiseKey.Priv[:],
		"noise public key":           d.NoiseKey.Pub[:],
		"identity private key":       d.IdentityKey.Priv[:],
		"identity public key":        d.IdentityKey.Pub[:],
		"signed pre-key private key": d.SignedPreKey.Priv[:],
		"signed pre-key public key":  d.SignedPreKey.Pub[:],
		"signed pre-key signature":   d.SignedPreKey.Signature[:],
		"advertisement secret":       d.AdvSecretKey,
	}
	all := make(map[string][]byte, len(raw)*2)
	for name, value := range raw {
		all[name] = value
		all[name+" (base64)"] = []byte(base64.StdEncoding.EncodeToString(value))
	}
	return all
}

// assertNoKeyMaterialOnDisk reads every file the database wrote and fails if
// any byte of key material shows up in it.
func assertNoKeyMaterialOnDisk(t *testing.T, dir string, material map[string][]byte) {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("read %s: %v", dir, err)
	}
	checked := 0
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		path := filepath.Join(dir, entry.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("read %s: %v", path, err)
		}
		checked++
		for name, value := range material {
			if len(value) == 0 {
				continue
			}
			if bytes.Contains(data, value) {
				t.Errorf("%s appears in %s", name, path)
			}
		}
	}
	if checked == 0 {
		t.Fatalf("no database files found in %s to check", dir)
	}
}

func assertSameDevice(t *testing.T, got, want *wastore.Device) {
	t.Helper()
	if !bytes.Equal(got.NoiseKey.Priv[:], want.NoiseKey.Priv[:]) ||
		!bytes.Equal(got.NoiseKey.Pub[:], want.NoiseKey.Pub[:]) {
		t.Errorf("noise key differs:\n got %x\nwant %x", got.NoiseKey.Priv[:], want.NoiseKey.Priv[:])
	}
	if !bytes.Equal(got.IdentityKey.Priv[:], want.IdentityKey.Priv[:]) ||
		!bytes.Equal(got.IdentityKey.Pub[:], want.IdentityKey.Pub[:]) {
		t.Errorf("identity key differs:\n got %x\nwant %x", got.IdentityKey.Priv[:], want.IdentityKey.Priv[:])
	}
	if !bytes.Equal(got.SignedPreKey.Priv[:], want.SignedPreKey.Priv[:]) ||
		!bytes.Equal(got.SignedPreKey.Pub[:], want.SignedPreKey.Pub[:]) ||
		got.SignedPreKey.KeyID != want.SignedPreKey.KeyID ||
		!bytes.Equal(got.SignedPreKey.Signature[:], want.SignedPreKey.Signature[:]) {
		t.Errorf("signed pre-key differs: got %+v, want %+v", got.SignedPreKey, want.SignedPreKey)
	}
	if !bytes.Equal(got.AdvSecretKey, want.AdvSecretKey) {
		t.Errorf("advertisement secret differs:\n got %x\nwant %x", got.AdvSecretKey, want.AdvSecretKey)
	}
	if got.RegistrationID != want.RegistrationID {
		t.Errorf("registration id = %d, want %d", got.RegistrationID, want.RegistrationID)
	}
	if got.Platform != want.Platform || got.PushName != want.PushName || got.Initialized != want.Initialized {
		t.Errorf("device metadata = %+v, want %+v", got, want)
	}
	switch {
	case got.ID == nil && want.ID == nil:
	case got.ID == nil || want.ID == nil:
		t.Errorf("id = %v, want %v", got.ID, want.ID)
	case got.ID.String() != want.ID.String():
		t.Errorf("id = %s, want %s", got.ID, want.ID)
	}
}

func newTestDeviceStore(t *testing.T, dir string, kr Keyring) (*DeviceStore, *sql.DB) {
	t.Helper()
	db, _ := openDBIn(t, dir)
	s, err := NewDeviceStore(db, kr)
	if err != nil {
		t.Fatalf("NewDeviceStore: %v", err)
	}
	return s, db
}

func TestDeviceStoreRoundTripsThroughKeyring(t *testing.T) {
	pairingJID := types.NewJID("15551230001", types.DefaultUserServer)
	cases := []struct {
		name string
		jid  *types.JID
	}{
		{name: "unpaired device", jid: nil},
		{name: "device carrying a JID", jid: &pairingJID},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			ctx := context.Background()
			kr := newFakeKeyring()
			s, _ := newTestDeviceStore(t, t.TempDir(), kr)

			device := syntheticDevice(t, tc.jid)
			device.Container = s
			if err := device.Save(ctx); err != nil {
				t.Fatalf("Save: %v", err)
			}
			if got := kr.count(); got != 4 {
				t.Fatalf("keyring holds %d items, want the 4 pieces of key material", got)
			}

			loaded, err := s.LoadDevice(ctx)
			if err != nil {
				t.Fatalf("LoadDevice: %v", err)
			}
			assertSameDevice(t, loaded, device)

			// The loaded device must be usable: saving it again goes through
			// the same container and changes nothing.
			if err := loaded.Save(ctx); err != nil {
				t.Fatalf("Save on the loaded device: %v", err)
			}
			again, err := s.LoadDevice(ctx)
			if err != nil {
				t.Fatalf("second LoadDevice: %v", err)
			}
			assertSameDevice(t, again, device)
		})
	}
}

func TestDeviceStoreKeepsKeyMaterialOutOfSQLite(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	kr := newFakeKeyring()
	s, db := newTestDeviceStore(t, dir, kr)

	device := syntheticDevice(t, nil)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close the database: %v", err)
	}

	material := keyMaterial(t, device)
	assertNoKeyMaterialOnDisk(t, dir, material)

	// A sanity check that the assertion can fail: the material really does
	// live in the keyring, in base64.
	item, err := kr.Get(KeyringService, "device.noise_key")
	if err != nil {
		t.Fatalf("keyring item: %v", err)
	}
	if item != base64.StdEncoding.EncodeToString(device.NoiseKey.Priv[:]) {
		t.Fatal("the keyring does not hold the base64 noise private key")
	}
}

func TestDeviceStoreFailsClosedWithoutKeyring(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	kr := newFakeKeyring()
	kr.setErr = errors.New("no secret service is running")
	s, db := newTestDeviceStore(t, dir, kr)

	device := syntheticDevice(t, nil)
	device.Container = s
	err := device.Save(ctx)
	if err == nil {
		t.Fatal("Save with an unavailable keyring: got nil error")
	}
	if !strings.Contains(err.Error(), "keyring") {
		t.Errorf("error %q must name the keyring so the failure is not mistaken for something else", err)
	}

	if _, err := s.LoadDevice(ctx); !errors.Is(err, ErrNoDevice) {
		t.Errorf("LoadDevice after a failed Save = %v, want ErrNoDevice", err)
	}

	if err := db.Close(); err != nil {
		t.Fatalf("close the database: %v", err)
	}
	// The whole point: no fallback wrote the keys to the database file.
	assertNoKeyMaterialOnDisk(t, dir, keyMaterial(t, device))
}

func TestDeviceStoreReportsKeyringFailureOnLoad(t *testing.T) {
	ctx := context.Background()
	kr := newFakeKeyring()
	s, _ := newTestDeviceStore(t, t.TempDir(), kr)

	device := syntheticDevice(t, nil)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}

	kr.getErr = errors.New("The name org.freedesktop.secrets was not provided")
	if _, err := s.LoadDevice(ctx); err == nil {
		t.Fatal("LoadDevice with an unreachable keyring: got nil error")
	} else if !strings.Contains(err.Error(), "keyring") {
		t.Errorf("error %q must name the keyring", err)
	}
}

func TestDeviceStoreDeleteRemovesRowAndKeyringItems(t *testing.T) {
	ctx := context.Background()
	kr := newFakeKeyring()
	s, _ := newTestDeviceStore(t, t.TempDir(), kr)

	device := syntheticDevice(t, nil)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}
	if err := device.Delete(ctx); err != nil {
		t.Fatalf("Delete: %v", err)
	}
	if got := kr.count(); got != 0 {
		t.Errorf("keyring still holds %d items after Delete, want 0", got)
	}
	if _, err := s.LoadDevice(ctx); !errors.Is(err, ErrNoDevice) {
		t.Errorf("LoadDevice after Delete = %v, want ErrNoDevice", err)
	}
}

func TestDeviceStoreLoadWithoutADevice(t *testing.T) {
	s, _ := newTestDeviceStore(t, t.TempDir(), newFakeKeyring())
	if _, err := s.LoadDevice(context.Background()); !errors.Is(err, ErrNoDevice) {
		t.Fatalf("LoadDevice on an empty store = %v, want ErrNoDevice", err)
	}
}

func TestNewDeviceStoreRequiresAKeyring(t *testing.T) {
	db, _ := openDBIn(t, t.TempDir())
	if _, err := NewDeviceStore(db, nil); err == nil {
		t.Fatal("NewDeviceStore without a keyring: got nil error")
	}
}

// TestDeviceStoreAgainstRealSecretService exercises the real keyring. It is
// off by default because it writes into the owner's keyring, and it always
// cleans its own uniquely named items up.
func TestDeviceStoreAgainstRealSecretService(t *testing.T) {
	if os.Getenv("NUNCIO_KEYRING_TESTS") != "1" {
		t.Skip("set NUNCIO_KEYRING_TESTS=1 to exercise the owner's real Secret Service")
	}
	ctx := context.Background()
	service := fmt.Sprintf("nuncio-test-%d", os.Getpid())
	t.Cleanup(func() {
		if err := keyring.DeleteAll(service); err != nil {
			t.Errorf("clean up keyring service %s: %v", service, err)
		}
		// The spike may not leave anything behind in the owner's keyring.
		for _, name := range keyItems {
			if _, err := keyring.Get(service, name); !errors.Is(err, keyring.ErrNotFound) {
				t.Errorf("keyring item %s of service %s survived the cleanup: %v", name, service, err)
			}
		}
	})

	db, _ := openDBIn(t, t.TempDir())
	s, err := NewDeviceStoreWithService(db, SecretService{}, service)
	if err != nil {
		t.Fatalf("NewDeviceStoreWithService: %v", err)
	}

	device := syntheticDevice(t, nil)
	device.Container = s
	if err := device.Save(ctx); err != nil {
		t.Fatalf("Save into the real Secret Service: %v", err)
	}
	loaded, err := s.LoadDevice(ctx)
	if err != nil {
		t.Fatalf("LoadDevice from the real Secret Service: %v", err)
	}
	assertSameDevice(t, loaded, device)
}
