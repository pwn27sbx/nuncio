package store

import (
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"net/url"
	"os"
	"path/filepath"

	// The pure-Go SQLite driver: no cgo, no system libsqlite3.
	_ "modernc.org/sqlite"
)

// driverName is the pure-Go SQLite driver this project uses.
const driverName = "sqlite"

// openPragmas are the pragmas every connection gets, through the DSN: database/sql may drop a
// connection (after a cancelled query) and open another, and a pragma run once with Exec would be
// missing on the new one.
const openPragmas = "_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)"

// Open opens the SQLite database at path with the pragmas nuncio relies on (WAL, foreign keys) on
// every connection, and one connection at a time. The database and its WAL/SHM files are private:
// the directory is 0700 and every file is 0600, including files an older build left at 0644.
//
// SQLite creates the WAL and SHM files with the mode of the database file, so creating the
// database 0600 before the driver touches it covers files that do not exist yet.
func Open(path string) (*sql.DB, error) {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("cannot create %s: %w", dir, err)
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		return nil, fmt.Errorf("cannot make %s private: %w", dir, err)
	}
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return nil, fmt.Errorf("cannot create %s: %w", path, err)
	}
	_ = f.Close()
	for _, p := range []string{path, path + "-wal", path + "-shm"} {
		if err := os.Chmod(p, 0o600); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return nil, fmt.Errorf("cannot make %s private: %w", p, err)
		}
	}

	// A file: URI of the absolute path, so a '?', '#' or '%' in it is escaped and stays part of
	// the file name (a relative path would be read as a host).
	abs, err := filepath.Abs(path)
	if err != nil {
		return nil, fmt.Errorf("cannot open %s: %w", path, err)
	}
	dsn := url.URL{Scheme: "file", Path: abs, RawQuery: openPragmas}
	db, err := sql.Open(driverName, dsn.String())
	if err != nil {
		return nil, fmt.Errorf("cannot open %s: %w", path, err)
	}
	db.SetMaxOpenConns(1)
	// The first connection, which applies the pragmas, so a database that cannot take them fails
	// here and not on the first query.
	if err := db.Ping(); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("cannot prepare %s: %w", path, err)
	}
	return db, nil
}
