package wa

import (
	"unicode/utf8"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// Group senders (proto/wire.md, Message `sender`). An incoming message in a group chat names its
// author by the chat-name priority of names.go: the address-book name (full, else first), else the
// business name, else the push name, a LID read through her phone number too (whatsmeow's LID map);
// else her number masked by ipc.MaskPhone; else unknownSender. Every read is local (the contact
// store and the LID map), never the network, and each sender is read once per reply or event. A
// direct chat, a message of mine and one stored before senders were recorded carry none. Names and
// numbers never reach a log: a failed read is logged by its error type (contactName). A name is at
// most maxSenderRunes runes (capSender), so senders cannot push a reply past ipc.MaxFrameLen.

// unknownSender is the sender of a group message whose author has neither a name nor a known number.
const unknownSender = "unknown"

// maxSenderRunes is the most runes a sender text holds, its "…" included.
const maxSenderRunes = 64

// capSender is name when it has at most maxSenderRunes runes, else its first maxSenderRunes-1 runes
// followed by "…": the cut falls on a rune boundary, never inside a UTF-8 sequence.
func capSender(name string) string {
	if utf8.RuneCountInString(name) <= maxSenderRunes {
		return name
	}
	cut, n := 0, 0
	for i := range name {
		if n == maxSenderRunes-1 {
			cut = i
			break
		}
		n++
	}
	return name[:cut] + "…"
}

// Senders is what names the authors of the incoming group messages of one reply or event: it
// resolves each sender JID once and keeps the answer for the next message of the same sender.
func (s *Session) Senders() func(sender string) string {
	cache := map[string]string{}
	return func(sender string) string {
		if name, ok := cache[sender]; ok {
			return name
		}
		name := s.senderName(sender)
		cache[sender] = name
		return name
	}
}

// senderName is the sender text of the user JID sender (see above).
func (s *Session) senderName(sender string) string {
	if book, ok := s.contacts(); ok {
		if name, ok := s.contactName(book, sender); ok && name != "" {
			return capSender(name)
		}
	}
	if phone := s.ChatPhone(sender); phone != "" {
		return phone
	}
	return unknownSender
}

// wireSender is the `sender` of message m: set only for an incoming group message whose sender was
// recorded, and only when senders is given.
func wireSender(m store.Message, senders func(string) string) string {
	if senders == nil || m.FromMe || m.Sender == "" || !isGroupChat(m.ChatJID) {
		return ""
	}
	return senders(m.Sender)
}
