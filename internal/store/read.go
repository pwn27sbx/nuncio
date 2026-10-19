package store

import (
	"database/sql"
	"fmt"
)

// UnreadIncoming returns the unread messages of a chat that are not from me, oldest first, each with
// its ID, ChatJID, Timestamp and recorded Sender (empty for a message stored before senders were
// recorded). It is what a read receipt for the chat needs; text and media are not read.
func UnreadIncoming(db *sql.DB, chatJID string) ([]Message, error) {
	rows, err := db.Query(`SELECT m.id, m.ts, COALESCE(mm.sender, '')
		FROM messages m LEFT JOIN message_meta mm ON mm.chat_jid = m.chat_jid AND mm.id = m.id
		WHERE m.chat_jid = ? AND m.from_me = 0 AND m.read = 0
		ORDER BY m.ts, m.seq`, chatJID)
	if err != nil {
		return nil, fmt.Errorf("cannot list unread messages: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var msgs []Message
	for rows.Next() {
		m := Message{ChatJID: chatJID}
		var ts int64
		if err := rows.Scan(&m.ID, &ts, &m.Sender); err != nil {
			return nil, fmt.Errorf("cannot list unread messages: %w", err)
		}
		m.Timestamp = fromNanos(ts)
		msgs = append(msgs, m)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list unread messages: %w", err)
	}
	return msgs, nil
}

// MarkMessagesRead marks the messages ids of a chat read, in one transaction. A message that
// arrived after the ids were listed is not among them and stays unread.
func MarkMessagesRead(db *sql.DB, chatJID string, ids []string) (err error) {
	tx, err := db.Begin()
	if err != nil {
		return fmt.Errorf("cannot mark messages read: %w", err)
	}
	defer func() {
		if err != nil {
			_ = tx.Rollback()
		}
	}()
	for _, id := range ids {
		if _, err = tx.Exec(`UPDATE messages SET read = 1 WHERE chat_jid = ? AND id = ?`, chatJID, id); err != nil {
			return fmt.Errorf("cannot mark messages read: %w", err)
		}
	}
	if err = tx.Commit(); err != nil {
		return fmt.Errorf("cannot mark messages read: %w", err)
	}
	return nil
}
