package wa

import (
	"time"

	"go.mau.fi/whatsmeow/proto/waHistorySync"
	"go.mau.fi/whatsmeow/proto/waWeb"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// History sync (whatsmeow's events.HistorySync): after a device is linked, the phone sends blobs of
// its older conversations. nuncio stores their messages and nothing else. A blob carries messages
// older than the link, so every one of them is stored read: history never counts as unread and
// never lights the badge. Nothing is downloaded (a media message keeps its reference for a download
// on request, as a live one does), nothing is asked of WhatsApp (no history on demand) and no read
// receipt is sent.
//
// A blob is imported off whatsmeow's event goroutine, one blob at a time, so a large one blocks
// neither a delivery nor the daemon's start. A message the store refuses is logged by its error's
// type only (the store's text could quote a message or a JID) and the import goes on; a blob that
// panics is contained the same way.
//
// An import is the one work whose length is the phone's, not nuncio's (a full sync is thousands of
// messages), so it is the only one Close stops instead of awaiting: the session's context is
// checked before every message and a blob still queued is dropped, so an import ends at its next
// message and a wipe never waits for a whole sync. The store is closed once no import writes to it
// (Session.Close, and the wait it bounds with importStopTimeout).
//
// The messages are stored with the same row a live delivery produces (messageRow, the same
// canonical chat, the same store), and the store keeps one message per (chat, id), so the same blob
// applied twice adds nothing. Each chat that received a message is published once as event_chat, as
// `chats` would list it now, so an open UI shows its last message without asking again.

// storeHistory is the session's events.HistorySync callback: it starts the import of one blob.
func (s *Session) storeHistory(evt *waevents.HistorySync) {
	if s.db == nil || evt == nil || evt.Data == nil {
		return
	}
	// Counted under the lock that Close uses to end the session, so an import never starts after
	// Close began asking the running ones to stop (it closes the store they write to).
	s.mu.Lock()
	run := s.started
	if run {
		s.imports.Add(1)
	}
	s.mu.Unlock()
	if !run {
		return
	}
	go func() {
		defer s.imports.Done()
		s.historyMu.Lock()
		defer s.historyMu.Unlock()
		// A blob that was queued behind the running one is dropped when the session is closing:
		// nobody is going to read what it would store.
		if s.importCtx.Err() != nil {
			return
		}
		s.importHistory(evt.Data)
	}()
}

// importHistory stores every message of one blob, conversation by conversation. It stops at the
// next message once Close cancelled the session, so a full sync never holds a closing daemon.
func (s *Session) importHistory(data *waHistorySync.HistorySync) {
	defer func() {
		if r := recover(); r != nil {
			s.log("wa: cannot import a history sync blob (%T)", r)
		}
	}()
	for _, conv := range data.GetConversations() {
		chat, ok := historyChat(conv.GetID())
		if !ok {
			continue
		}
		stored := ""
		for _, hm := range conv.GetMessages() {
			if s.importCtx.Err() != nil {
				return
			}
			m, ok := historyRow(chat, hm.GetMessage())
			if !ok {
				continue
			}
			m = s.canonicalRow(m, chat)
			inserted, err := store.InsertMessage(s.db, m)
			if err != nil {
				s.log("wa: cannot store a historical message (%T)", err)
				continue
			}
			if inserted {
				stored = m.ChatJID
			}
		}
		if stored == "" {
			continue
		}
		// A chat the import created is named at once, as its first live message would name it.
		s.nameNewChat(stored)
		s.publishChat(stored)
	}
}

// historyChat is the chat a conversation names, or false for a conversation the wire does not carry
// (status updates, newsletters) or an id that does not parse.
func historyChat(id string) (types.JID, bool) {
	jid, err := types.ParseJID(id)
	if err != nil || !ipc.ValidJID(jid.String()) {
		return types.JID{}, false
	}
	return jid, true
}

// historyRow is messageRow for one historical message: the row a live delivery would produce, with
// Read set, since a historical message never counts as unread. A protocol or system entry, and
// anything else messageRow does not carry, reports false.
func historyRow(chat types.JID, info *waWeb.WebMessageInfo) (store.Message, bool) {
	if info == nil || info.GetMessage() == nil || info.GetKey().GetID() == "" {
		return store.Message{}, false
	}
	evt := &waevents.Message{
		Info: types.MessageInfo{
			MessageSource: types.MessageSource{Chat: chat, Sender: historySender(chat, info),
				IsGroup: chat.Server == types.GroupServer, IsFromMe: info.GetKey().GetFromMe()},
			ID:        info.GetKey().GetID(),
			Timestamp: time.Unix(int64(info.GetMessageTimestamp()), 0),
			PushName:  info.GetPushName(),
		},
		RawMessage: info.GetMessage(),
	}
	// The unwrapping a live delivery has already had: ephemeral, view-once and device-sent.
	evt.UnwrapRaw()
	m, ok := messageRow(evt)
	if !ok {
		return store.Message{}, false
	}
	m.Read = true
	return m, true
}

// historySender is who sent one historical message: the chat itself in a direct chat, the
// participant in a group (its key's, else the entry's own), and none for a message of mine.
func historySender(chat types.JID, info *waWeb.WebMessageInfo) types.JID {
	if info.GetKey().GetFromMe() {
		return types.JID{}
	}
	if chat.Server != types.GroupServer {
		return chat
	}
	for _, id := range []string{info.GetKey().GetParticipant(), info.GetParticipant()} {
		if id == "" {
			continue
		}
		if jid, err := types.ParseJID(id); err == nil {
			return jid
		}
	}
	return types.JID{}
}
