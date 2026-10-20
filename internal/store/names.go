package store

import (
	"database/sql"
	"errors"
	"fmt"
)

// Chat names live in the chats table's name column: the group's subject or the contact's name, as
// the session resolves them from whatsmeow (see wa/names.go). Only a chat that is already stored is
// named, so an address book full of contacts never fills the chat list.

// SetChatName sets the name of the stored chat jid and reports whether it changed. A chat that is
// not stored is left alone (and answers false).
func SetChatName(db *sql.DB, jid, name string) (changed bool, err error) {
	res, err := db.Exec(`UPDATE chats SET name = ? WHERE jid = ? AND name <> ?`, name, jid, name)
	if err != nil {
		return false, fmt.Errorf("cannot save chat name: %w", err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return false, fmt.Errorf("cannot save chat name: %w", err)
	}
	return n > 0, nil
}

// ChatName is the stored name of chat jid (may be empty), or ErrNoChat.
func ChatName(db *sql.DB, jid string) (string, error) {
	var name string
	err := db.QueryRow(`SELECT name FROM chats WHERE jid = ?`, jid).Scan(&name)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNoChat
	}
	if err != nil {
		return "", fmt.Errorf("cannot read chat name: %w", err)
	}
	return name, nil
}

// ChatJIDs returns the JID of every stored chat, in JID order.
func ChatJIDs(db *sql.DB) ([]string, error) {
	rows, err := db.Query(`SELECT jid FROM chats ORDER BY jid`)
	if err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	defer func() { _ = rows.Close() }()
	var jids []string
	for rows.Next() {
		var jid string
		if err := rows.Scan(&jid); err != nil {
			return nil, fmt.Errorf("cannot list chats: %w", err)
		}
		jids = append(jids, jid)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("cannot list chats: %w", err)
	}
	return jids, nil
}
