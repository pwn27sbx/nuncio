package store

import (
	"database/sql"
	"errors"
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

// Chat is one conversation. LastMessageAt and Unread are derived from its messages. MutedUntil is
// zero for a chat that is not muted and MutedForever for one muted with no end; UpsertChat leaves
// it alone, only SetChatMutedUntil changes it.
type Chat struct {
	JID           string
	Name          string
	Archived      bool
	Pinned        bool
	LastMessageAt time.Time
	Unread        int
	MutedUntil    time.Time
}

// MutedForever is the MutedUntil of a chat muted with no end (whatsmeow's store.MutedForever).
var MutedForever = time.Date(9999, 12, 31, 23, 59, 59, 999999999, time.UTC)

// mutedForeverNanos stands for MutedForever in chat_mutes: year 9999 does not fit Unix nanoseconds.
const mutedForeverNanos = -1

// Muted reports whether the chat is muted at now.
func (c Chat) Muted(now time.Time) bool { return c.MutedUntil.After(now) }

// ErrNoMessage: no message with that id is stored in that chat.
var ErrNoMessage = errors.New("no such message in that chat")

// Message is one stored message. Its identity is (ChatJID, ID). Mime, Size and FileName
// describe media and stay empty for text. MediaRef is what a later download needs (the
// serialised media sub-message, with its key, path and hashes); only GetMessage reads it back.
// State and ClientID are set only on a send of mine that is not sent yet, as ListMessages lists
// it from the outbox: State is OutboxQueued, OutboxNotSent or OutboxNotConfirmed.
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
	MediaRef  []byte
	State     string
	ClientID  string
	Sender    string
	ReplyTo   string
	Reactions []Reaction
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

// SetChatPinned sets the pinned flag of a chat, creating the chat if it is not stored yet.
func SetChatPinned(db *sql.DB, jid string, pinned bool) error {
	_, err := db.Exec(`INSERT INTO chats (jid, pinned) VALUES (?, ?)
		ON CONFLICT (jid) DO UPDATE SET pinned = excluded.pinned`, jid, pinned)
	if err != nil {
		return fmt.Errorf("cannot save chat pin: %w", err)
	}
	return nil
}

// SetChatArchived sets the archived flag of a chat, creating the chat if it is not stored yet.
func SetChatArchived(db *sql.DB, jid string, archived bool) error {
	_, err := db.Exec(`INSERT INTO chats (jid, archived) VALUES (?, ?)
		ON CONFLICT (jid) DO UPDATE SET archived = excluded.archived`, jid, archived)
	if err != nil {
		return fmt.Errorf("cannot save chat archive: %w", err)
	}
	return nil
}

// SetChatMutedUntil mutes a chat until the given time (MutedForever for no end) or, with the zero
// time, unmutes it. The chat is created if it is not stored yet.
func SetChatMutedUntil(db *sql.DB, jid string, until time.Time) error {
	if _, err := db.Exec(`INSERT INTO chats (jid) VALUES (?) ON CONFLICT (jid) DO NOTHING`, jid); err != nil {
		return fmt.Errorf("cannot save chat mute: %w", err)
	}
	var err error
	switch {
	case until.IsZero():
		_, err = db.Exec(`DELETE FROM chat_mutes WHERE jid = ?`, jid)
	default:
		end := int64(mutedForeverNanos)
		if !until.Equal(MutedForever) {
			end = until.UnixNano()
		}
		_, err = db.Exec(`INSERT INTO chat_mutes (jid, until) VALUES (?, ?)
			ON CONFLICT (jid) DO UPDATE SET until = excluded.until`, jid, end)
	}
	if err != nil {
		return fmt.Errorf("cannot save chat mute: %w", err)
	}
	return nil
}

// InsertMessage stores m, creating its chat if needed, indexes its text for search and keeps its
// MediaRef, if any, for a later download (nothing is downloaded here). A message
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
	if m.Sender != "" || m.ReplyTo != "" {
		if _, err = tx.Exec(`INSERT INTO message_meta (chat_jid, id, sender, reply_to) VALUES (?, ?, ?, ?)
			ON CONFLICT (chat_jid, id) DO NOTHING`, m.ChatJID, m.ID, m.Sender, m.ReplyTo); err != nil {
			return false, fmt.Errorf("cannot save message details: %w", err)
		}
	}
	if len(m.MediaRef) > 0 {
		if _, err = tx.Exec(`INSERT INTO media_refs (chat_jid, id, ref) VALUES (?, ?, ?)`,
			m.ChatJID, m.ID, m.MediaRef); err != nil {
			return false, fmt.Errorf("cannot save media reference: %w", err)
		}
	}
	if err = tx.Commit(); err != nil {
		return false, fmt.Errorf("cannot save message: %w", err)
	}
	return true, nil
}

// GetMessage returns one stored message with its MediaRef (nil for text), sender, quoted id and
// reactions, or ErrNoMessage.
func GetMessage(db *sql.DB, chatJID, id string) (Message, error) {
	var m Message
	var ts int64
	err := db.QueryRow(`SELECT m.id, m.chat_jid, m.kind, m.text, m.from_me, m.ts, m.read,
			COALESCE(m.mime, ''), COALESCE(m.size, 0), COALESCE(m.file_name, ''), r.ref,
			COALESCE(mm.sender, ''), COALESCE(mm.reply_to, '')
		FROM messages m LEFT JOIN media_refs r ON r.chat_jid = m.chat_jid AND r.id = m.id
		LEFT JOIN message_meta mm ON mm.chat_jid = m.chat_jid AND mm.id = m.id
		WHERE m.chat_jid = ? AND m.id = ?`, chatJID, id).
		Scan(&m.ID, &m.ChatJID, &m.Kind, &m.Text, &m.FromMe, &ts, &m.Read, &m.Mime, &m.Size, &m.FileName, &m.MediaRef,
			&m.Sender, &m.ReplyTo)
	if errors.Is(err, sql.ErrNoRows) {
		return Message{}, ErrNoMessage
	}
	if err != nil {
		return Message{}, fmt.Errorf("cannot read message: %w", err)
	}
	m.Timestamp = fromNanos(ts)
	reactions, err := chatReactions(db, chatJID)
	if err != nil {
		return Message{}, err
	}
	m.Reactions = reactions[m.ID]
	return m, nil
}

// chatColumns is what ListChats and GetChat select for one chat (c is chats, m its messages, mu
// its mute), in the order scanChat reads it.
const chatColumns = `c.jid, c.name, c.archived, c.pinned, COALESCE(MAX(m.ts), 0),
			COUNT(CASE WHEN m.from_me = 0 AND m.read = 0 THEN 1 END), COALESCE(mu.until, 0)`

// scanChat reads one row of chatColumns. A missing row is sql.ErrNoRows, unwrapped.
func scanChat(row interface{ Scan(...any) error }) (Chat, error) {
	var c Chat
	var last, muted int64
	if err := row.Scan(&c.JID, &c.Name, &c.Archived, &c.Pinned, &last, &c.Unread, &muted); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return Chat{}, err
		}
		return Chat{}, fmt.Errorf("cannot read chat: %w", err)
	}
	if last != 0 {
		c.LastMessageAt = fromNanos(last)
	}
	switch muted {
	case 0:
	case mutedForeverNanos:
		c.MutedUntil = MutedForever
	default:
		c.MutedUntil = fromNanos(muted)
	}
	return c, nil
}

// ListChats returns up to limit chats, the one with the newest message first.
func ListChats(db *sql.DB, limit int) ([]Chat, error) {
	rows, err := db.Query(`SELECT `+chatColumns+`
		FROM chats c LEFT JOIN messages m ON m.chat_jid = c.jid LEFT JOIN chat_mutes mu ON mu.jid = c.jid
		GROUP BY c.jid ORDER BY COALESCE(MAX(m.ts), 0) DESC, c.jid LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var chats []Chat
	for rows.Next() {
		c, err := scanChat(rows)
		if err != nil {
			return nil, err
		}
		chats = append(chats, c)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	return chats, nil
}

// ErrNoChat: no chat with that JID is stored.
var ErrNoChat = errors.New("no such chat")

// GetChat returns the stored chat jid exactly as ListChats lists it, or ErrNoChat. It aggregates
// that chat's messages only, so it stays cheap when called once per app-state event.
func GetChat(db *sql.DB, jid string) (Chat, error) {
	row := db.QueryRow(`SELECT `+chatColumns+`
		FROM chats c LEFT JOIN messages m ON m.chat_jid = c.jid LEFT JOIN chat_mutes mu ON mu.jid = c.jid
		WHERE c.jid = ? GROUP BY c.jid`, jid)
	c, err := scanChat(row)
	if errors.Is(err, sql.ErrNoRows) {
		return Chat{}, ErrNoChat
	}
	if err != nil {
		return Chat{}, err
	}
	return c, nil
}

// ListMessages returns up to limit messages of a chat, newest first: the stored messages and the
// sends of mine to that chat that are queued or not sent, which carry their State and ClientID and
// the time they were queued. A send that was sent is listed once, as the stored message.
func ListMessages(db *sql.DB, chatJID string, limit int) ([]Message, error) {
	rows, err := db.Query(`SELECT m.id, m.chat_jid, m.kind, m.text, m.from_me, m.ts AS ts, m.read,
			COALESCE(m.mime, ''), COALESCE(m.size, 0), COALESCE(m.file_name, ''),
			'', '', 0 AS pending, m.seq AS seq, COALESCE(mm.reply_to, '')
		FROM messages m LEFT JOIN message_meta mm ON mm.chat_jid = m.chat_jid AND mm.id = m.id
		WHERE m.chat_jid = ?
		UNION ALL
		SELECT o.message_id, o.chat_jid, 'text', o.text, 1, o.created, 1, '', 0, '', o.state, o.client_id, 1, o.seq,
			COALESCE(r.reply_to, '')
		FROM outbox o LEFT JOIN outbox_replies r ON r.client_id = o.client_id
		WHERE o.chat_jid = ? AND o.state != ?
			AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.chat_jid = o.chat_jid AND m.id = o.message_id)
		ORDER BY ts DESC, pending DESC, seq DESC LIMIT ?`, chatJID, chatJID, OutboxSent, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot list messages: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var msgs []Message
	for rows.Next() {
		var m Message
		var ts, pending, seq int64
		if err := rows.Scan(&m.ID, &m.ChatJID, &m.Kind, &m.Text, &m.FromMe, &ts, &m.Read,
			&m.Mime, &m.Size, &m.FileName, &m.State, &m.ClientID, &pending, &seq, &m.ReplyTo); err != nil {
			return nil, fmt.Errorf("cannot read message: %w", err)
		}
		m.Timestamp = fromNanos(ts)
		msgs = append(msgs, m)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list messages: %w", err)
	}
	reactions, err := chatReactions(db, chatJID)
	if err != nil {
		return nil, err
	}
	for i := range msgs {
		msgs[i].Reactions = reactions[msgs[i].ID]
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
