package wa

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/rand"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// longText is a text message body of n bytes that names its message (so a search finds it).
func longText(id string, n int) string {
	return id + " " + strings.Repeat("x", n-len(id)-1)
}

// replyFrame is the messages reply for jid as the daemon writes it (without the newline).
func replyFrame(t *testing.T, jid string, msgs []ipc.Message) []byte {
	t.Helper()
	b, err := json.Marshal(ipc.MessagesResponse{V: ipc.Version, Op: ipc.OpMessages, JID: jid, Messages: msgs})
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func sha(b []byte) string { s := sha256.Sum256(b); return hex.EncodeToString(s[:]) }

// fittingReplies are replies that fit in one frame, from small to near the limit, with thumbnails,
// senders, reactions, states and replies, so their bytes pin what WireMessages served before the
// frame fit.
func fittingReplies(t *testing.T) map[string][]byte {
	t.Helper()
	big := jpegOf(MaxThumb)
	bigRef, err := proto.Marshal(thumbImage(big, 640, 480).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	smallRef, err := proto.Marshal(thumbImage(fakeJPEGThumb, 1280, 960).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	stickerRef, err := proto.Marshal(stickerMessage(true).GetStickerMessage())
	if err != nil {
		t.Fatal(err)
	}
	group := omarchy.String()
	names := func(jid string) string { return capSender("name of " + jid + strings.Repeat("é", 80)) }
	out := map[string][]byte{}

	mixed := []store.Message{
		{ID: "T1", ChatJID: group, Kind: store.KindText, Text: "hi <there> & \"you\"", Timestamp: t0, Sender: "15551230001@s.whatsapp.net",
			Reactions: []store.Reaction{{Emoji: "👍", FromMe: true}, {Emoji: "🎉"}}, ReplyTo: "T0"},
		{ID: "I1", ChatJID: group, Kind: store.KindImage, Text: "the venue", Mime: "image/jpeg", Size: 26, Timestamp: t0, MediaRef: smallRef,
			Sender: "15551230002@s.whatsapp.net"},
		{ID: "S1", ChatJID: group, Kind: store.KindSticker, Mime: "image/webp", Size: 33, Timestamp: t0, MediaRef: stickerRef, FromMe: true},
		{ID: "Q1", ChatJID: group, Kind: store.KindText, Text: "queued", Timestamp: t0, FromMe: true, State: store.OutboxQueued, ClientID: "c1"},
		{ID: "N1", ChatJID: group, Kind: store.KindText, Text: "not confirmed", Timestamp: t0, FromMe: true, State: store.OutboxNotConfirmed, ClientID: "c2"},
		{ID: "D1", ChatJID: group, Kind: store.KindDocument, Text: "", Mime: "application/pdf", Size: 9, FileName: "a.pdf", Timestamp: t0},
	}
	out["mixed"] = replyFrame(t, group, WireMessages(group, mixed, names))

	var thumbs []store.Message
	for i := 0; i < 2*ThumbBudget/MaxThumb; i++ {
		thumbs = append(thumbs, store.Message{ID: fmt.Sprintf("IMG%d", i), Kind: store.KindImage, Mime: "image/jpeg", Timestamp: t0, MediaRef: bigRef})
	}
	out["thumbs"] = replyFrame(t, ada.String(), WireMessages(ada.String(), thumbs, nil))

	// Near the limit: long texts take most of the frame, so the room left bounds the thumbnails.
	var near []store.Message
	for i := 0; i < 6; i++ {
		near = append(near, store.Message{ID: fmt.Sprintf("NI%d", i), Kind: store.KindImage, Mime: "image/jpeg", Timestamp: t0, MediaRef: bigRef})
	}
	for i := 0; i < 150; i++ {
		near = append(near, store.Message{ID: fmt.Sprintf("NT%d", i), Kind: store.KindText, Text: longText(fmt.Sprintf("NT%d", i), 5250), Timestamp: t0})
	}
	out["near"] = replyFrame(t, ada.String(), WireMessages(ada.String(), near, nil))

	// Just under the limit without thumbnails: 190 texts of 5250 bytes.
	var under []store.Message
	for i := 0; i < 190; i++ {
		under = append(under, store.Message{ID: fmt.Sprintf("U%03d", i), Kind: store.KindText, Text: longText(fmt.Sprintf("U%03d", i), 5250), Timestamp: t0})
	}
	out["under"] = replyFrame(t, ada.String(), WireMessages(ada.String(), under, nil))
	out["empty"] = replyFrame(t, ada.String(), WireMessages(ada.String(), nil, nil))
	return out
}

// Replies that fit in one frame are served byte for byte as before the frame fit existed (sha256 of
// each frame as WireMessages served it at e6b7d70).
func TestRepliesThatFitAreByteIdenticalToBefore(t *testing.T) {
	want := map[string]string{
		"mixed":  "bc7f164857bc7c585711d50f57a18473268f1038b503b28ae34e41b2b92da963",
		"thumbs": "e1b117b31066118b959513bf2797db2d6760dff8bd8d5037b635aff39ffe1989",
		"near":   "a14a6513ae666fd3ae4a8872ea79cbe318ef35f34bdf5d4e93e08169d1ed76c2",
		"under":  "0d23edfe879633b00b9cbe63cce221ff4f2d405dc94bec4f8cd9e982a682ac39",
		"empty":  "f5b351de63e7af28eadaf0efad6be30deeadd814f4c82313c73486920260f4b3",
	}
	for name, frame := range fittingReplies(t) {
		if len(frame) > ipc.MaxFrameLen {
			t.Errorf("%s: %d bytes, over the frame limit; it should fit", name, len(frame))
		}
		if got := sha(frame); got != want[name] {
			t.Errorf("%s: sha256 %s (%d bytes), want %s", name, got, len(frame), want[name])
		}
	}
}

// chatsFrame is the chats reply for every chat of a store holding a text, an image with its thumbnail,
// a sticker, a reacted message and a queued send, as the daemon builds it with WireChat.
func chatsFrame(t *testing.T) []byte {
	t.Helper()
	return chatsOf(t, chatsStore(t))
}

// chatsStore is the store chatsFrame serves.
func chatsStore(t *testing.T) *sql.DB {
	t.Helper()
	db := openTestStore(t)
	imageRef, err := proto.Marshal(thumbImage(fakeJPEGThumb, 1280, 960).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	stickerRef, err := proto.Marshal(stickerMessage(false).GetStickerMessage())
	if err != nil {
		t.Fatal(err)
	}
	for _, m := range []store.Message{
		{ID: "A1", ChatJID: "15551230001@s.whatsapp.net", Kind: store.KindText, Text: "hello <you>", Timestamp: t0},
		{ID: "B1", ChatJID: "15551230002@s.whatsapp.net", Kind: store.KindImage, Text: "the venue", Mime: "image/jpeg", Size: 26,
			Timestamp: t0.Add(time.Minute), MediaRef: imageRef},
		{ID: "C1", ChatJID: "15551230003@s.whatsapp.net", Kind: store.KindSticker, Mime: "image/webp", Size: 33,
			Timestamp: t0.Add(2 * time.Minute), MediaRef: stickerRef, FromMe: true},
		{ID: "D1", ChatJID: omarchy.String(), Kind: store.KindText, Text: "group hi", Timestamp: t0.Add(3 * time.Minute),
			Sender: "15551230001@s.whatsapp.net"},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.SetReaction(db, omarchy.String(), "D1", "15551230002@s.whatsapp.net", false, "🎉"); err != nil {
		t.Fatal(err)
	}
	if _, _, err := store.Enqueue(db, store.OutboxEntry{ClientID: "c1", ChatJID: "15551230004@s.whatsapp.net", Text: "later",
		MessageID: "E1", Created: t0.Add(4 * time.Minute)}); err != nil {
		t.Fatal(err)
	}
	return db
}

// chatsOf is the chats reply for every chat of db, built with WireChat.
func chatsOf(t *testing.T, db *sql.DB) []byte {
	t.Helper()
	rows, err := store.ListChats(db, ipc.MaxChatLimit)
	if err != nil {
		t.Fatal(err)
	}
	var chats []ipc.Chat
	for _, r := range rows {
		c, err := WireChat(db, r, t0)
		if err != nil {
			t.Fatal(err)
		}
		chats = append(chats, c)
	}
	b, err := json.Marshal(ipc.ChatsResponse{V: ipc.Version, Op: ipc.OpChats, Chats: chats})
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// The chats reply is byte for byte what it was before WireChat stopped reading the last message's
// media reference (at e6b7d70).
func TestChatsAreByteIdenticalToBefore(t *testing.T) {
	const want = `{"v":1,"op":"chats","chats":[{"jid":"120363000000000000@g.us","name":"","unread":1,"archived":false,"pinned":false,"muted":false,"last_message":{"id":"D1","kind":"text","text":"group hi","from_me":false,"timestamp":"2026-10-18T17:58:04Z","reactions":[{"emoji":"🎉","from_me":false}]}},{"jid":"15551230003@s.whatsapp.net","name":"","unread":0,"archived":false,"pinned":false,"muted":false,"last_message":{"id":"C1","kind":"sticker","text":"","from_me":true,"timestamp":"2026-10-18T17:57:04Z","media":{"mime":"image/webp","size":33,"file_name":""}}},{"jid":"15551230002@s.whatsapp.net","name":"","unread":1,"archived":false,"pinned":false,"muted":false,"last_message":{"id":"B1","kind":"image","text":"the venue","from_me":false,"timestamp":"2026-10-18T17:56:04Z","media":{"mime":"image/jpeg","size":26,"file_name":""}}},{"jid":"15551230001@s.whatsapp.net","name":"","unread":1,"archived":false,"pinned":false,"muted":false,"last_message":{"id":"A1","kind":"text","text":"hello \u003cyou\u003e","from_me":false,"timestamp":"2026-10-18T17:55:04Z"}},{"jid":"15551230004@s.whatsapp.net","name":"","unread":0,"archived":false,"pinned":false,"muted":false,"last_message":{"id":"E1","kind":"text","text":"later","from_me":true,"timestamp":"2026-10-18T17:59:04Z","state":"queued","client_id":"c1"}}]}`
	if got := chatsFrame(t); string(got) != want {
		t.Errorf("chats =\n%s\nwant\n%s", got, want)
	}
}

// WireChat serves a chat's last message without reading its media reference: the chat list carries
// no preview, so the reference is never read. Structural check: with the media_refs table gone the
// chats reply is still served, byte for byte the same.
func TestWireChatDoesNotReadTheLastMessagesMediaReference(t *testing.T) {
	db := chatsStore(t)
	if _, err := db.Exec(`DROP TABLE media_refs`); err != nil {
		t.Fatal(err)
	}
	if got := chatsOf(t, db); string(got) != string(chatsFrame(t)) {
		t.Errorf("chats without media_refs =\n%s\nwant\n%s", got, chatsFrame(t))
	}
}

// fitLimit is the most a messages reply measures before its thumbnails (WireMessages).
const fitLimit = ipc.MaxFrameLen - frameMargin

// A messages or search reply whose texts alone do not fit in one frame keeps its newest messages,
// as many as fit, and leaves the oldest out; every message it keeps is whole.
func TestAReplyTooLongForOneFrameKeepsItsNewestMessages(t *testing.T) {
	var rows []store.Message
	for i := 0; i < ipc.MaxChatLimit; i++ {
		id := fmt.Sprintf("M%03d", i)
		rows = append(rows, store.Message{ID: id, Kind: store.KindText, Text: longText(id, 5250), Timestamp: t0})
	}
	msgs := WireMessages(ada.String(), rows, nil)
	frame := replyFrame(t, ada.String(), msgs)
	if len(frame) > fitLimit {
		t.Fatalf("reply of %d messages is %d bytes, want at most %d", len(msgs), len(frame), fitLimit)
	}
	if len(msgs) == 0 || len(msgs) == len(rows) {
		t.Fatalf("reply keeps %d of %d messages, want the newest that fit", len(msgs), len(rows))
	}
	for i, m := range msgs {
		if m.ID != rows[i].ID || m.Text != rows[i].Text {
			t.Fatalf("message %d = %s (%d bytes of text), want %s whole", i, m.ID, len(m.Text), rows[i].ID)
		}
	}
	// As many as fit: one more message would not.
	more := replyFrame(t, ada.String(), append(msgs, WireMessage(rows[len(msgs)])))
	if len(more) <= fitLimit {
		t.Errorf("%d messages would fit in %d bytes, but the reply keeps %d", len(msgs)+1, len(more), len(msgs))
	}
}

// One message whose text alone does not fit in a frame is served with its text cut, on a code point
// boundary (never inside a UTF-8 sequence), followed by "…", so the reply fits; older messages that
// no longer fit are left out.
func TestAMessageTooLongForOneFrameIsCutOnACodePointBoundary(t *testing.T) {
	// 1.2 MiB of text mixing 1-, 2-, 3- and 4-byte code points and characters JSON escapes (<, &).
	huge := strings.Repeat("a<é€🎉&", (12<<20)/10/12+1) // 12 bytes, repeated past 1.2 MiB
	for name, rows := range map[string][]store.Message{
		"alone":        {{ID: "HUGE", Kind: store.KindText, Text: huge, Timestamp: t0}},
		"newest":       {{ID: "HUGE", Kind: store.KindText, Text: huge, Timestamp: t0}, {ID: "OLD", Kind: store.KindText, Text: "old", Timestamp: t0}},
		"behind a new": {{ID: "NEW", Kind: store.KindText, Text: "new", Timestamp: t0}, {ID: "HUGE", Kind: store.KindText, Text: huge, Timestamp: t0}},
	} {
		msgs := WireMessages(ada.String(), rows, nil)
		frame := replyFrame(t, ada.String(), msgs)
		if len(frame) > fitLimit {
			t.Errorf("%s: reply is %d bytes, want at most %d", name, len(frame), fitLimit)
			continue
		}
		if name == "behind a new" {
			if len(msgs) != 1 || msgs[0].ID != "NEW" || msgs[0].Text != "new" {
				t.Errorf("%s: reply = %d messages, want only NEW, whole", name, len(msgs))
			}
			continue
		}
		if len(msgs) != 1 || msgs[0].ID != "HUGE" {
			t.Fatalf("%s: reply = %d messages, want only HUGE", name, len(msgs))
		}
		text := msgs[0].Text
		cut, ok := strings.CutSuffix(text, "…")
		if !ok || !utf8.ValidString(text) || !strings.HasPrefix(huge, cut) {
			t.Fatalf("%s: text (%d bytes) is not a valid prefix of the original followed by …", name, len(text))
		}
		if _, size := utf8.DecodeRuneInString(huge[len(cut):]); size == 0 {
			t.Fatalf("%s: text was not cut", name)
		}
		// The cut keeps as much as fits: one more code point would not.
		_, size := utf8.DecodeRuneInString(huge[len(cut):])
		longer := msgs[0]
		longer.Text = huge[:len(cut)+size] + "…"
		if n := len(replyFrame(t, ada.String(), []ipc.Message{longer})); n <= fitLimit {
			t.Errorf("%s: a cut one code point longer (%d bytes) would fit too", name, n)
		}
	}
}

// The fit counts every byte of the reply, the commas between its messages included: 200 messages
// that would fit but for those commas are not all kept.
func TestTheFitCountsTheCommasBetweenMessages(t *testing.T) {
	rowsOf := func(n, first int) []store.Message {
		var rows []store.Message
		for i := 0; i < ipc.MaxChatLimit; i++ {
			id := fmt.Sprintf("C%03d", i)
			size := n
			if i == 0 {
				size += first
			}
			rows = append(rows, store.Message{ID: id, Kind: store.KindText, Text: longText(id, size), Timestamp: t0})
		}
		return rows
	}
	unfitted := func(rows []store.Message) int {
		var wire []ipc.Message
		for _, r := range rows {
			wire = append(wire, WireMessage(r))
		}
		return len(replyFrame(t, ada.String(), wire))
	}
	// Size the texts so that the 200 messages without their 199 commas take the limit less 100 bytes:
	// with the commas they are 99 bytes over it.
	commas := ipc.MaxChatLimit - 1
	base := unfitted(rowsOf(1000, 0)) - commas
	grow := (fitLimit - 100 - base) / ipc.MaxChatLimit
	rows := rowsOf(1000+grow, fitLimit-100-base-grow*ipc.MaxChatLimit)
	if n := unfitted(rows); n != fitLimit+99 {
		t.Fatalf("the 200 messages take %d bytes, want %d", n, fitLimit+99)
	}
	msgs := WireMessages(ada.String(), rows, nil)
	if n := len(replyFrame(t, ada.String(), msgs)); n > fitLimit || len(msgs) != ipc.MaxChatLimit-1 {
		t.Errorf("reply keeps %d messages in %d bytes, want %d in at most %d", len(msgs), n, ipc.MaxChatLimit-1, fitLimit)
	}
}

// A trimmed reply whose left-out (older) messages carry thumbnails is served: only the messages
// kept get thumbnails, and nothing panics.
func TestATrimmedReplyWhoseDroppedMessageHasAThumbnailIsServed(t *testing.T) {
	ref, err := proto.Marshal(thumbImage(fakeJPEGThumb, 1280, 960).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	var rows []store.Message
	for i := 0; i < 199; i++ {
		id := fmt.Sprintf("M%03d", i)
		rows = append(rows, store.Message{ID: id, Kind: store.KindText, Text: longText(id, 5300), Timestamp: t0})
	}
	rows = append(rows, store.Message{ID: "IMG", Kind: store.KindImage, Mime: "image/jpeg", Size: 26, Timestamp: t0, MediaRef: ref})
	msgs := WireMessages(ada.String(), rows, nil)
	if len(msgs) == 0 || len(msgs) == len(rows) || len(replyFrame(t, ada.String(), msgs)) > ipc.MaxFrameLen {
		t.Fatalf("kept %d of %d", len(msgs), len(rows))
	}
	for _, m := range msgs {
		if m.ID == "IMG" {
			t.Fatal("the dropped image was served")
		}
	}
}

// A newest message whose file name or caption alone is larger than a frame is served with them cut
// on a code point boundary, and the older messages, thumbnails included, are left out: no panic,
// and the reply fits.
func TestANewestMessageWithAHugeFileNameIsServedCut(t *testing.T) {
	ref, err := proto.Marshal(thumbImage(fakeJPEGThumb, 1280, 960).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	hugeName := strings.Repeat("é<", (12<<20)/10/3+1) + ".pdf"
	hugeCaption := strings.Repeat("€", (12<<20)/10/3+1)
	rows := []store.Message{{ID: "DOC", Kind: store.KindDocument, Mime: "application/pdf", Size: 9, FileName: hugeName,
		Text: hugeCaption, Timestamp: t0}}
	for i := 0; i < 5; i++ {
		rows = append(rows, store.Message{ID: fmt.Sprintf("IMG%d", i), Kind: store.KindImage, Mime: "image/jpeg", Size: 26,
			Timestamp: t0, MediaRef: ref})
	}
	msgs := WireMessages(ada.String(), rows, nil)
	if n := len(replyFrame(t, ada.String(), msgs)); n > ipc.MaxFrameLen {
		t.Fatalf("reply is %d bytes, over the frame limit", n)
	}
	if len(msgs) != 1 || msgs[0].ID != "DOC" || msgs[0].Media == nil {
		t.Fatalf("reply = %d messages, want only DOC", len(msgs))
	}
	name, caption := msgs[0].Media.FileName, msgs[0].Text
	for what, got := range map[string][2]string{"file name": {name, hugeName}, "caption": {caption, hugeCaption}} {
		cut, ok := strings.CutSuffix(got[0], "…")
		if !utf8.ValidString(got[0]) || (ok && !strings.HasPrefix(got[1], cut)) || (!ok && got[0] != got[1]) {
			t.Errorf("%s (%d bytes) is not the original or a valid prefix of it followed by …", what, len(got[0]))
		}
	}
	if !strings.HasSuffix(name, "…") {
		t.Errorf("file name (%d bytes) was not cut", len(name))
	}
}

// Property: whatever mix of thumbnails, long texts and counts a reply holds, WireMessages never
// panics, never writes over the frame limit, and keeps a newest prefix of the rows.
func TestAnyReplyFitsInOneFrame(t *testing.T) {
	small, err := proto.Marshal(thumbImage(fakeJPEGThumb, 1280, 960).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	big, err := proto.Marshal(thumbImage(jpegOf(MaxThumb), 640, 480).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	for seed := int64(1); seed <= 40; seed++ {
		r := rand.New(rand.NewSource(seed))
		var rows []store.Message
		for i, n := 0, 1+r.Intn(ipc.MaxChatLimit); i < n; i++ {
			id := fmt.Sprintf("R%03d", i)
			m := store.Message{ID: id, Kind: store.KindText, Timestamp: t0}
			switch r.Intn(6) {
			case 0:
				m.Kind, m.Mime, m.MediaRef = store.KindImage, "image/jpeg", small
			case 1:
				m.Kind, m.Mime, m.MediaRef = store.KindImage, "image/jpeg", big
			case 2:
				m.Text = longText(id, 1+len(id)+r.Intn(1<<20))
			default:
				m.Text = longText(id, 1+len(id)+r.Intn(12000))
			}
			rows = append(rows, m)
		}
		msgs := WireMessages(ada.String(), rows, nil)
		if n := len(replyFrame(t, ada.String(), msgs)); n > ipc.MaxFrameLen {
			t.Errorf("seed %d: reply of %d messages is %d bytes", seed, len(msgs), n)
		}
		for i, m := range msgs {
			if m.ID != rows[i].ID {
				t.Errorf("seed %d: message %d is %s, want %s (a newest prefix)", seed, i, m.ID, rows[i].ID)
				break
			}
		}
	}
}
