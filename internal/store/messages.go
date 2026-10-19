package store

import (
	"database/sql"
	"fmt"
	"strings"
	"time"
)

// Message kinds.
const (
	KindText     = "text"
	KindImage    = "image"
	KindAudio    = "audio"
	KindVideo    = "video"
	KindDocument = "document"
)

// Chat is one conversation. LastMessageAt and Unread are derived from its messages.
type Chat struct {
	JID           string
	Name          string
	Archived      bool
	Pinned        bool
	LastMessageAt time.Time
	Unread        int
}

// Message is one stored message. Its identity is (ChatJID, ID). Mime, Size and FileName
// describe media and stay empty for text.
type Message struct {
	ID        string
	ChatJID   string
	Kind      string
	Text      string
	FromMe    bool
	Timestamp time.Time
	Read      bool
	Mime      string
	Size      int64
	FileName  string
}

// SearchHit is one message that matched a search.
type SearchHit struct {
	ChatJID   string
	MessageID string
	Text      string
	Timestamp time.Time
}

// UpsertChat inserts a chat or updates its name, archived and pinned flags.
func UpsertChat(db *sql.DB, c Chat) error {
	_, err := db.Exec(`INSERT INTO chats (jid, name, archived, pinned) VALUES (?, ?, ?, ?)
		ON CONFLICT (jid) DO UPDATE SET name = excluded.name, archived = excluded.archived, pinned = excluded.pinned`,
		c.JID, c.Name, c.Archived, c.Pinned)
	if err != nil {
		return fmt.Errorf("cannot save chat: %w", err)
	}
	return nil
}

// InsertMessage stores m, creating its chat if needed, and indexes its text for search. A message
// whose (ChatJID, ID) is already stored is left untouched and InsertMessage returns false.
func InsertMessage(db *sql.DB, m Message) (inserted bool, err error) {
	tx, err := db.Begin()
	if err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	defer func() {
		if err != nil {
			_ = tx.Rollback()
		}
	}()
	if _, err = tx.Exec(`INSERT INTO chats (jid) VALUES (?) ON CONFLICT (jid) DO NOTHING`, m.ChatJID); err != nil {
		return false, fmt.Errorf("cannot save message chat: %w", err)
	}
	res, err := tx.Exec(`INSERT INTO messages (chat_jid, id, kind, text, from_me, ts, read, mime, size, file_name)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (chat_jid, id) DO NOTHING`,
		m.ChatJID, m.ID, m.Kind, m.Text, m.FromMe, m.Timestamp.UnixNano(), m.Read,
		nullString(m.Mime), nullInt(m.Size), nullString(m.FileName))
	if err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	if n == 0 {
		if err = tx.Commit(); err != nil {
			return false, fmt.Errorf("cannot save message: %w", err)
		}
		return false, nil
	}
	seq, err := res.LastInsertId()
	if err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	if _, err = tx.Exec(`INSERT INTO messages_fts (rowid, text) VALUES (?, ?)`, seq, m.Text); err != nil {
		return false, fmt.Errorf("cannot index message: %w", err)
	}
	if err = tx.Commit(); err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	return true, nil
}

// ListChats returns up to limit chats, the one with the newest message first.
func ListChats(db *sql.DB, limit int) ([]Chat, error) {
	rows, err := db.Query(`SELECT c.jid, c.name, c.archived, c.pinned, COALESCE(MAX(m.ts), 0),
			COUNT(CASE WHEN m.from_me = 0 AND m.read = 0 THEN 1 END)
		FROM chats c LEFT JOIN messages m ON m.chat_jid = c.jid
		GROUP BY c.jid ORDER BY COALESCE(MAX(m.ts), 0) DESC, c.jid LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var chats []Chat
	for rows.Next() {
		var c Chat
		var last int64
		if err := rows.Scan(&c.JID, &c.Name, &c.Archived, &c.Pinned, &last, &c.Unread); err != nil {
			return nil, fmt.Errorf("cannot read chat: %w", err)
		}
		if last != 0 {
			c.LastMessageAt = fromNanos(last)
		}
		chats = append(chats, c)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	return chats, nil
}

// ListMessages returns up to limit messages of a chat, newest first.
func ListMessages(db *sql.DB, chatJID string, limit int) ([]Message, error) {
	rows, err := db.Query(`SELECT id, chat_jid, kind, text, from_me, ts, read,
			COALESCE(mime, ''), COALESCE(size, 0), COALESCE(file_name, '')
		FROM messages WHERE chat_jid = ? ORDER BY ts DESC, seq DESC LIMIT ?`, chatJID, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot list messages: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var msgs []Message
	for rows.Next() {
		var m Message
		var ts int64
		if err := rows.Scan(&m.ID, &m.ChatJID, &m.Kind, &m.Text, &m.FromMe, &ts, &m.Read,
			&m.Mime, &m.Size, &m.FileName); err != nil {
			return nil, fmt.Errorf("cannot read message: %w", err)
		}
		m.Timestamp = fromNanos(ts)
		msgs = append(msgs, m)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list messages: %w", err)
	}
	return msgs, nil
}

// Search returns up to limit messages whose text contains every word of query, best match
// (bm25) first. The words are matched literally: FTS5 operators in query have no effect.
func Search(db *sql.DB, query string, limit int) ([]SearchHit, error) {
	match := quoteTerms(query)
	if match == "" {
		return nil, nil
	}
	rows, err := db.Query(`SELECT m.chat_jid, m.id, m.text, m.ts
		FROM messages_fts f JOIN messages m ON m.seq = f.rowid
		WHERE messages_fts MATCH ? ORDER BY bm25(messages_fts), m.ts DESC LIMIT ?`, match, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot search messages: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var hits []SearchHit
	for rows.Next() {
		var h SearchHit
		var ts int64
		if err := rows.Scan(&h.ChatJID, &h.MessageID, &h.Text, &ts); err != nil {
			return nil, fmt.Errorf("cannot read search hit: %w", err)
		}
		h.Timestamp = fromNanos(ts)
		hits = append(hits, h)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot search messages: %w", err)
	}
	return hits, nil
}

// UnreadCount returns how many incoming messages of a chat are unread.
func UnreadCount(db *sql.DB, chatJID string) (int, error) {
	var n int
	err := db.QueryRow(`SELECT COUNT(*) FROM messages WHERE chat_jid = ? AND from_me = 0 AND read = 0`, chatJID).Scan(&n)
	if err != nil {
		return 0, fmt.Errorf("cannot count unread messages: %w", err)
	}
	return n, nil
}

// TotalUnread returns how many incoming messages are unread across all chats.
func TotalUnread(db *sql.DB) (int, error) {
	var n int
	if err := db.QueryRow(`SELECT COUNT(*) FROM messages WHERE from_me = 0 AND read = 0`).Scan(&n); err != nil {
		return 0, fmt.Errorf("cannot count unread messages: %w", err)
	}
	return n, nil
}

// MarkChatRead marks every message of a chat as read.
func MarkChatRead(db *sql.DB, chatJID string) error {
	if _, err := db.Exec(`UPDATE messages SET read = 1 WHERE chat_jid = ? AND read = 0`, chatJID); err != nil {
		return fmt.Errorf("cannot mark chat read: %w", err)
	}
	return nil
}

// quoteTerms turns free text into an FTS5 query that ANDs every word as a literal string, so
// quotes, operators and column filters typed by the user cannot break the query.
func quoteTerms(query string) string {
	words := strings.Fields(query)
	for i, w := range words {
		words[i] = `"` + strings.ReplaceAll(w, `"`, `""`) + `"`
	}
	return strings.Join(words, " ")
}

func fromNanos(n int64) time.Time { return time.Unix(0, n).UTC() }

func nullString(s string) sql.NullString { return sql.NullString{String: s, Valid: s != ""} }

func nullInt(n int64) sql.NullInt64 { return sql.NullInt64{Int64: n, Valid: n != 0} }
