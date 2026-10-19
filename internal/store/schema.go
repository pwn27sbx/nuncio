package store

import (
	"database/sql"
	"fmt"
)

// schema creates nuncio's own tables next to whatsmeow's (which are prefixed
// whatsmeow_). Every statement is idempotent, so it runs on every open.
//
// Timestamps are Unix nanoseconds in UTC, so ordering is numeric. messages_fts
// shares rowids with messages and is written in the same transaction.
//
// media_refs holds, per media message, the serialised whatsmeow media sub-message (key, direct
// path, hashes) a download on request needs. It is a side table, not a new messages column, so a
// database created before it existed gains it with CREATE TABLE IF NOT EXISTS and keeps its rows:
// the schema only ever adds, never drops or rewrites.
const schema = `
CREATE TABLE IF NOT EXISTS chats (
	jid      TEXT PRIMARY KEY,
	name     TEXT NOT NULL DEFAULT '',
	archived INTEGER NOT NULL DEFAULT 0,
	pinned   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
	seq       INTEGER PRIMARY KEY,
	chat_jid  TEXT NOT NULL,
	id        TEXT NOT NULL,
	kind      TEXT NOT NULL,
	text      TEXT NOT NULL DEFAULT '',
	from_me   INTEGER NOT NULL,
	ts        INTEGER NOT NULL,
	read      INTEGER NOT NULL DEFAULT 0,
	mime      TEXT,
	size      INTEGER,
	file_name TEXT,
	UNIQUE (chat_jid, id)
);
CREATE INDEX IF NOT EXISTS messages_chat_ts ON messages (chat_jid, ts);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text);
CREATE TABLE IF NOT EXISTS media_refs (
	chat_jid TEXT NOT NULL,
	id       TEXT NOT NULL,
	ref      BLOB NOT NULL,
	PRIMARY KEY (chat_jid, id)
);
`

// EnsureSchema creates the chat and message tables if they do not exist yet.
func EnsureSchema(db *sql.DB) error {
	if _, err := db.Exec(schema); err != nil {
		return fmt.Errorf("cannot create the message schema: %w", err)
	}
	return nil
}
