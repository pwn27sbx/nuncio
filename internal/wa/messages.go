package wa

import (
	"fmt"
	"log"

	"go.mau.fi/whatsmeow/proto/waE2E"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// messageRow turns a whatsmeow message into a store row: text and extended text as text, and the
// image, video, audio and document kinds with their caption as text, their mime type, size and file
// name, and the serialised media sub-message as MediaRef, which is everything a download on request
// needs later; nothing is downloaded here. A quoted reply, text or media, records the id it quotes
// as ReplyTo. Any other kind and any chat the wire protocol does not carry (status updates,
// broadcasts, newsletters) report false. The row starts unread; the store
// counts it as unread only when it is not from me.
func messageRow(evt *waevents.Message) (store.Message, bool) {
	chat := evt.Info.Chat.String()
	if !ipc.ValidJID(chat) {
		return store.Message{}, false
	}
	m := store.Message{
		ID:        evt.Info.ID,
		ChatJID:   chat,
		Kind:      store.KindText,
		FromMe:    evt.Info.IsFromMe,
		Timestamp: evt.Info.Timestamp.UTC(),
	}
	if !m.FromMe {
		m.Sender = evt.Info.Sender.ToNonAD().String()
	}
	m.Text = evt.Message.GetConversation()
	if m.Text == "" {
		m.Text = evt.Message.GetExtendedTextMessage().GetText()
		m.ReplyTo = evt.Message.GetExtendedTextMessage().GetContextInfo().GetStanzaID()
	}
	if m.Text != "" {
		return m, true
	}
	kind, sub, caption, fileName, ok := mediaOf(evt.Message)
	if !ok {
		return store.Message{}, false
	}
	ref, err := proto.Marshal(sub)
	if err != nil {
		log.Printf("wa: cannot keep the media reference of %s: %v", m.ID, err)
		return store.Message{}, false
	}
	m.Kind, m.Text, m.FileName, m.MediaRef = kind, caption, fileName, ref
	m.Mime, m.Size = sub.GetMimetype(), int64(sub.GetFileLength())
	if quoting, ok := sub.(interface{ GetContextInfo() *waE2E.ContextInfo }); ok {
		m.ReplyTo = quoting.GetContextInfo().GetStanzaID()
	}
	return m, true
}

// WireMessage is a stored message in its wire shape: the media kinds carry their media object,
// text carries none, and a send of mine that is not sent yet carries its state and client_id (the
// store's outbox states are the wire's, except not_confirmed, which the wire calls not_sent).
func WireMessage(m store.Message) ipc.Message {
	w := ipc.Message{ID: m.ID, Kind: m.Kind, Text: m.Text, FromMe: m.FromMe, Timestamp: ipc.Timestamp(m.Timestamp),
		State: m.State, ClientID: m.ClientID, ReplyTo: m.ReplyTo}
	for _, r := range m.Reactions {
		w.Reactions = append(w.Reactions, ipc.Reaction{Emoji: r.Emoji, FromMe: r.FromMe})
	}
	if m.State == store.OutboxNotConfirmed {
		// Not confirmed is a failure the user retries by hand, like not sent.
		w.State = ipc.MessageNotSent
	}
	if m.Kind != store.KindText {
		w.Media = &ipc.Media{Mime: m.Mime, Size: m.Size, FileName: m.FileName}
	}
	return w
}

// storeMessage saves one incoming or own message and publishes it with any reactions that arrived
// before it, followed by the new unread counts when it is incoming; a reaction is stored on its
// target instead (see storeReaction). A repeated event is ignored by the store and publishes
// nothing. A failure is logged without the message text and the session goes on.
func (s *Session) storeMessage(evt *waevents.Message) {
	if evt.Message.GetReactionMessage() != nil {
		s.storeReaction(evt)
		return
	}
	m, ok := messageRow(evt)
	if !ok {
		return
	}
	inserted, err := store.InsertMessage(s.db, m)
	if err != nil {
		log.Printf("wa: cannot store message %s: %v", m.ID, err)
		return
	}
	if !inserted {
		return
	}
	// A reaction can arrive before its target; the target's first event already carries it.
	if m.Reactions, err = store.MessageReactions(s.db, m.ChatJID, m.ID); err != nil {
		log.Printf("wa: %v", err)
	}
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: m.ChatJID, Message: WireMessage(m)})
	if !m.FromMe {
		s.publishUnread(m.ChatJID)
	}
}

// publishUnread publishes the total unread count and that of chat jid.
// TODO(T8 D-Bus unit): emit UnreadChanged on the session bus from the same point.
func (s *Session) publishUnread(jid string) {
	total, err := store.TotalUnread(s.db)
	if err != nil {
		log.Printf("wa: %v", err)
		return
	}
	chat, err := store.UnreadCount(s.db, jid)
	if err != nil {
		log.Printf("wa: %v", err)
		return
	}
	s.publish(ipc.EventUnread{V: ipc.Version, Op: ipc.OpEventUnread, Unread: total, JID: jid, ChatUnread: chat})
}

// MarkChatRead marks every message of a chat read in the local store, which clears its unread
// count, and publishes the new counts when that changed them. It sends no read receipt: receipts
// go out only on an explicit view, never from here. Without a store there is nothing to mark.
func (s *Session) MarkChatRead(jid string) error {
	if s.db == nil {
		return nil
	}
	before, err := store.UnreadCount(s.db, jid)
	if err != nil {
		return fmt.Errorf("wa: %w", err)
	}
	if err := store.MarkChatRead(s.db, jid); err != nil {
		return fmt.Errorf("wa: %w", err)
	}
	if before > 0 {
		s.publishUnread(jid)
	}
	return nil
}
