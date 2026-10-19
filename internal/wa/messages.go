package wa

import (
	"fmt"
	"log"

	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// messageRow turns a whatsmeow message into a store row. Only text and extended text are stored
// for now: any other kind (media is a later task) and any chat the wire protocol does not carry
// (status updates, broadcasts, newsletters) report false. The row starts unread; the store counts
// it as unread only when it is not from me.
func messageRow(evt *waevents.Message) (store.Message, bool) {
	chat := evt.Info.Chat.String()
	if !ipc.ValidJID(chat) {
		return store.Message{}, false
	}
	text := evt.Message.GetConversation()
	if text == "" {
		text = evt.Message.GetExtendedTextMessage().GetText()
	}
	if text == "" {
		return store.Message{}, false
	}
	return store.Message{
		ID:        evt.Info.ID,
		ChatJID:   chat,
		Kind:      store.KindText,
		Text:      text,
		FromMe:    evt.Info.IsFromMe,
		Timestamp: evt.Info.Timestamp.UTC(),
	}, true
}

// storeMessage saves one incoming or own message. A repeated event is ignored by the store. A
// failure is logged without the message text and the session goes on.
func (s *Session) storeMessage(evt *waevents.Message) {
	m, ok := messageRow(evt)
	if !ok {
		return
	}
	if _, err := store.InsertMessage(s.db, m); err != nil {
		log.Printf("wa: cannot store message %s: %v", m.ID, err)
	}
}

// MarkChatRead marks every message of a chat read in the local store, which clears its unread
// count. It sends no read receipt: receipts go out only on an explicit view, never from here.
// Without a store there is nothing to mark.
func (s *Session) MarkChatRead(jid string) error {
	if s.db == nil {
		return nil
	}
	if err := store.MarkChatRead(s.db, jid); err != nil {
		return fmt.Errorf("wa: %w", err)
	}
	return nil
}
