package store

import (
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"
)

// errMergeSelf: MergeChat was asked to merge a chat into itself, or named no chat.
var errMergeSelf = errors.New("store: a chat can only be merged into another chat")

// MergeChat moves everything stored under the chat alias into the chat canonical, in one
// transaction, and reports whether there was anything to move (ADR 0001: a contact's phone-number
// JID is an alias of its LID chat). Every table keyed by chat follows: messages (with their search
// index), message_meta, reactions, media_refs, the outbox (outbox_replies is keyed by client_id
// and follows it), chat_mutes and the chats row. The rules:
//
//   - A message id stored in both chats is kept once, as the canonical chat's row; it is read when
//     either copy was read, and takes from the alias copy only what its own row lacks (the sender,
//     the quoted id, the media reference). Reactions follow the same rule per sender.
//   - A sender recorded as alias (the contact itself, in a direct chat) becomes canonical.
//   - Flags: pinned when either chat was pinned; archived only when both were archived (a copy in
//     the inbox keeps it there), or as the alias was when canonical had no chats row; the name of
//     canonical, or the alias's when canonical has none; muted until the later of the two ends.
//   - chat_aliases records alias -> canonical.
//
// Nothing is deleted that is not kept in canonical, unread counts are recomputed from the merged
// rows, and a failure at any step rolls the whole merge back, leaving the database as it was. A
// second call finds nothing under alias, writes nothing and answers false.
func MergeChat(db *sql.DB, alias, canonical string) (merged bool, err error) {
	if alias == "" || canonical == "" || alias == canonical {
		return false, errMergeSelf
	}
	tx, err := db.Begin()
	if err != nil {
		return false, fmt.Errorf("cannot merge chats: %w", err)
	}
	defer func() {
		if err != nil || !merged {
			_ = tx.Rollback()
		}
	}()
	var stored bool
	if err = tx.QueryRow(`SELECT EXISTS (SELECT 1 FROM chats WHERE jid = ?1)
		OR EXISTS (SELECT 1 FROM messages WHERE chat_jid = ?1)
		OR EXISTS (SELECT 1 FROM outbox WHERE chat_jid = ?1)
		OR EXISTS (SELECT 1 FROM reactions WHERE chat_jid = ?1)
		OR EXISTS (SELECT 1 FROM message_meta WHERE chat_jid = ?1)
		OR EXISTS (SELECT 1 FROM media_refs WHERE chat_jid = ?1)
		OR EXISTS (SELECT 1 FROM chat_mutes WHERE jid = ?1)`, alias).Scan(&stored); err != nil {
		return false, fmt.Errorf("cannot merge chats: %w", err)
	}
	if !stored {
		return false, nil
	}
	if err = mergeChatRow(tx, alias, canonical); err != nil {
		return false, err
	}
	steps := []string{
		// Messages: a duplicate is kept as the canonical row, read if either copy was.
		`UPDATE messages SET read = 1 WHERE chat_jid = ?2 AND read = 0
			AND id IN (SELECT id FROM messages WHERE chat_jid = ?1 AND read = 1)`,
		`DELETE FROM messages_fts WHERE rowid IN (SELECT a.seq FROM messages a
			JOIN messages c ON c.chat_jid = ?2 AND c.id = a.id WHERE a.chat_jid = ?1)`,
		`DELETE FROM messages WHERE chat_jid = ?1 AND id IN (SELECT id FROM messages WHERE chat_jid = ?2)`,
		`UPDATE messages SET chat_jid = ?2 WHERE chat_jid = ?1`,
		// Sender and quote: the canonical row keeps what it has and takes what it lacks.
		`INSERT INTO message_meta (chat_jid, id, sender, reply_to)
			SELECT ?2, id, sender, reply_to
			FROM message_meta WHERE chat_jid = ?1 AND true
			ON CONFLICT (chat_jid, id) DO UPDATE SET
				sender = CASE WHEN message_meta.sender = '' THEN excluded.sender ELSE message_meta.sender END,
				reply_to = CASE WHEN message_meta.reply_to = '' THEN excluded.reply_to ELSE message_meta.reply_to END`,
		`DELETE FROM message_meta WHERE chat_jid = ?1`,
		// The contact herself, recorded by her phone number, is now named by her LID.
		`UPDATE message_meta SET sender = ?2 WHERE chat_jid = ?2 AND sender = ?1`,
		// Reactions: one per sender and message; the canonical chat's wins.
		`INSERT INTO reactions (chat_jid, id, sender, from_me, emoji)
			SELECT ?2, id, CASE WHEN sender = ?1 THEN ?2 ELSE sender END, from_me, emoji
			FROM reactions WHERE chat_jid = ?1 AND true
			ON CONFLICT (chat_jid, id, sender) DO NOTHING`,
		`DELETE FROM reactions WHERE chat_jid = ?1`,
		`INSERT INTO media_refs (chat_jid, id, ref) SELECT ?2, id, ref FROM media_refs WHERE chat_jid = ?1 AND true
			ON CONFLICT (chat_jid, id) DO NOTHING`,
		`DELETE FROM media_refs WHERE chat_jid = ?1`,
		`UPDATE outbox SET chat_jid = ?2 WHERE chat_jid = ?1`,
		// Mutes: the later end wins, and -1 (no end) is the latest.
		`INSERT INTO chat_mutes (jid, until) SELECT ?2, until FROM chat_mutes WHERE jid = ?1 AND true
			ON CONFLICT (jid) DO UPDATE SET until = CASE
				WHEN chat_mutes.until = -1 OR excluded.until = -1 THEN -1
				ELSE MAX(chat_mutes.until, excluded.until) END`,
		`DELETE FROM chat_mutes WHERE jid = ?1`,
		`INSERT INTO chat_aliases (alias, canonical, merged_at) VALUES (?1, ?2, ?3)
			ON CONFLICT (alias) DO UPDATE SET canonical = excluded.canonical, merged_at = excluded.merged_at`,
		// Last, so a failure anywhere before it leaves the alias chat whole.
		`DELETE FROM chats WHERE jid = ?1`,
	}
	now := time.Now().UnixNano()
	for _, step := range steps {
		if _, err = tx.Exec(step, alias, canonical, now); err != nil {
			return false, fmt.Errorf("cannot merge chats: %w", err)
		}
	}
	if err = tx.Commit(); err != nil {
		return false, fmt.Errorf("cannot merge chats: %w", err)
	}
	return true, nil
}

// mergeChatRow gives canonical its chats row with the merged name, pin and archive (see MergeChat).
func mergeChatRow(tx *sql.Tx, alias, canonical string) error {
	type row struct {
		ok             bool
		name           string
		archived, pinn bool
	}
	read := func(jid string) (row, error) {
		var r row
		err := tx.QueryRow(`SELECT name, archived, pinned FROM chats WHERE jid = ?`, jid).Scan(&r.name, &r.archived, &r.pinn)
		if errors.Is(err, sql.ErrNoRows) {
			return row{}, nil
		}
		if err != nil {
			return row{}, fmt.Errorf("cannot merge chats: %w", err)
		}
		r.ok = true
		return r, nil
	}
	a, err := read(alias)
	if err != nil {
		return err
	}
	c, err := read(canonical)
	if err != nil {
		return err
	}
	m := c
	switch {
	case !c.ok:
		m = a
	case a.ok:
		m.pinn = a.pinn || c.pinn
		m.archived = a.archived && c.archived
		if m.name == "" {
			m.name = a.name
		}
	}
	if _, err := tx.Exec(`INSERT INTO chats (jid, name, archived, pinned) VALUES (?, ?, ?, ?)
		ON CONFLICT (jid) DO UPDATE SET name = excluded.name, archived = excluded.archived, pinned = excluded.pinned`,
		canonical, m.name, m.archived, m.pinn); err != nil {
		return fmt.Errorf("cannot merge chats: %w", err)
	}
	return nil
}

// PhoneChats returns every chat JID addressed by phone number (`digits@s.whatsapp.net`) that has
// anything stored in a table MergeChat moves, in JID order: the candidates for MergeChat once
// their LID is known.
func PhoneChats(db *sql.DB) ([]string, error) {
	rows, err := db.Query(`SELECT jid FROM chats UNION SELECT chat_jid FROM messages
		UNION SELECT chat_jid FROM outbox UNION SELECT chat_jid FROM reactions
		UNION SELECT chat_jid FROM message_meta UNION SELECT chat_jid FROM media_refs
		UNION SELECT jid FROM chat_mutes ORDER BY 1`)
	if err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []string
	for rows.Next() {
		var jid string
		if err := rows.Scan(&jid); err != nil {
			return nil, fmt.Errorf("cannot list chats: %w", err)
		}
		if strings.HasSuffix(jid, "@s.whatsapp.net") {
			out = append(out, jid)
		}
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	return out, nil
}

// AliasesOf returns the phone-number chat JIDs chat_aliases records as merged into canonical, in
// JID order. It is only a record of merges: the session lists one as an alias only while
// whatsmeow's LID map still points it to canonical.
func AliasesOf(db *sql.DB, canonical string) ([]string, error) {
	rows, err := db.Query(`SELECT alias FROM chat_aliases WHERE canonical = ? ORDER BY alias`, canonical)
	if err != nil {
		return nil, fmt.Errorf("cannot list a chat's aliases: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var out []string
	for rows.Next() {
		var alias string
		if err := rows.Scan(&alias); err != nil {
			return nil, fmt.Errorf("cannot list a chat's aliases: %w", err)
		}
		out = append(out, alias)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list a chat's aliases: %w", err)
	}
	return out, nil
}
