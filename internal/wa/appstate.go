package wa

import (
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
// A flag change publishes nothing (there is no chat event yet): clients see it on their next
// `chats`. A failure is logged without the chat JID and the session goes on.

// appStateChat is the chat JID of an app-state event, or false when it is not a chat JID.
func appStateChat(jid types.JID) (string, bool) {
	chat := jid.String()
	return chat, ipc.ValidJID(chat)
}

func (s *Session) applyPin(evt *waevents.Pin) {
	chat, ok := appStateChat(evt.JID)
	if !ok || evt.Action == nil {
		return
	}
	if err := store.SetChatPinned(s.db, chat, evt.Action.GetPinned()); err != nil {
		log.Printf("wa: cannot apply a pin from app state: %v", err)
	}
}

func (s *Session) applyArchive(evt *waevents.Archive) {
	chat, ok := appStateChat(evt.JID)
	if !ok || evt.Action == nil {
		return
	}
	if err := store.SetChatArchived(s.db, chat, evt.Action.GetArchived()); err != nil {
		log.Printf("wa: cannot apply an archive from app state: %v", err)
	}
}

// applyMute stores the mute end the way whatsmeow reads it: a negative end is no end, otherwise
// it is Unix milliseconds; unmuted clears it.
func (s *Session) applyMute(evt *waevents.Mute) {
	chat, ok := appStateChat(evt.JID)
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
		log.Printf("wa: cannot apply a mute from app state: %v", err)
	}
}

// applyMarkChatAsRead marks a chat read on the phone as read here too (MarkChatRead: the unread
// count clears and is published, and no read receipt is sent). A chat marked unread on the phone
// changes nothing: nuncio has no "marked unread" flag and does not invent unread messages.
func (s *Session) applyMarkChatAsRead(evt *waevents.MarkChatAsRead) {
	chat, ok := appStateChat(evt.JID)
	if !ok || evt.Action == nil || !evt.Action.GetRead() {
		return
	}
	if err := s.MarkChatRead(chat); err != nil {
		log.Printf("wa: cannot apply a read state from app state: %v", err)
	}
}
