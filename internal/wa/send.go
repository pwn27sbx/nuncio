package wa

import (
	"context"
	"errors"
	"log"
	"strings"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// sendTimeout bounds one send; it is shorter than ipc.SendTimeout, so a client always gets the
// daemon's answer instead of its own timeout. Hitting it answers ipc.ErrNotConfirmed.
const sendTimeout = 20 * time.Second

var (
	// ErrNotSent: WhatsApp did not accept the message, or an earlier attempt of this send ended
	// not sent. whatsmeow's own error is not passed on, because it can name a number.
	ErrNotSent = errors.New("WhatsApp did not accept the message")
	// ErrNoSend: a retry named a client_id this daemon never stored.
	ErrNoSend = errors.New("no send with that client_id")
	// errNoStore: a linked session without a message store has no outbox to send through.
	errNoStore = errors.New("there is no message store, so nothing was sent")
	// errOutbox: the outbox could not be read or written, so nothing was sent.
	errOutbox = errors.New("the message could not be saved, so it was not sent")
	// ErrNoQuote: a reply or a reaction named a message id that is not stored in that chat.
	ErrNoQuote = errors.New("no message with that id in that chat")
)

// Send sends one text message to the chat jid, which the caller has checked with ipc.ValidJID,
// through the outbox: the send is stored under clientID with a WhatsApp message id chosen now, so
// it always has a durable state. While unlinked it returns ipc.ErrNotLinked and stores nothing.
// While offline the send stays queued (published as event_message with state queued) and drains
// once on the next reconnect. While linked it is attempted once now; see deliver for the outcomes.
//
// A clientID already stored answers that send's state and sends nothing: queued, sent (with its
// id and ack time) or ErrNotSent. Only Retry sends a not-sent message again.
func (s *Session) Send(jid, text, clientID string) (ipc.Sent, error) {
	return s.SendReply(jid, text, clientID, "")
}

// SendReply is Send quoting the message replyTo of the same chat ("" for none, which is Send). The
// quoted message must be stored in that chat, otherwise it answers ErrNoQuote and stores nothing;
// a clientID already stored answers that send's state first, as for Send, whatever replyTo names.
// The outbox keeps replyTo, so a queued reply still quotes when the drain or a retry sends it.
func (s *Session) SendReply(jid, text, clientID, replyTo string) (ipc.Sent, error) {
	if s.client == nil || s.State() == StateUnlinked {
		return ipc.Sent{}, ipc.ErrNotLinked
	}
	if s.db == nil {
		return ipc.Sent{}, errNoStore
	}
	jid = s.CanonicalJID(jid)
	if replyTo != "" {
		// A client_id already stored is that send again, answered from its state whatever it quotes.
		if e, err := store.GetOutbox(s.db, clientID); err == nil {
			return storedReply(e)
		} else if !errors.Is(err, store.ErrNoOutboxEntry) {
			log.Printf("wa: cannot read a send: %v", err)
			return ipc.Sent{}, errOutbox
		}
		if _, err := store.GetMessage(s.db, jid, replyTo); errors.Is(err, store.ErrNoMessage) {
			return ipc.Sent{}, ErrNoQuote
		} else if err != nil {
			log.Printf("wa: cannot read a quoted message: %v", err)
			return ipc.Sent{}, errOutbox
		}
	}
	e, inserted, err := store.Enqueue(s.db, store.OutboxEntry{ClientID: clientID, ChatJID: jid, Text: text,
		MessageID: s.client.NewMessageID(), Created: time.Now().UTC(), ReplyTo: replyTo})
	if err != nil {
		log.Printf("wa: cannot queue a send: %v", err)
		return ipc.Sent{}, errOutbox
	}
	if !inserted {
		return storedReply(e)
	}
	return s.sendOrQueue(e)
}

// Retry is the manual retry: the send stored under clientID that ended not sent (or not
// confirmed) is queued again and, while linked, attempted once now under its first WhatsApp
// message id, so it can never become a second message; while offline it drains on the next
// reconnect. A send that is queued or sent answers its state and sends nothing, and of two retries
// at once only one requeues it; an unknown clientID is ErrNoSend.
func (s *Session) Retry(clientID string) (ipc.Sent, error) {
	if s.client == nil || s.State() == StateUnlinked {
		return ipc.Sent{}, ipc.ErrNotLinked
	}
	if s.db == nil {
		return ipc.Sent{}, errNoStore
	}
	requeued, err := store.RequeueFailed(s.db, clientID)
	if err != nil {
		log.Printf("wa: cannot queue a retry: %v", err)
		return ipc.Sent{}, errOutbox
	}
	e, err := store.GetOutbox(s.db, clientID)
	if errors.Is(err, store.ErrNoOutboxEntry) {
		return ipc.Sent{}, ErrNoSend
	}
	if err != nil {
		log.Printf("wa: cannot read a send to retry: %v", err)
		return ipc.Sent{}, errOutbox
	}
	if !requeued {
		return storedReply(e)
	}
	if c := s.CanonicalJID(e.ChatJID); c != e.ChatJID {
		e.ChatJID = c // the merge moved the entry there
	}
	return s.sendOrQueue(e)
}

// sendOrQueue attempts a queued send now while linked, and otherwise leaves it queued for the
// next reconnect's drain and publishes it as queued.
func (s *Session) sendOrQueue(e store.OutboxEntry) (ipc.Sent, error) {
	if s.State() != StateLinked {
		s.publishEntry(e)
		return ipc.Sent{State: ipc.SendQueued}, nil
	}
	return s.deliver(e.ClientID, true)
}

// storedReply is the answer for a send in its stored state, without sending anything: a send whose
// ack never came stays ipc.ErrNotConfirmed, one WhatsApp refused stays ErrNotSent.
func storedReply(e store.OutboxEntry) (ipc.Sent, error) {
	switch e.State {
	case store.OutboxSent:
		return ipc.Sent{State: ipc.SendSent, ID: e.MessageID, Timestamp: ipc.Timestamp(e.SentAt)}, nil
	case store.OutboxNotSent:
		return ipc.Sent{}, ErrNotSent
	case store.OutboxNotConfirmed:
		return ipc.Sent{}, ipc.ErrNotConfirmed
	}
	return ipc.Sent{State: ipc.SendQueued}, nil
}

// deliver gets the queued send stored under clientID its one attempt, under sendMu, after every
// send queued before it, so no send overtakes an older one: while linked, each older queued send
// gets its one attempt first, in order. A send that a drain or another call already attempted is
// answered from its stored state, never sent twice. live is true for the attempt a send or retry
// makes for its caller and false for a drain's; see attempt.
func (s *Session) deliver(clientID string, live bool) (ipc.Sent, error) {
	s.sendMu.Lock()
	defer s.sendMu.Unlock()
	queued, err := store.QueuedOutbox(s.db)
	if err != nil {
		log.Printf("wa: cannot read the outbox: %v", err)
		return ipc.Sent{}, errOutbox
	}
	for _, e := range queued {
		if e.ClientID == clientID {
			return s.attemptLive(e, live)
		}
		if s.State() != StateLinked {
			break
		}
		_, _ = s.attempt(e, false)
	}
	e, err := store.GetOutbox(s.db, clientID)
	if err != nil {
		log.Printf("wa: cannot read a queued send: %v", err)
		return ipc.Sent{}, errOutbox
	}
	if e.State == store.OutboxQueued {
		return s.attemptLive(e, live)
	}
	return storedReply(e)
}

// attemptLive is attempt, except that a live send whose session is no longer linked by the time
// its turn comes (an older send's attempt, or a drop, took the connection down) is not attempted
// on a connection that is gone: it stays queued for the next reconnect's drain, is published as
// queued and answers queued.
func (s *Session) attemptLive(e store.OutboxEntry, live bool) (ipc.Sent, error) {
	if live && s.State() == StateOffline {
		s.publishEntry(e)
		return ipc.Sent{State: ipc.SendQueued}, nil
	}
	return s.attempt(e, live)
}

// attempt is the one attempt of queued send e; the caller holds sendMu. On success the entry is
// sent, the message is stored as mine (never unread) and published as event_message; a store
// failure after that is logged and the send still counts. Any failure is published with the wire
// state not_sent and stored with its reason: an unknown outcome (unknownOutcome: an ack that does
// not arrive in time, or a connection lost while it waits) is not_confirmed and answers ipc.ErrNotConfirmed (it may have been delivered; a retry reuses its
// WhatsApp id), anything else is not_sent and answers ErrNotSent. Nothing is retried on its own.
//
// One exception, for the live attempt only (live: the one a send or retry makes for its caller):
// an unknown outcome because the connection died while it waited (leftLinked)
// leaves the entry queued, under the WhatsApp id it was queued with, publishes it as queued and
// answers queued, so the next reconnect's one drain sends it, as WhatsApp itself queues a message
// sent as the network drops. A drain's attempt never does that: its failures end as above, so a
// send gets at most one automatic resend.
func (s *Session) attempt(e store.OutboxEntry, live bool) (ipc.Sent, error) {
	// An entry queued under a phone number before its LID was known is sent, and kept, in the LID
	// chat once it is (the merge moves the entry there).
	e.ChatJID = s.CanonicalJID(e.ChatJID)
	ctx, cancel := context.WithTimeout(context.Background(), sendTimeout)
	defer cancel()
	drops := s.dropCount()
	ts, err := s.client.SendMessage(ctx, e.ChatJID, s.textMessage(e), e.MessageID)
	if err != nil {
		state, reply := store.OutboxNotSent, ErrNotSent
		if unknownOutcome(err) {
			if live && s.leftLinked(drops) {
				// The row is still queued: no state to write, and the drain waits on sendMu.
				log.Print("wa: a send was not acknowledged before the connection dropped; it stays queued")
				e.State = store.OutboxQueued
				s.publishEntry(e)
				return ipc.Sent{State: ipc.SendQueued}, nil
			}
			// whatsmeow writes the frame before it waits for the ack, so the message may have
			// arrived: report the outcome as unknown, never as not accepted.
			state, reply = store.OutboxNotConfirmed, ipc.ErrNotConfirmed
			log.Print("wa: a send was not acknowledged in time")
		} else {
			// The error's type only: whatsmeow's text can carry a JID.
			log.Printf("wa: a send failed (%T)", err)
		}
		if err := store.SetOutboxState(s.db, e.ClientID, state, time.Time{}); err != nil {
			log.Printf("wa: cannot record a failed send: %v", err)
		}
		e.State = state
		s.publishEntry(e)
		return ipc.Sent{}, reply
	}
	if ts.IsZero() {
		// An ack without a timestamp; the zero time would overflow the store's nanoseconds.
		ts = time.Now()
	}
	ts = ts.UTC()
	if err := store.SetOutboxState(s.db, e.ClientID, store.OutboxSent, ts); err != nil {
		log.Printf("wa: cannot record a sent message %s: %v", e.MessageID, err)
	}
	// whatsmeow may have learnt the LID during this very send (GetUserInfo, user.go:265-276): the
	// message is then stored in the LID chat and the entry, with anything else, moves there.
	e.ChatJID = s.CanonicalJID(e.ChatJID)
	m := store.Message{ID: e.MessageID, ChatJID: e.ChatJID, Kind: store.KindText, Text: e.Text, FromMe: true, Timestamp: ts,
		ReplyTo: e.ReplyTo}
	if _, err := store.InsertMessage(s.db, m); err != nil {
		log.Printf("wa: cannot store sent message %s: %v", m.ID, err)
	}
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: m.ChatJID, Message: WireMessage(m)})
	return ipc.Sent{State: ipc.SendSent, ID: m.ID, Timestamp: ipc.Timestamp(ts)}, nil
}

// textMessage is the WhatsApp message of send e: a plain conversation, or for a reply an extended
// text whose ContextInfo quotes the stored message (its id as StanzaID, its sender as Participant,
// the message itself as QuotedMessage; see quotedOf), which is what WhatsApp shows as a quote. A
// quoted message that is gone by now sends the text unquoted rather than not at all.
func (s *Session) textMessage(e store.OutboxEntry) *waE2E.Message {
	if e.ReplyTo == "" {
		return &waE2E.Message{Conversation: proto.String(e.Text)}
	}
	q, err := store.GetMessage(s.db, e.ChatJID, e.ReplyTo)
	if err != nil {
		log.Printf("wa: a reply's quoted message %s cannot be read, so it is sent unquoted: %v", e.ReplyTo, err)
		return &waE2E.Message{Conversation: proto.String(e.Text)}
	}
	ci := &waE2E.ContextInfo{StanzaID: proto.String(q.ID), QuotedMessage: quotedOf(q)}
	if p := s.senderOf(q); p != "" {
		ci.Participant = proto.String(p)
	}
	return &waE2E.Message{ExtendedTextMessage: &waE2E.ExtendedTextMessage{Text: proto.String(e.Text), ContextInfo: ci}}
}

// quotedOf is stored message q as a quote carries it: a media message as its own kind (image,
// video, audio or document) with its caption, mime type and file name, taken from its stored
// reference when it has one and from the row otherwise, so WhatsApp shows a quoted photo or file;
// a text, or a media row that keeps neither its reference nor its mime type, as a conversation of
// its text. The quote carries no media key or path, only what WhatsApp shows of the quoted message.
func quotedOf(q store.Message) *waE2E.Message {
	caption, mime, fileName := q.Text, q.Mime, q.FileName
	if len(q.MediaRef) > 0 {
		if sub, err := decodeRef(q.Kind, q.MediaRef); err == nil {
			if v := sub.GetMimetype(); v != "" {
				mime = v
			}
			switch m := sub.(type) {
			case *waE2E.ImageMessage:
				caption = m.GetCaption()
			case *waE2E.VideoMessage:
				caption = m.GetCaption()
			case *waE2E.DocumentMessage:
				caption, fileName = m.GetCaption(), m.GetFileName()
			}
		}
	}
	if mime == "" {
		return &waE2E.Message{Conversation: proto.String(q.Text)}
	}
	switch q.Kind {
	case store.KindImage:
		return &waE2E.Message{ImageMessage: &waE2E.ImageMessage{Caption: proto.String(caption), Mimetype: proto.String(mime)}}
	case store.KindVideo:
		return &waE2E.Message{VideoMessage: &waE2E.VideoMessage{Caption: proto.String(caption), Mimetype: proto.String(mime)}}
	case store.KindAudio:
		return &waE2E.Message{AudioMessage: &waE2E.AudioMessage{Mimetype: proto.String(mime)}}
	case store.KindDocument:
		return &waE2E.Message{DocumentMessage: &waE2E.DocumentMessage{Caption: proto.String(caption),
			Mimetype: proto.String(mime), FileName: proto.String(fileName)}}
	}
	return &waE2E.Message{Conversation: proto.String(q.Text)}
}

// senderOf is who sent stored message m: me for mine (my own LID in a chat addressed by LID, when
// the device store knows it, my phone-number JID otherwise), its recorded sender otherwise, and
// for a message stored before senders were recorded the chat itself when it is a direct chat
// (empty in a group, where it is unknown).
func (s *Session) senderOf(m store.Message) string {
	switch {
	case m.FromMe:
		if lid := s.client.OwnLID(); lid != "" && strings.HasSuffix(m.ChatJID, "@lid") {
			return lid
		}
		return s.client.OwnJID()
	case m.Sender != "":
		return m.Sender
	case !strings.HasSuffix(m.ChatJID, "@g.us"):
		return m.ChatJID
	}
	return ""
}

// unknownOutcome reports whether a failed SendMessage may still have reached WhatsApp: its ack
// timed out, or the socket dropped while it waited (whatsmeow clears the response waiter with a
// stream end and, once its retryFrame gives up, answers a DisconnectedError; errors.As, because
// DisconnectedError.Is compares the Action, which differs for the retried frame). whatsmeow writes
// the frame before it waits, so neither is ever "not accepted".
func unknownOutcome(err error) bool {
	var dropped *whatsmeow.DisconnectedError
	return errors.Is(err, context.DeadlineExceeded) || errors.Is(err, whatsmeow.ErrMessageTimedOut) || errors.As(err, &dropped)
}

// dropCount is how many times the session has left linked so far.
func (s *Session) dropCount() uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.drops
}

// leftLinked reports whether the connection a send was attempted on died before its answer:
// the session is offline now, or it has left linked at least once since dropCount answered
// drops, even if it has reconnected since. It is decided when the attempt returns, from state the
// session already holds; nothing waits for a Disconnected that has not arrived. An unlinked
// session (logged out) never counts, since nothing would drain the send.
func (s *Session) leftLinked(drops uint64) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.state == StateUnlinked {
		return false
	}
	return s.state != StateLinked || s.drops != drops
}

// publishEntry publishes a send that is not sent (queued or not_sent) as event_message, with its
// state and client_id, timestamped when it was queued.
func (s *Session) publishEntry(e store.OutboxEntry) {
	m := store.Message{ID: e.MessageID, ChatJID: e.ChatJID, Kind: store.KindText, Text: e.Text, FromMe: true,
		Timestamp: e.Created, State: e.State, ClientID: e.ClientID, ReplyTo: e.ReplyTo}
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: e.ChatJID, Message: WireMessage(m)})
}

// drain is the one pass a reconnect makes over the outbox: every queued send, oldest first, gets
// its one attempt. A failed one stays not_sent and no later drain attempts it again; when the
// connection drops mid-pass the rest stays queued for the next reconnect. Nothing here schedules
// anything.
func (s *Session) drain() {
	queued, err := store.QueuedOutbox(s.db)
	if err != nil {
		log.Printf("wa: cannot read the outbox: %v", err)
		return
	}
	for _, e := range queued {
		if s.State() != StateLinked {
			return
		}
		_, _ = s.deliver(e.ClientID, false)
	}
}
