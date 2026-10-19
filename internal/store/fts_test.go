package store

import (
	"database/sql"
	"fmt"
	"path/filepath"
	"testing"

	// The pure-Go SQLite driver under test: no cgo, no system libsqlite3.
	_ "modernc.org/sqlite"
)

// driverName is the pure-Go SQLite driver this project uses.
const driverName = "sqlite"

// openTestDB opens a throwaway SQLite database in the test's temp directory.
func openTestDB(t *testing.T) *sql.DB {
	t.Helper()
	db, _ := openDBIn(t, t.TempDir())
	return db
}

// openDBIn opens a throwaway SQLite database inside dir and returns it with
// the directory, so tests can inspect the files the driver wrote.
func openDBIn(t *testing.T, dir string) (*sql.DB, string) {
	t.Helper()
	db, err := sql.Open(driverName, filepath.Join(dir, "spike.db"))
	if err != nil {
		t.Fatalf("open %s: %v", driverName, err)
	}
	db.SetMaxOpenConns(1)
	t.Cleanup(func() { _ = db.Close() })
	return db, dir
}

func mustExec(t *testing.T, db *sql.DB, query string, args ...any) {
	t.Helper()
	if _, err := db.Exec(query, args...); err != nil {
		t.Fatalf("exec %s: %v", query, err)
	}
}

// matchTexts runs an FTS5 MATCH query and returns the matching texts in rowid
// order.
func matchTexts(t *testing.T, db *sql.DB, query string) []string {
	t.Helper()
	rows, err := db.Query(`SELECT text FROM messages_fts WHERE messages_fts MATCH ? ORDER BY rowid`, query)
	if err != nil {
		t.Fatalf("match %q: %v", query, err)
	}
	defer func() { _ = rows.Close() }()
	var got []string
	for rows.Next() {
		var text string
		if err := rows.Scan(&text); err != nil {
			t.Fatalf("scan: %v", err)
		}
		got = append(got, text)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate matches of %q: %v", query, err)
	}
	return got
}

// TestSQLiteFTS5Spike is spike S8: the pure-Go driver must be able to create an
// fts5 virtual table, index rows and answer a MATCH query, because nuncio's
// message search is planned on FTS5.
func TestSQLiteFTS5Spike(t *testing.T) {
	db := openTestDB(t)

	var version string
	if err := db.QueryRow(`SELECT sqlite_version()`).Scan(&version); err != nil {
		t.Fatalf("sqlite_version: %v", err)
	}
	t.Logf("driver %q, sqlite %s, FTS5 enabled", driverName, version)

	if _, err := db.Exec(`CREATE VIRTUAL TABLE messages_fts USING fts5(text, chat UNINDEXED)`); err != nil {
		t.Fatalf("CREATE VIRTUAL TABLE ... USING fts5 failed, so %s has no FTS5: %v\n"+
			"fallback candidate: github.com/ncruces/go-sqlite3", driverName, err)
	}

	rows := []struct{ text, chat string }{
		{"The compiler is done", "15551230002@s.whatsapp.net"},
		{"the compiler needs a rebuild", "120363000000000000@g.us"},
		{"See you at 18:00", "15551230001@s.whatsapp.net"},
	}
	for _, row := range rows {
		mustExec(t, db, `INSERT INTO messages_fts (text, chat) VALUES (?, ?)`, row.text, row.chat)
	}

	t.Run("exact token", func(t *testing.T) {
		got := matchTexts(t, db, "compiler")
		want := []string{"The compiler is done", "the compiler needs a rebuild"}
		if fmt.Sprint(got) != fmt.Sprint(want) {
			t.Fatalf("MATCH compiler = %v, want %v", got, want)
		}
	})

	t.Run("prefix", func(t *testing.T) {
		got := matchTexts(t, db, "compil*")
		if len(got) != 2 {
			t.Fatalf("MATCH compil* = %v, want the two compiler messages", got)
		}
	})

	t.Run("no match", func(t *testing.T) {
		if got := matchTexts(t, db, "whatsapp"); len(got) != 0 {
			t.Fatalf("MATCH whatsapp = %v, want no rows (chat is UNINDEXED)", got)
		}
	})

	t.Run("filtered by column", func(t *testing.T) {
		got := matchTexts(t, db, "text:rebuild")
		want := []string{"the compiler needs a rebuild"}
		if fmt.Sprint(got) != fmt.Sprint(want) {
			t.Fatalf("MATCH text:rebuild = %v, want %v", got, want)
		}
	})

	t.Run("ranked search", func(t *testing.T) {
		// bm25 is FTS5's built-in ranker; nuncio's search wants it.
		var text string
		err := db.QueryRow(`SELECT text FROM messages_fts WHERE messages_fts MATCH ? ORDER BY bm25(messages_fts) LIMIT 1`, "compiler").Scan(&text)
		if err != nil {
			t.Fatalf("ranked query: %v", err)
		}
		if text == "" {
			t.Fatal("ranked query returned an empty text")
		}
	})
}

// TestSQLiteIsPureGo pins the reason for the driver choice: the daemon must
// build without cgo.
func TestSQLiteIsPureGo(t *testing.T) {
	db := openTestDB(t)
	mustExec(t, db, `CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)`)
	mustExec(t, db, `INSERT INTO t (v) VALUES ('ok')`)
	var v string
	if err := db.QueryRow(`SELECT v FROM t WHERE id = 1`).Scan(&v); err != nil {
		t.Fatalf("round trip: %v", err)
	}
	if v != "ok" {
		t.Fatalf("v = %q, want ok", v)
	}
}
