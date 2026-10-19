package store

import (
	"database/sql"
	"fmt"
)

// Reaction is the current reaction of one sender on a message: mine (FromMe) or someone else's.
type Reaction struct {
	Emoji  string
	FromMe bool
}

// SetReaction records sender's reaction on message id of a chat, replacing that sender's earlier
// one; an empty emoji removes it. sender is the reactor's JID without device, empty for mine. The
// target message does not have to be stored yet, so a reaction that arrives first is kept.
func SetReaction(db *sql.DB, chatJID, id, sender string, fromMe bool, emoji string) error {
	var err error
	if emoji == "" {
		_, err = db.Exec(`DELETE FROM reactions WHERE chat_jid = ? AND id = ? AND sender = ?`, chatJID, id, sender)
	} else {
		_, err = db.Exec(`INSERT INTO reactions (chat_jid, id, sender, from_me, emoji) VALUES (?, ?, ?, ?, ?)
			ON CONFLICT (chat_jid, id, sender) DO UPDATE SET emoji = excluded.emoji, from_me = excluded.from_me`,
			chatJID, id, sender, fromMe, emoji)
	}
	if err != nil {
		return fmt.Errorf("cannot save reaction: %w", err)
	}
	return nil
}

// chatReactions returns the reactions of a chat by message id, mine first.
func chatReactions(db *sql.DB, chatJID string) (map[string][]Reaction, error) {
	rows, err := db.Query(`SELECT id, emoji, from_me FROM reactions WHERE chat_jid = ?
		ORDER BY id, from_me DESC, sender`, chatJID)
	if err != nil {
		return nil, fmt.Errorf("cannot read reactions: %w", err)
	}
	defer func() { _ = rows.Close() }()
	out := map[string][]Reaction{}
	for rows.Next() {
		var id string
		var r Reaction
		if err := rows.Scan(&id, &r.Emoji, &r.FromMe); err != nil {
			return nil, fmt.Errorf("cannot read reaction: %w", err)
		}
		out[id] = append(out[id], r)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot read reactions: %w", err)
	}
	return out, nil
}

// SearchChat returns up to limit stored messages of one chat whose text contains every word of
// query, newest first, each with its quoted id and reactions. Like Search, the words are matched
// literally, so FTS5 syntax typed by the user cannot break the query; a query with no words finds
// nothing.
func SearchChat(db *sql.DB, chatJID, query string, limit int) ([]Message, error) {
	match := quoteTerms(query)
	if match == "" {
		return nil, nil
	}
	rows, err := db.Query(`SELECT m.id, m.chat_jid, m.kind, m.text, m.from_me, m.ts, m.read,
			COALESCE(m.mime, ''), COALESCE(m.size, 0), COALESCE(m.file_name, ''), COALESCE(mm.reply_to, '')
		FROM messages_fts f JOIN messages m ON m.seq = f.rowid
		LEFT JOIN message_meta mm ON mm.chat_jid = m.chat_jid AND mm.id = m.id
		WHERE messages_fts MATCH ? AND m.chat_jid = ? ORDER BY m.ts DESC, m.seq DESC LIMIT ?`, match, chatJID, limit)
	if err != nil {
		return nil, fmt.Errorf("cannot search messages: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var msgs []Message
	for rows.Next() {
		var m Message
		var ts int64
		if err := rows.Scan(&m.ID, &m.ChatJID, &m.Kind, &m.Text, &m.FromMe, &ts, &m.Read,
			&m.Mime, &m.Size, &m.FileName, &m.ReplyTo); err != nil {
			return nil, fmt.Errorf("cannot read search hit: %w", err)
		}
		m.Timestamp = fromNanos(ts)
		msgs = append(msgs, m)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot search messages: %w", err)
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
