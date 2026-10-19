package store

import (
	"io/fs"
	"os"
	"path/filepath"
	"testing"
)

func assertMode(t *testing.T, path string, want fs.FileMode) {
	t.Helper()
	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat %s: %v", path, err)
	}
	if got := info.Mode().Perm(); got != want {
		t.Errorf("%s: mode %#o, want %#o", filepath.Base(path), got, want)
	}
}

// openAndWrite opens the store at path and makes one write, which is what creates the WAL and
// SHM files.
func openAndWrite(t *testing.T, path string) {
	t.Helper()
	db, err := Open(path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	mustExec(t, db, `CREATE TABLE IF NOT EXISTS probe (v TEXT)`)
	mustExec(t, db, `INSERT INTO probe (v) VALUES ('x')`)
}

func TestOpenCreatesPrivateFiles(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "nuncio")
	path := filepath.Join(dir, "nuncio.db")

	openAndWrite(t, path)

	assertMode(t, dir, 0o700)
	for _, name := range []string{"nuncio.db", "nuncio.db-wal", "nuncio.db-shm"} {
		assertMode(t, filepath.Join(dir, name), 0o600)
	}
}

func TestOpenTightensExistingFiles(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "nuncio")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "nuncio.db")
	for _, name := range []string{"nuncio.db", "nuncio.db-wal", "nuncio.db-shm"} {
		if err := os.WriteFile(filepath.Join(dir, name), nil, 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chmod(filepath.Join(dir, name), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	openAndWrite(t, path)

	assertMode(t, dir, 0o700)
	for _, name := range []string{"nuncio.db", "nuncio.db-wal", "nuncio.db-shm"} {
		assertMode(t, filepath.Join(dir, name), 0o600)
	}
}

func TestOpenKeepsPragmas(t *testing.T) {
	db, err := Open(filepath.Join(t.TempDir(), "nuncio.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })

	var mode string
	if err := db.QueryRow(`PRAGMA journal_mode`).Scan(&mode); err != nil || mode != "wal" {
		t.Errorf("journal_mode = %q (%v), want wal", mode, err)
	}
	var fk int
	if err := db.QueryRow(`PRAGMA foreign_keys`).Scan(&fk); err != nil || fk != 1 {
		t.Errorf("foreign_keys = %d (%v), want 1", fk, err)
	}
	if got := db.Stats().MaxOpenConnections; got != 1 {
		t.Errorf("MaxOpenConnections = %d, want 1", got)
	}
}
