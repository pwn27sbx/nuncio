package wa

import (
	"context"
	"strings"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// One conversation per contact (docs/adr/0001-one-chat-per-contact.md). WhatsApp addresses a direct
// contact by phone number (`digits@s.whatsapp.net`) or by hidden id (`digits@lid`), and delivers
// her messages in the LID chat; a send to her phone number is routed by whatsmeow to her LID too
// (send.go:339-362). The session therefore stores and serves one chat per contact: the LID chat
// whenever whatsmeow's LID map knows the pair, the phone-number JID being an alias of it. Without a
// mapping nothing changes and nothing is guessed.

// lidLookupTimeout bounds one lookup in whatsmeow's LID map (a cached SQLite read).
const lidLookupTimeout = 2 * time.Second

// LIDMapper is the slice of whatsmeow's LID map (store.LIDStore: GetLIDForPN, GetPNForLID) the
// session reads. It is optional: a Client without it has no mapping, and every chat stays as named.
type LIDMapper interface {
	// LIDForPN returns the LID chat JID (`digits@lid`) of a phone-number chat JID, or "" when it is
	// not known.
	LIDForPN(ctx context.Context, pn string) (string, error)
	// PNForLID returns the phone-number chat JID of a LID chat JID, or "" when it is not known.
	PNForLID(ctx context.Context, lid string) (string, error)
}

func isPhoneChat(jid string) bool { return strings.HasSuffix(jid, "@s.whatsapp.net") }

func isLIDChat(jid string) bool { return strings.HasSuffix(jid, "@lid") }

// lookup runs f against the client's LID map, if it has one, and answers its JID when it is a chat
// JID of the wanted kind; "" otherwise, a lookup error included (logged by its type only).
func (s *Session) lookup(jid string, f func(LIDMapper, context.Context, string) (string, error), want func(string) bool) string {
	m, ok := s.client.(LIDMapper)
	if !ok {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), lidLookupTimeout)
	defer cancel()
	got, err := f(m, ctx, jid)
	if err != nil {
		s.log("wa: cannot read the LID map (%T)", err)
		return ""
	}
	if !want(got) || !ipc.ValidJID(got) {
		return ""
	}
	return got
}

func (s *Session) lidOf(pn string) string {
	if s.client == nil || !isPhoneChat(pn) {
		return ""
	}
	return s.lookup(pn, LIDMapper.LIDForPN, isLIDChat)
}

func (s *Session) pnOf(lid string) string {
	if s.client == nil || !isLIDChat(lid) {
		return ""
	}
	return s.lookup(lid, LIDMapper.PNForLID, isPhoneChat)
}

// CanonicalJID is the chat a request or an event naming jid acts on: the LID chat of a phone-number
// JID whose LID whatsmeow knows, and jid itself otherwise (a group, a LID chat, an unmapped number).
// When the phone-number chat still holds anything, it is merged into the LID chat first; when that
// merge failed in this session, both chats are kept as they are and jid is its own chat again, so
// what is stored under it stays readable by it.
func (s *Session) CanonicalJID(jid string) string {
	lid := s.lidOf(jid)
	if lid == "" || !s.mergeAlias(jid, lid) {
		return jid
	}
	return lid
}

// log reports through logf, read under mu like every other use of it.
func (s *Session) log(format string, args ...any) {
	s.mu.Lock()
	logf := s.logf
	s.mu.Unlock()
	logf(format, args...)
}

// mergeAliasOf merges into the LID chat lid the chat stored under its phone number, if whatsmeow
// knows that number. It runs for every message stored in a LID chat, since whatsmeow learns a
// mapping from the message itself (message.go:51-55) and emits no event for it.
func (s *Session) mergeAliasOf(lid string) {
	if pn := s.pnOf(lid); pn != "" {
		s.mergeAlias(pn, lid)
	}
}

// mergeKnownAliases merges every stored phone-number chat whose LID whatsmeow knows: the split a
// store kept from before ADR 0001. Start runs it before connecting.
func (s *Session) mergeKnownAliases() {
	pns, err := store.PhoneChats(s.db)
	if err != nil {
		s.log("wa: cannot list the chats to merge (%T)", err)
		return
	}
	for _, pn := range pns {
		if lid := s.lidOf(pn); lid != "" {
			s.mergeAlias(pn, lid)
		}
	}
}

// mergeAlias moves what is stored under the phone-number chat pn into the LID chat lid (one
// transaction, see store.MergeChat) and publishes the merged chat as event_chat. It reports false
// when the merge failed, now or earlier in this session: a failure leaves both chats as they were
// and is logged once per chat for the life of the session, with neither JID in the log.
func (s *Session) mergeAlias(pn, lid string) bool {
	if s.db == nil {
		return true
	}
	s.mu.Lock()
	failed := s.mergeFailed[pn]
	s.mu.Unlock()
	if failed {
		return false
	}
	merged, err := store.MergeChat(s.db, pn, lid)
	if err != nil {
		s.mu.Lock()
		if s.mergeFailed == nil {
			s.mergeFailed = map[string]bool{}
		}
		s.mergeFailed[pn] = true
		s.mu.Unlock()
		// The error's type only: SQLite's text could quote a value.
		s.log("wa: cannot merge a contact's phone-number chat into its LID chat; both are kept as they were (%T)", err)
		return false
	}
	if merged {
		// The merge keeps the LID chat's name; the number's address-book entry may outrank it, so
		// the name is resolved again before the one event_chat of the merge.
		s.storeName(lid)
		s.publishChat(lid)
	}
	return true
}

// ChatAliases is what the wire lists as a chat's `aliases`: the phone-number chats merged into the
// LID chat canonical (chat_aliases) whose LID whatsmeow's map still names as canonical. A number
// whatsmeow remapped (a recycled number) is never listed on the old contact's chat, a number whose
// merge failed in this session is its own chat again, and without a LID map, or with the pair
// unknown, nothing is listed.
func (s *Session) ChatAliases(canonical string) []string {
	if s.db == nil || s.client == nil || !isLIDChat(canonical) {
		return nil
	}
	recorded, err := store.AliasesOf(s.db, canonical)
	if err != nil {
		s.log("wa: cannot read a chat's aliases (%T)", err)
		return nil
	}
	var out []string
	for _, pn := range recorded {
		s.mu.Lock()
		failed := s.mergeFailed[pn]
		s.mu.Unlock()
		if !failed && s.lidOf(pn) == canonical {
			out = append(out, pn)
		}
	}
	return out
}
