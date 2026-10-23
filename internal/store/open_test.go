package store

import (
	"context"
	"database/sql/driver"
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

// A connection database/sql opens later (after a cancelled query made it drop the first one, as
// L8 observed) gets the same pragmas: they are in the DSN, not run once on the first connection.
func TestOpenGivesEveryNewConnectionItsPragmas(t *testing.T) {
	db, err := Open(filepath.Join(t.TempDir(), "nuncio.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = db.Close() })
	ctx := context.Background()
	conn, err := db.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	// ErrBadConn makes database/sql discard this connection instead of returning it to the pool.
	_ = conn.Raw(func(any) error { return driver.ErrBadConn })
	_ = conn.Close()

	var fk int
	if err := db.QueryRow(`PRAGMA foreign_keys`).Scan(&fk); err != nil || fk != 1 {
		t.Errorf("foreign_keys on a new connection = %d (%v), want 1", fk, err)
	}
	if got := db.Stats().OpenConnections; got != 1 {
		t.Errorf("OpenConnections = %d, want 1 (a new one)", got)
	}
}

// A path with characters that mean something in a URI still opens that file and no other.
func TestOpenKeepsAPathWithURICharacters(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "a?b#c%d")
	path := filepath.Join(dir, "nuncio.db")
	openAndWrite(t, path)
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if len(names) == 0 || names[0] != "nuncio.db" {
		t.Errorf("files in the data dir = %v, want nuncio.db there", names)
	}
}

// A relative path opens the file relative to the working directory, as before.
func TestOpenKeepsARelativePath(t *testing.T) {
	t.Chdir(t.TempDir())
	openAndWrite(t, filepath.Join("data", "nuncio.db"))
	if _, err := os.Stat(filepath.Join("data", "nuncio.db")); err != nil {
		t.Errorf("stat data/nuncio.db: %v", err)
	}
}
