package store

import (
	"database/sql"
	"errors"
	"fmt"
	"time"
)

// Outbox states. A send is queued until one attempt is made; that attempt leaves it sent,
// not_sent (WhatsApp did not accept it) or not_confirmed (no ack in time: it may have been
// delivered). Nothing moves a not_sent or not_confirmed entry except a manual retry, and nothing
// deletes one. Clients see both failures as the wire state not_sent.
const (
	OutboxQueued       = "queued"
	OutboxSent         = "sent"
	OutboxNotSent      = "not_sent"
	OutboxNotConfirmed = "not_confirmed"
)

// OutboxEntry is one send a client asked for, identified by the client's ClientID. MessageID is
// the WhatsApp message id chosen when the send was queued; every attempt uses it, so a retry of an
// entry is never a second message. SentAt is WhatsApp's ack time, zero unless State is sent.
// ReplyTo is the id of the message the send quotes, empty for none; every attempt quotes it.
type OutboxEntry struct {
	ClientID  string
	ChatJID   string
	Text      string
	MessageID string
	Created   time.Time
	State     string
	SentAt    time.Time
	ReplyTo   string
}

// ErrNoOutboxEntry: no send with that client_id is stored.
var ErrNoOutboxEntry = errors.New("no send with that client_id")

// Enqueue stores e as queued, creating its chat if needed, and returns it with inserted true. When
// a send with the same ClientID is already stored, nothing changes and Enqueue returns that stored
// entry with inserted false.
func Enqueue(db *sql.DB, e OutboxEntry) (OutboxEntry, bool, error) {
	inserted, err := enqueue(db, e)
	if err != nil {
		return OutboxEntry{}, false, err
	}
	stored, err := GetOutbox(db, e.ClientID)
	return stored, inserted, err
}

// enqueue writes the send, its chat and its quoted id in one transaction, so a crash cannot keep
// the send and lose the quote.
func enqueue(db *sql.DB, e OutboxEntry) (inserted bool, err error) {
	tx, err := db.Begin()
	if err != nil {
		return false, fmt.Errorf("cannot queue the send: %w", err)
	}
	defer func() {
		if err != nil {
			_ = tx.Rollback()
		}
	}()
	if _, err = tx.Exec(`INSERT INTO chats (jid) VALUES (?) ON CONFLICT (jid) DO NOTHING`, e.ChatJID); err != nil {
		return false, fmt.Errorf("cannot save the send's chat: %w", err)
	}
	res, err := tx.Exec(`INSERT INTO outbox (client_id, chat_jid, text, message_id, created, state)
		VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (client_id) DO NOTHING`,
		e.ClientID, e.ChatJID, e.Text, e.MessageID, e.Created.UnixNano(), OutboxQueued)
	if err != nil {
		return false, fmt.Errorf("cannot queue the send: %w", err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("cannot queue the send: %w", err)
	}
	if n == 1 && e.ReplyTo != "" {
		if _, err = tx.Exec(`INSERT INTO outbox_replies (client_id, reply_to) VALUES (?, ?)
			ON CONFLICT (client_id) DO NOTHING`, e.ClientID, e.ReplyTo); err != nil {
			return false, fmt.Errorf("cannot queue the send: %w", err)
		}
	}
	if err = tx.Commit(); err != nil {
		return false, fmt.Errorf("cannot queue the send: %w", err)
	}
	return n == 1, nil
}

const outboxSelect = `SELECT o.client_id, o.chat_jid, o.text, o.message_id, o.created, o.state, o.sent_at,
	COALESCE(r.reply_to, '') FROM outbox o LEFT JOIN outbox_replies r ON r.client_id = o.client_id`

func scanOutbox(row interface{ Scan(...any) error }) (OutboxEntry, error) {
	var e OutboxEntry
	var created, sentAt int64
	if err := row.Scan(&e.ClientID, &e.ChatJID, &e.Text, &e.MessageID, &created, &e.State, &sentAt, &e.ReplyTo); err != nil {
		return OutboxEntry{}, err
	}
	e.Created = fromNanos(created)
	if sentAt != 0 {
		e.SentAt = fromNanos(sentAt)
	}
	return e, nil
}

// GetOutbox returns the send stored for clientID, or ErrNoOutboxEntry.
func GetOutbox(db *sql.DB, clientID string) (OutboxEntry, error) {
	e, err := scanOutbox(db.QueryRow(outboxSelect+` WHERE o.client_id = ?`, clientID))
	if errors.Is(err, sql.ErrNoRows) {
		return OutboxEntry{}, ErrNoOutboxEntry
	}
	if err != nil {
		return OutboxEntry{}, fmt.Errorf("cannot read the send: %w", err)
	}
	return e, nil
}

// QueuedOutbox returns every queued send, in the order they were queued.
func QueuedOutbox(db *sql.DB) ([]OutboxEntry, error) {
	rows, err := db.Query(outboxSelect+` WHERE o.state = ? ORDER BY o.seq`, OutboxQueued)
	if err != nil {
		return nil, fmt.Errorf("cannot list queued sends: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var entries []OutboxEntry
	for rows.Next() {
		e, err := scanOutbox(rows)
		if err != nil {
			return nil, fmt.Errorf("cannot read a queued send: %w", err)
		}
		entries = append(entries, e)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list queued sends: %w", err)
	}
	return entries, nil
}

// RequeueFailed queues again the send stored for clientID if, and only if, it is not_sent or
// not_confirmed, and reports whether it did; a send that is queued or sent is left as it is, so
// two retries at once requeue it once.
func RequeueFailed(db *sql.DB, clientID string) (bool, error) {
	res, err := db.Exec(`UPDATE outbox SET state = ? WHERE client_id = ? AND state IN (?, ?)`,
		OutboxQueued, clientID, OutboxNotSent, OutboxNotConfirmed)
	if err != nil {
		return false, fmt.Errorf("cannot queue the send again: %w", err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("cannot queue the send again: %w", err)
	}
	return n == 1, nil
}

// SetOutboxState records the state of the send stored for clientID; sentAt is the ack time of a
// sent one and is ignored otherwise.
func SetOutboxState(db *sql.DB, clientID, state string, sentAt time.Time) error {
	var at int64
	if state == OutboxSent && !sentAt.IsZero() {
		at = sentAt.UnixNano()
	}
	if _, err := db.Exec(`UPDATE outbox SET state = ?, sent_at = ? WHERE client_id = ?`, state, at, clientID); err != nil {
		return fmt.Errorf("cannot save the send's state: %w", err)
	}
	return nil
}
