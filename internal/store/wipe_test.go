package store

import (
	"os"
	"path/filepath"
	"testing"
)

// populatedDataDir is a nuncio data dir as a running daemon leaves it: the database with its WAL
// and SHM files, a media cache and an exports dir, each holding a file.
func populatedDataDir(t *testing.T) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "nuncio")
	db, err := Open(filepath.Join(dir, DBFile))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	// SQLite removes the WAL and SHM files on a clean close, so they are recreated as a crashed
	// daemon would leave them.
	for _, name := range []string{DBFile + "-wal", DBFile + "-shm"} {
		writeFile(t, filepath.Join(dir, name))
	}
	writeFile(t, filepath.Join(dir, MediaDir, "0f3a.jpg"))
	writeFile(t, filepath.Join(dir, ExportsDir, "chat.txt"))
	return dir
}

func writeFile(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("synthetic"), 0o600); err != nil {
		t.Fatal(err)
	}
}

// fullKeyring holds every device item under the nuncio service.
func fullKeyring(t *testing.T) *fakeKeyring {
	t.Helper()
	kr := newFakeKeyring()
	for _, name := range keyItems {
		if err := kr.Set(KeyringService, name, "synthetic"); err != nil {
			t.Fatal(err)
		}
	}
	return kr
}

func assertGone(t *testing.T, path string) {
	t.Helper()
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Errorf("%s still exists after Wipe (lstat: %v)", path, err)
	}
}

func TestWipeRemovesTheDatabaseItsWALAndSHM(t *testing.T) {
	dir := populatedDataDir(t)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	for _, name := range []string{DBFile, DBFile + "-wal", DBFile + "-shm"} {
		assertGone(t, filepath.Join(dir, name))
	}
}

func TestWipeRemovesTheMediaCacheAndExports(t *testing.T) {
	dir := populatedDataDir(t)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	assertGone(t, filepath.Join(dir, MediaDir))
	assertGone(t, filepath.Join(dir, ExportsDir))
	// Nothing else was there, so the data dir itself goes too.
	assertGone(t, dir)
}

func TestWipeEmptiesTheKeyring(t *testing.T) {
	kr := fullKeyring(t)
	if err := Wipe(populatedDataDir(t), kr); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	if n := kr.count(); n != 0 {
		t.Errorf("keyring holds %d items after Wipe, want 0", n)
	}
}

func TestWipeTwiceIsNotAnError(t *testing.T) {
	dir := populatedDataDir(t)
	kr := fullKeyring(t)
	if err := Wipe(dir, kr); err != nil {
		t.Fatalf("first Wipe: %v", err)
	}
	if err := Wipe(dir, kr); err != nil {
		t.Errorf("second Wipe = %v, want nil", err)
	}
}

// A symlink inside the data dir is removed as a link; what it points at, outside the data dir,
// is never touched.
func TestWipeNeverDeletesOutsideTheDataDir(t *testing.T) {
	dir := populatedDataDir(t)
	outside := t.TempDir()
	keep := filepath.Join(outside, "keep.txt")
	writeFile(t, keep)
	writeFile(t, filepath.Join(outside, "nuncio.db"))
	if err := os.RemoveAll(filepath.Join(dir, MediaDir)); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dir, MediaDir)); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(filepath.Join(dir, DBFile+"-wal")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(outside, "nuncio.db"), filepath.Join(dir, DBFile+"-wal")); err != nil {
		t.Fatal(err)
	}

	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	for _, p := range []string{keep, filepath.Join(outside, "nuncio.db")} {
		if _, err := os.Stat(p); err != nil {
			t.Errorf("%s outside the data dir was touched: %v", p, err)
		}
	}
	assertGone(t, filepath.Join(dir, MediaDir))
	assertGone(t, filepath.Join(dir, DBFile+"-wal"))
}

// A data dir that is itself a symlink is refused: wiping through it would delete files somewhere
// else.
func TestWipeRefusesADataDirThatIsASymlink(t *testing.T) {
	real := populatedDataDir(t)
	link := filepath.Join(t.TempDir(), "nuncio")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	kr := fullKeyring(t)
	if err := Wipe(link, kr); err == nil {
		t.Fatal("Wipe through a symlinked data dir = nil, want an error")
	}
	if _, err := os.Stat(filepath.Join(real, DBFile)); err != nil {
		t.Errorf("the database behind the link was touched: %v", err)
	}
	if n := kr.count(); n != len(keyItems) {
		t.Errorf("keyring holds %d items after a refused Wipe, want %d", n, len(keyItems))
	}
}

// Files nuncio does not own are left alone, and so is the data dir that holds them.
func TestWipeKeepsFilesItDoesNotOwn(t *testing.T) {
	dir := populatedDataDir(t)
	other := filepath.Join(dir, "notes.txt")
	writeFile(t, other)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	if _, err := os.Stat(other); err != nil {
		t.Errorf("%s was deleted: %v", other, err)
	}
	assertGone(t, filepath.Join(dir, DBFile))
}
