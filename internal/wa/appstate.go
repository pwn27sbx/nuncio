package wa

import (
	"database/sql"
	"log"
	"time"

	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// App-state sync is read-only: the pins, archives, mutes and read states set on another device
// (including the initial full sync) are applied to the store, and nothing is ever sent back to
// WhatsApp. A change to a chat not stored yet creates it, so the flag is not lost before its first
// message. Events for chats the wire protocol does not carry, or with no action, are ignored.
//
// A pin, archive or mute that is applied publishes the chat as `event_chat`, as `chats` would list
// it now, so a UI updates its markers at once. A read state publishes only its `event_unread`. A
// failure is logged by its error type only (the store's text could quote a JID) and the session
// goes on.

// appStateChat is the chat JID of an app-state event, or false when it is not a chat JID.
func (s *Session) appStateChat(jid types.JID) (string, bool) {
	chat := jid.String()
	if !ipc.ValidJID(chat) {
		return "", false
	}
	return s.CanonicalJID(chat), true
}

func (s *Session) applyPin(evt *waevents.Pin) {
	chat, ok := s.appStateChat(evt.JID)
	if !ok || evt.Action == nil {
		return
	}
	if err := store.SetChatPinned(s.db, chat, evt.Action.GetPinned()); err != nil {
		log.Printf("wa: cannot apply a pin from app state (%T)", err)
		return
	}
	s.publishChat(chat)
}

func (s *Session) applyArchive(evt *waevents.Archive) {
	chat, ok := s.appStateChat(evt.JID)
	if !ok || evt.Action == nil {
		return
	}
	if err := store.SetChatArchived(s.db, chat, evt.Action.GetArchived()); err != nil {
		log.Printf("wa: cannot apply an archive from app state (%T)", err)
		return
	}
	s.publishChat(chat)
}

// applyMute stores the mute end the way whatsmeow reads it: a negative end is no end, otherwise
// it is Unix milliseconds; unmuted clears it.
func (s *Session) applyMute(evt *waevents.Mute) {
	chat, ok := s.appStateChat(evt.JID)
	if !ok || evt.Action == nil {
		return
	}
	var until time.Time
	if evt.Action.GetMuted() {
		until = store.MutedForever
		if end := evt.Action.GetMuteEndTimestamp(); end >= 0 {
			until = time.UnixMilli(end).UTC()
		}
	}
	if err := store.SetChatMutedUntil(s.db, chat, until); err != nil {
		log.Printf("wa: cannot apply a mute from app state (%T)", err)
		return
	}
	s.publishChat(chat)
}

// applyMarkChatAsRead marks a chat read on the phone as read here too (MarkChatRead: the unread
// count clears and is published, and no read receipt is sent). A chat marked unread on the phone
// changes nothing: nuncio has no "marked unread" flag and does not invent unread messages.
func (s *Session) applyMarkChatAsRead(evt *waevents.MarkChatAsRead) {
	chat, ok := s.appStateChat(evt.JID)
	if !ok || evt.Action == nil || !evt.Action.GetRead() {
		return
	}
	if err := s.MarkChatRead(chat); err != nil {
		log.Printf("wa: cannot apply a read state from app state (%T)", err)
	}
}

// publishChat publishes chat jid as event_chat, as it is stored now, with the phone-number chats
// merged into it (ChatAliases) and its masked phone number (ChatPhone).
func (s *Session) publishChat(jid string) {
	c, err := StoredChat(s.db, jid, time.Now())
	if err != nil {
		log.Printf("wa: cannot publish a chat changed by app state (%T)", err)
		return
	}
	c.Aliases = s.ChatAliases(jid)
	c.Phone = s.ChatPhone(jid)
	s.publish(ipc.EventChat{V: ipc.Version, Op: ipc.OpEventChat, Chat: c})
}

// StoredChat is chat jid as the wire carries it (see WireChat), or store.ErrNoChat.
func StoredChat(db *sql.DB, jid string, now time.Time) (ipc.Chat, error) {
	c, err := store.GetChat(db, jid)
	if err != nil {
		return ipc.Chat{}, err
	}
	return WireChat(db, c, now)
}

// WireChat is the stored chat c as the wire carries it: its name, unread count, flags (muted only
// while its mute lasts at now) and newest message, if any.
func WireChat(db *sql.DB, c store.Chat, now time.Time) (ipc.Chat, error) {
	chat := ipc.Chat{JID: c.JID, Name: c.Name, Unread: c.Unread, Archived: c.Archived, Pinned: c.Pinned,
		Muted: c.Muted(now)}
	last, err := store.ListMessages(db, c.JID, 1)
	if err != nil {
		return ipc.Chat{}, err
	}
	if len(last) == 1 {
		// The chat list carries no previews: 200 chats with a thumbnail each would not fit a frame.
		last[0].MediaRef = nil
		m := WireMessage(last[0])
		chat.LastMessage = &m
	}
	return chat, nil
}
