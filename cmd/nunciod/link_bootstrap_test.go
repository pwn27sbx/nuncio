package main

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// tmpfilesDataDir is the data directory systemd-tmpfiles creates before the unit starts: empty,
// 0700, and never created by the daemon itself.
func tmpfilesDataDir(t *testing.T) (dir, dbPath string) {
	t.Helper()
	dir = filepath.Join(t.TempDir(), "nuncio")
	if err := os.Mkdir(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	return dir, filepath.Join(dir, store.DBFile)
}

// The daemon's link bootstrap over the empty directory tmpfiles made: one store, one data key and
// an unpaired device, all private, and a second flow reuses the key instead of writing another.
func TestLinkBootstrapCreatesTheStoreAndOneDataKey(t *testing.T) {
	dir, path := tmpfilesDataDir(t)
	kr := newLockableKeyring()

	b, err := openLinkBootstrap(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openLinkBootstrap: %v", err)
	}
	if b.device == nil || b.device.ID != nil {
		t.Fatalf("bootstrap device %+v, want an unpaired device", b.device)
	}
	if !b.createdKey {
		t.Error("the bootstrap created the data key but did not report it")
	}
	if !kr.hasDataKey() || kr.count() != 1 {
		t.Errorf("keyring holds %d items, want the one data key", kr.count())
	}
	if info, err := os.Stat(path); err != nil {
		t.Fatalf("no store: %v", err)
	} else if perm := info.Mode().Perm(); perm != 0o600 {
		t.Errorf("store mode %04o, want 0600", perm)
	}
	if info, err := os.Stat(dir); err != nil {
		t.Fatal(err)
	} else if perm := info.Mode().Perm(); perm != 0o700 {
		t.Errorf("data dir mode %04o, want 0700", perm)
	}
	if stored, err := wa.HasStoredDevice(context.Background(), b.db); err != nil {
		t.Fatal(err)
	} else if stored {
		t.Error("the bootstrap stored a device before pairing")
	}
	_ = b.db.Close()

	b, err = openLinkBootstrap(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("second openLinkBootstrap: %v", err)
	}
	defer func() { _ = b.db.Close() }()
	if b.createdKey {
		t.Error("the second bootstrap created another data key")
	}
	if kr.count() != 1 {
		t.Errorf("keyring holds %d items after two bootstraps, want 1", kr.count())
	}
}

// A store that already holds a linked device is refused before the keyring is even read: the
// device, its key and its keyring entry stay exactly as they were.
func TestLinkBootstrapRefusesALinkedStoreAndLeavesItAlone(t *testing.T) {
	path, kr := sealedStore(t)
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	calls, items := kr.calls.Load(), kr.count()

	b, err := openLinkBootstrap(context.Background(), path, kr)
	if !errors.Is(err, errLinkRefused) {
		t.Fatalf("openLinkBootstrap over a linked store: %v, want errLinkRefused", err)
	}
	if b != nil {
		t.Error("the refused bootstrap returned a store")
	}
	if calls != kr.calls.Load() {
		t.Errorf("the refused bootstrap asked the keyring (%d calls, had %d)", kr.calls.Load(), calls)
	}
	if kr.count() != items || !kr.hasDataKey() {
		t.Error("the refused bootstrap changed the keyring")
	}
	if after, err := os.ReadFile(path); err != nil {
		t.Fatal(err)
	} else if !bytes.Equal(before, after) {
		t.Error("the refused bootstrap changed nuncio.db")
	}

	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	device, _, err := wa.OpenDevice(context.Background(), db, path, kr, false)
	if err != nil {
		t.Fatalf("the linked device no longer opens: %v", err)
	}
	if device.ID == nil || device.ID.User != sealedJID.User {
		t.Errorf("linked device is %v, want %s", device.ID, sealedJID.User)
	}
}

// With the keyring stopped, the bootstrap refuses with the keyring error, exactly as `nuncio link`
// does today: no data key, no device, nothing linked.
func TestLinkBootstrapRefusesWithTheKeyringStopped(t *testing.T) {
	_, path := tmpfilesDataDir(t)
	kr := newLockableKeyring()
	kr.locked.Store(true)

	b, err := openLinkBootstrap(context.Background(), path, kr)
	if !errors.Is(err, store.ErrKeyringUnavailable) {
		t.Fatalf("openLinkBootstrap with a stopped keyring: %v, want store.ErrKeyringUnavailable", err)
	}
	if errors.Is(err, errLinkRefused) {
		t.Error("a stopped keyring was reported as an already linked device")
	}
	if b != nil {
		t.Error("the refused bootstrap returned a store")
	}
	if kr.hasDataKey() || kr.count() != 0 {
		t.Error("the refused bootstrap left something in the keyring")
	}

	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = db.Close() }()
	if stored, err := wa.HasStoredDevice(context.Background(), db); err != nil {
		t.Fatal(err)
	} else if stored {
		t.Error("the refused bootstrap stored a device")
	}
}
