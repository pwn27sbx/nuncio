package wa

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"log"

	"go.mau.fi/whatsmeow/proto/waE2E"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// messageRow turns a whatsmeow message into a store row: text and extended text as text, and the
// image, video, audio, document and sticker kinds with their caption as text, their mime type, size and file
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
// store's outbox states are the wire's, except not_confirmed, which the wire calls not_sent). It
// carries no `sender`: wireMessage adds it.
func WireMessage(m store.Message) ipc.Message { return wireMessage(m, nil) }

// wireMessage is WireMessage naming the author of an incoming group message with senders (see
// senders.go); a nil senders names none.
func wireMessage(m store.Message, senders func(string) string) ipc.Message {
	w := ipc.Message{ID: m.ID, Kind: m.Kind, Text: m.Text, FromMe: m.FromMe, Timestamp: ipc.Timestamp(m.Timestamp),
		State: m.State, ClientID: m.ClientID, ReplyTo: m.ReplyTo, Sender: wireSender(m, senders)}
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
	setPreview(&w, m)
	return w
}

const (
	// MaxThumb is the largest embedded thumbnail served, in bytes before base64; a larger one is
	// left out (WhatsApp's own are a few KiB).
	MaxThumb = 64 << 10
	// ThumbBudget bounds the thumbnails of one messages or search reply, in bytes before base64;
	// the room left in the frame bounds them too (WireMessages).
	ThumbBudget = 384 << 10
	// MaxSide is the largest width or height served; a size outside 1..MaxSide is left out, so a
	// client can size a box from it safely.
	MaxSide = 16384
	// frameMargin is room a reply frame keeps free beyond what WireMessages measures.
	frameMargin = 4 << 10
)

var (
	jpegMagic = []byte{0xff, 0xd8, 0xff}
	pngMagic  = []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}
	// thumbKeys is what a thumb adds to an encoded message besides its base64 (both thumb mime
	// types have the same length).
	thumbKeys = len(`,"thumb":"","thumb_mime":"image/jpeg"`)
)

// setPreview adds to w what an image or sticker message embeds of itself, read from its stored
// reference (no network): the thumbnail (up to MaxThumb, and only when its bytes are the JPEG or
// PNG its label says), its mime type, the size in pixels (each within 1..MaxSide) and, for a
// sticker, whether it is animated. Any other kind, a message without a reference or one whose
// reference is unreadable gets none, so it encodes as before.
func setPreview(w *ipc.Message, m store.Message) {
	if len(m.MediaRef) == 0 || (m.Kind != store.KindImage && m.Kind != store.KindSticker) {
		return
	}
	sub, err := decodeRef(m.Kind, m.MediaRef)
	if err != nil {
		return
	}
	var thumb, magic []byte
	var width, height uint32
	switch v := sub.(type) {
	case *waE2E.ImageMessage:
		thumb, width, height, magic = v.GetJPEGThumbnail(), v.GetWidth(), v.GetHeight(), jpegMagic
		w.ThumbMime = "image/jpeg"
	case *waE2E.StickerMessage:
		thumb, width, height, magic = v.GetPngThumbnail(), v.GetWidth(), v.GetHeight(), pngMagic
		w.ThumbMime, w.Animated = "image/png", v.GetIsAnimated()
	}
	if len(thumb) > MaxThumb || !bytes.HasPrefix(thumb, magic) {
		w.ThumbMime = ""
	} else {
		w.Thumb = thumb
	}
	w.Width, w.Height = side(width), side(height)
}

// side is a stated width or height when it is within 1..MaxSide, else 0 (left out).
func side(n uint32) int {
	if n > MaxSide {
		return 0
	}
	return int(n)
}

// WireMessages is wireMessage for the rows of the messages or search reply for chat jid, newest
// first, each incoming group message named by senders (nil names none). The reply never exceeds
// ipc.MaxFrameLen because of thumbnails: it is measured without them (its senders included), and
// they are added back newest first while each fits in the room left in the frame (less
// frameMargin) and in ThumbBudget. One that does not fit is left out (its width and height stay).
func WireMessages(jid string, rows []store.Message, senders func(string) string) []ipc.Message {
	msgs := make([]ipc.Message, len(rows))
	thumbs := make([]ipc.Message, len(rows))
	for i, r := range rows {
		msgs[i] = wireMessage(r, senders)
		thumbs[i] = ipc.Message{Thumb: msgs[i].Thumb, ThumbMime: msgs[i].ThumbMime}
		msgs[i].Thumb, msgs[i].ThumbMime = nil, ""
	}
	bare, err := json.Marshal(ipc.MessagesResponse{V: ipc.Version, Op: ipc.OpMessages, JID: jid, Messages: msgs})
	if err != nil {
		return msgs
	}
	room, budget := ipc.MaxFrameLen-frameMargin-len(bare), ThumbBudget
	for i, t := range thumbs {
		cost := base64.StdEncoding.EncodedLen(len(t.Thumb)) + thumbKeys
		if len(t.Thumb) == 0 || cost > room || len(t.Thumb) > budget {
			continue
		}
		msgs[i].Thumb, msgs[i].ThumbMime = t.Thumb, t.ThumbMime
		room, budget = room-cost, budget-len(t.Thumb)
	}
	return msgs
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
	// One chat per contact: a message addressed by phone number goes to her LID chat when whatsmeow
	// knows it, and a message in a LID chat brings along what is still stored under her number.
	m.ChatJID = s.CanonicalJID(m.ChatJID)
	if m.Sender == evt.Info.Chat.ToNonAD().String() && m.Sender != m.ChatJID && !evt.Info.IsGroup {
		m.Sender = m.ChatJID
	}
	s.mergeAliasOf(m.ChatJID)
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
	s.publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: m.ChatJID, Message: wireMessage(m, s.Senders())})
	if !m.FromMe {
		s.publishUnread(m.ChatJID)
	}
	// A chat its first message created is named at once (names.go): a local read, never the network.
	s.nameNewChat(m.ChatJID)
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
	jid = s.CanonicalJID(jid)
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
