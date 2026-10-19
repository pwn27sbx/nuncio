package wa

import (
	"context"
	"errors"
	"fmt"
	"log"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// markReadTimeout bounds the whole mark_read (the privacy read and every receipt), so the answer
// reaches a client inside its ordinary 5 s read bound.
const markReadTimeout = 4 * time.Second

var (
	// ErrMarkReadOffline: read receipts are never queued, so a chat opened while offline stays
	// unread and the next view marks it read.
	ErrMarkReadOffline = errors.New("the WhatsApp session is offline, so the chat was not marked read")
	// errReadPrivacy: without the account's read-receipt setting a receipt could show blue ticks the
	// user turned off, so nothing is sent.
	errReadPrivacy = errors.New("could not read the read-receipt privacy setting, so the chat was not marked read")
	// errReceiptNotSent: WhatsApp did not take a read receipt.
	errReceiptNotSent = errors.New("WhatsApp did not accept the read receipt, so the chat was not marked read")
)

// MarkRead marks chat jid read because a UI opened it: this is the explicit view after which read
// receipts go out (AGENTS.md), and nothing else sends one. It sends WhatsApp one read receipt per
// sender for the chat's unread incoming messages, as the official clients do, then marks those
// messages read in the store, publishes event_unread and returns the new total unread count.
//
// The receipt type follows the account's read-receipt privacy: with read receipts off it is
// read-self, so the user's other devices mark the chat read but the sender sees no blue ticks. A
// group message stored before senders were recorded (M1 data) has no participant to address, so it
// gets no receipt, but it is still marked read here.
//
// It is one attempt, only while linked, and is never queued or retried: unlinked answers
// ipc.ErrNotLinked, offline ErrMarkReadOffline, and an unreadable privacy setting or a receipt
// WhatsApp did not take an error; in every such case the store is left exactly as it was. A chat
// with nothing unread sends nothing and answers the total.
func (s *Session) MarkRead(jid string) (int, error) {
	if s.client == nil || s.State() == StateUnlinked {
		return 0, ipc.ErrNotLinked
	}
	if s.db == nil {
		return 0, errNoStore
	}
	unread, err := store.UnreadIncoming(s.db, jid)
	if err != nil {
		return 0, fmt.Errorf("wa: %w", err)
	}
	if len(unread) == 0 {
		return s.totalUnread()
	}
	if s.State() != StateLinked {
		return 0, ErrMarkReadOffline
	}

	// One receipt per sender, in the order of each sender's oldest unread message.
	var senders []string
	bySender := map[string][]string{}
	ids := make([]string, 0, len(unread))
	for _, m := range unread {
		ids = append(ids, m.ID)
		sender := s.senderOf(m)
		if sender == "" {
			continue // a group message with no recorded sender: marked read here only
		}
		if _, seen := bySender[sender]; !seen {
			senders = append(senders, sender)
		}
		bySender[sender] = append(bySender[sender], m.ID)
	}

	if len(senders) > 0 {
		ctx, cancel := context.WithTimeout(context.Background(), markReadTimeout)
		defer cancel()
		privacy, err := s.client.ReadReceiptsPrivacy(ctx)
		if err != nil {
			log.Printf("wa: cannot read the read-receipt privacy setting (%T)", err)
			return 0, errReadPrivacy
		}
		receipt := string(types.ReceiptTypeRead)
		if privacy == string(types.PrivacySettingNone) {
			receipt = string(types.ReceiptTypeReadSelf)
		}
		now := time.Now()
		for _, sender := range senders {
			if err := s.client.MarkRead(ctx, jid, sender, bySender[sender], now, receipt); err != nil {
				// The error's type only: whatsmeow's text can carry a JID.
				log.Printf("wa: a read receipt failed (%T)", err)
				return 0, errReceiptNotSent
			}
		}
	}

	if err := store.MarkMessagesRead(s.db, jid, ids); err != nil {
		return 0, fmt.Errorf("wa: %w", err)
	}
	s.publishUnread(jid)
	return s.totalUnread()
}

// totalUnread is the store's total unread count.
func (s *Session) totalUnread() (int, error) {
	n, err := store.TotalUnread(s.db)
	if err != nil {
		return 0, fmt.Errorf("wa: %w", err)
	}
	return n, nil
}
