package wa

import (
	"context"
	"errors"
	"log"
	"time"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// sendTimeout bounds one send; it is shorter than ipc.SendTimeout, so a client always gets the
// daemon's answer instead of its own timeout. Hitting it answers ipc.ErrNotConfirmed.
const sendTimeout = 20 * time.Second

var (
	// ErrOffline: the session is paired but not connected, so nothing was sent. The outbox (T11)
	// replaces this answer with a queued send that drains once on reconnect.
	ErrOffline = errors.New("the WhatsApp session is offline, so the message was not sent (nothing is queued)")
	// ErrNotSent: WhatsApp did not accept the message. whatsmeow's own error is not passed on,
	// because it can name a number.
	ErrNotSent = errors.New("WhatsApp did not accept the message")
)

// SendText sends one text message to the chat jid, which the caller has checked with
// ipc.ValidJID. While unlinked it returns ipc.ErrNotLinked and while offline ErrOffline, sending
// and storing nothing; an ack that does not arrive in time is ipc.ErrNotConfirmed, storing nothing. Once WhatsApp accepts the message it is stored as mine (never unread) and
// published as event_message; a store failure after that is logged and the send still counts.
//
// Each call is one send: a repeated client_id is not recognised yet, because remembering accepted
// sends belongs to the outbox (T11).
func (s *Session) SendText(jid, text string) (ipc.Message, error) {
	switch st := s.State(); {
	case s.client == nil || st == StateUnlinked:
		return ipc.Message{}, ipc.ErrNotLinked
	case st == StateOffline:
		return ipc.Message{}, ErrOffline
	}
	ctx, cancel := context.WithTimeout(context.Background(), sendTimeout)
	defer cancel()
	id, ts, err := s.client.SendText(ctx, jid, text)
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, whatsmeow.ErrMessageTimedOut) {
		// whatsmeow writes the frame before it waits for the ack, so the message may have
		// arrived: report the outcome as unknown, never as not sent, which would invite a
		// duplicate resend. Nothing is stored; reconciling it belongs to the outbox (T11).
		log.Print("wa: a send was not acknowledged in time")
		return ipc.Message{}, ipc.ErrNotConfirmed
	}
	if err != nil {
		// The error's type only: whatsmeow's text can carry a JID.
		log.Printf("wa: a send failed (%T)", err)
		return ipc.Message{}, ErrNotSent
	}
	if ts.IsZero() {
		// An ack without a timestamp; the zero time would overflow the store's nanoseconds.
		ts = time.Now()
	}
	m := store.Message{ID: id, ChatJID: jid, Kind: store.KindText, Text: text, FromMe: true, Timestamp: ts.UTC()}
	if s.db != nil {
		if _, err := store.InsertMessage(s.db, m); err != nil {
			log.Printf("wa: cannot store sent message %s: %v", m.ID, err)
		}
	}
	msg := ipc.Message{ID: m.ID, Kind: m.Kind, Text: m.Text, FromMe: true, Timestamp: ipc.Timestamp(m.Timestamp)}
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: jid, Message: msg})
	return msg, nil
}
