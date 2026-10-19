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
// the schema only ever adds, never drops or rewrites. chat_mutes follows the same rule: one row
// per muted chat with its end time (Unix nanoseconds, -1 for muted with no end), and no row for a
// chat that is not muted.
//
// outbox follows it too: one row per send a client asked for, keyed by the client's client_id, with
// the WhatsApp message id chosen when it was queued (every attempt reuses it), its creation time,
// its state (queued, sent, not_sent, or not_confirmed for an ack that never came) and the ack time of a sent one. Rows are never deleted, so a
// send that failed stays visible until a manual retry.
//
// M2 adds three side tables under the same rule. message_meta holds, per message, who sent it (the
// sender's JID without device, empty for mine) and the id of the message it quotes, if any;
// reactions holds the current reaction of each sender on a message (sender empty for mine, no row
// once removed); outbox_replies holds the quoted id of a queued send, keyed by its client_id.
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
CREATE TABLE IF NOT EXISTS chat_mutes (
	jid   TEXT PRIMARY KEY,
	until INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS outbox (
	seq        INTEGER PRIMARY KEY,
	client_id  TEXT NOT NULL UNIQUE,
	chat_jid   TEXT NOT NULL,
	text       TEXT NOT NULL,
	message_id TEXT NOT NULL,
	created    INTEGER NOT NULL,
	state      TEXT NOT NULL,
	sent_at    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS message_meta (
	chat_jid TEXT NOT NULL,
	id       TEXT NOT NULL,
	sender   TEXT NOT NULL DEFAULT '',
	reply_to TEXT NOT NULL DEFAULT '',
	PRIMARY KEY (chat_jid, id)
);
CREATE TABLE IF NOT EXISTS reactions (
	chat_jid TEXT NOT NULL,
	id       TEXT NOT NULL,
	sender   TEXT NOT NULL,
	from_me  INTEGER NOT NULL,
	emoji    TEXT NOT NULL,
	PRIMARY KEY (chat_jid, id, sender)
);
CREATE TABLE IF NOT EXISTS outbox_replies (
	client_id TEXT PRIMARY KEY,
	reply_to  TEXT NOT NULL
);
`

// EnsureSchema creates the chat and message tables if they do not exist yet.
func EnsureSchema(db *sql.DB) error {
	if _, err := db.Exec(schema); err != nil {
		return fmt.Errorf("cannot create the message schema: %w", err)
	}
	return nil
}
