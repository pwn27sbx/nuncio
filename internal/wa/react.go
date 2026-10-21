package wa

import (
	"context"
	"errors"
	"fmt"
	"log"

	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

var (
	// ErrReactOffline: reactions are never queued, so one asked for while offline is refused and
	// the user asks again once linked.
	ErrReactOffline = errors.New("the WhatsApp session is offline, so the reaction was not sent")
	// errUnknownSender: the target is someone else's message whose sender was never recorded (a
	// group message stored before M2). A key without its sender would claim the message is mine,
	// so nothing is sent.
	errUnknownSender = errors.New("the sender of that message is unknown, so the reaction was not sent")
	// errReactNotSent: WhatsApp did not accept (or did not acknowledge) the reaction.
	errReactNotSent = errors.New("WhatsApp did not accept the reaction")
)

// React sends my reaction on stored message id of the chat jid, built by whatsmeow's
// BuildReaction; an empty emoji removes my reaction. It is one attempt, only while linked, and is
// never queued or retried: unlinked answers ipc.ErrNotLinked, offline ErrReactOffline, an id not
// stored in that chat ErrNoQuote, and someone else's message whose sender is unknown
// errUnknownSender (nothing is sent or stored). A sent reaction is stored as mine and the message is published
// again as event_message with its reactions.
func (s *Session) React(jid, id, emoji string) error {
	if s.client == nil || s.State() == StateUnlinked {
		return ipc.ErrNotLinked
	}
	if s.db == nil {
		return errNoStore
	}
	if s.State() != StateLinked {
		return ErrReactOffline
	}
	jid = s.CanonicalJID(jid)
	m, err := store.GetMessage(s.db, jid, id)
	if errors.Is(err, store.ErrNoMessage) {
		return ErrNoQuote
	}
	if err != nil {
		return fmt.Errorf("wa: %w", err)
	}
	sender := ""
	if !m.FromMe {
		if sender = s.senderOf(m); sender == "" {
			return errUnknownSender
		}
	}
	msg, err := s.client.BuildReaction(jid, sender, id, emoji)
	if err != nil {
		return fmt.Errorf("wa: cannot build the reaction: %w", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), sendTimeout)
	defer cancel()
	if _, err := s.client.SendMessage(ctx, jid, msg, ""); err != nil {
		// The error's type only: whatsmeow's text can carry a JID.
		log.Printf("wa: a reaction failed (%T)", err)
		return errReactNotSent
	}
	if err := store.SetReaction(s.db, jid, id, "", true, emoji); err != nil {
		// A store failure is logged by its error type only: SQLite's text could quote a value.
		log.Printf("wa: cannot store a sent reaction (%T)", err)
		return nil
	}
	s.publishStored(jid, id)
	return nil
}

// storeReaction records an incoming (or own, from another device) reaction against its target
// message instead of storing it as a message, and republishes the target when it is stored. An
// empty text removes that sender's reaction. Unread counts never change.
func (s *Session) storeReaction(evt *waevents.Message) {
	chat := evt.Info.Chat.String()
	if !ipc.ValidJID(chat) {
		return
	}
	chat = s.CanonicalJID(chat)
	rm := evt.Message.GetReactionMessage()
	target := rm.GetKey().GetID()
	if target == "" {
		return
	}
	sender := ""
	if !evt.Info.IsFromMe {
		sender = evt.Info.Sender.ToNonAD().String()
		if sender == evt.Info.Chat.ToNonAD().String() && !evt.Info.IsGroup {
			sender = chat
		}
	}
	if err := store.SetReaction(s.db, chat, target, sender, evt.Info.IsFromMe, rm.GetText()); err != nil {
		log.Printf("wa: cannot store a reaction (%T)", err)
		return
	}
	s.publishStored(chat, target)
}

// publishStored publishes stored message id of chat again as event_message, if it is stored, so a
// client replaces it (with its new reactions and, in a group, its sender).
func (s *Session) publishStored(chat, id string) {
	m, err := store.GetMessage(s.db, chat, id)
	if err != nil {
		return
	}
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: chat, Message: wireMessage(m, s.Senders())})
}
