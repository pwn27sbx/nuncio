package wa

import (
	"context"
	"errors"
	"fmt"
	"log"
	"slices"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// markReadTimeout bounds the whole mark_read (the privacy read and every receipt), so the answer
// reaches a client inside its ordinary 5 s read bound.
const markReadTimeout = 4 * time.Second

// maxReceiptIDs caps the message ids one read receipt carries. A receipt naming hundreds of ids is
// not what the official clients send after a long absence, and an unusual traffic shape is a ban
// risk, so a longer backlog goes out as consecutive receipts of at most this many ids.
const maxReceiptIDs = 50

var (
	// ErrMarkReadOffline: read receipts are never queued, so a chat opened while offline stays
	// unread and the next view marks it read.
	ErrMarkReadOffline = errors.New("the WhatsApp session is offline, so the chat was not marked read")
	// errReadPrivacy: without the account's read-receipt setting a receipt could show blue ticks the
	// user turned off, so nothing is sent.
	errReadPrivacy = errors.New("could not read the read-receipt privacy setting, so the chat was not marked read")
	// errMarkReadBusy: another mark_read was still running when this one's 4 s bound ended, so
	// nothing was sent and the store is unchanged.
	errMarkReadBusy = errors.New("another mark_read is still running, so the chat was not marked read")
	// errReceiptNotSent: WhatsApp did not take a read receipt.
	errReceiptNotSent = errors.New("WhatsApp did not accept the read receipt, so the chat was not marked read")
)

// MarkRead marks chat jid read because a UI opened it: this is the explicit view after which read
// receipts go out (AGENTS.md), and nothing else sends one. It sends WhatsApp one read receipt per
// sender for the chat's unread incoming messages, as the official clients do, then marks those
// messages read in the store, publishes event_unread and returns the new total unread count.
//
// One receipt carries at most 50 ids (maxReceiptIDs): a sender with more unread messages gets
// consecutive receipts of at most 50 ids each, oldest first, before the next sender's. They are
// still one operation: the first receipt WhatsApp does not take ends it, nothing after it is sent,
// nothing is retried, and the store changes only after every receipt went out; the 4 s bound
// (markReadTimeout) covers the wait for another mark_read, the privacy read and all the receipts
// together.
//
// The receipt type follows the account's read-receipt privacy: with read receipts off it is
// read-self, so the user's other devices mark the chat read but the sender sees no blue ticks. A
// group message stored before senders were recorded (M1 data) has no participant to address, so it
// gets no receipt, but it is still marked read here.
//
// Calls run one at a time: of two UIs opening the same chat at once, the second waits for the
// first and then finds nothing unread, so the receipts go out once. The 4 s bound starts when the
// call does, waiting included: one still waiting when it ends answers errMarkReadBusy, having sent
// and changed nothing.
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
	// The bound starts now, so a call waiting behind another still answers within it.
	ctx, cancel := context.WithTimeout(context.Background(), markReadTimeout)
	defer cancel()
	select {
	case s.markReadSem <- struct{}{}:
		defer func() { <-s.markReadSem }()
	case <-ctx.Done():
		return 0, errMarkReadBusy
	}
	jid = s.CanonicalJID(jid)
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

	// The receipts of one sender after another, in the order of each sender's oldest unread message.
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
			for chunk := range slices.Chunk(bySender[sender], maxReceiptIDs) {
				if err := s.client.MarkRead(ctx, jid, sender, chunk, now, receipt); err != nil {
					// The error's type only: whatsmeow's text can carry a JID.
					log.Printf("wa: a read receipt failed (%T)", err)
					return 0, errReceiptNotSent
				}
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
