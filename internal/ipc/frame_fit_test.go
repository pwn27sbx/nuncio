package ipc

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"unicode/utf8"
)

func sha(b []byte) string { s := sha256.Sum256(b); return hex.EncodeToString(s[:]) }

// longChat is a chat whose last message is a text of n bytes that names the chat.
func longChat(i, n int) Chat {
	id := fmt.Sprintf("chat%03d", i)
	return Chat{JID: fmt.Sprintf("1555123%04d@s.whatsapp.net", i), Name: "Name " + id, Unread: i % 3,
		LastMessage: &Message{ID: id, Kind: KindText, Text: id + " " + strings.Repeat("x", n-len(id)-1),
			Timestamp: "2026-10-18T17:58:00Z"}}
}

// chatsReply asks a server whose handler lists chats for limit chats and returns the reply frame,
// then checks that the same connection still answers a status request.
func chatsReply(t *testing.T, chats []Chat, limit int) []byte {
	t.Helper()
	h := newFakeHandler(t)
	h.chats = chats
	conn, br := rawConn(t, startServer(t, h))
	handshake(t, conn, br)
	sendRaw(t, conn, fmt.Sprintf(`{"v":1,"op":"chats","limit":%d}`, limit))
	reply := recvRaw(t, br)
	sendRaw(t, conn, `{"v":1,"op":"status"}`)
	if got := fieldOf(t, recvRaw(t, br), "op"); got != OpStatus {
		t.Fatalf("after the chats reply the connection answered %v, want a status frame", got)
	}
	return reply
}

// A chats reply whose chats do not fit in one frame keeps the connection: it holds the first chats
// of the list (the newest), each whole, as many as fit.
func TestAChatsReplyTooLongForOneFrameKeepsItsNewestChats(t *testing.T) {
	var chats []Chat
	for i := 0; i < MaxChatLimit; i++ {
		chats = append(chats, longChat(i, 6000))
	}
	want, err := json.Marshal(chats)
	if err != nil {
		t.Fatal(err)
	}
	reply := chatsReply(t, chats, MaxChatLimit)
	if len(reply) > MaxFrameLen {
		t.Fatalf("chats reply is %d bytes, over the %d byte frame limit", len(reply), MaxFrameLen)
	}
	var got ChatsResponse
	if err := json.Unmarshal(reply, &got); err != nil || got.Op != OpChats {
		t.Fatalf("chats reply = %.80s (%v), want a chats frame", reply, err)
	}
	n := len(got.Chats)
	if n == 0 || n == MaxChatLimit {
		t.Fatalf("reply keeps %d chats, want the newest that fit", n)
	}
	kept, err := json.Marshal(got.Chats)
	if err != nil {
		t.Fatal(err)
	}
	var whole []Chat
	if err := json.Unmarshal(want, &whole); err != nil {
		t.Fatal(err)
	}
	prefix, err := json.Marshal(whole[:n])
	if err != nil {
		t.Fatal(err)
	}
	if string(kept) != string(prefix) {
		t.Errorf("kept chats are not the first %d chats, whole", n)
	}
	// As many as fit: one more chat would not.
	more, err := json.Marshal(ChatsResponse{V: Version, Op: OpChats, Chats: whole[:n+1]})
	if err != nil {
		t.Fatal(err)
	}
	if len(more) <= MaxFrameLen {
		t.Errorf("reply keeps %d chats, but %d fit (%d bytes)", n, n+1, len(more))
	}
	// The limit still applies before the fit.
	var limited ChatsResponse
	if err := json.Unmarshal(chatsReply(t, chats, 3), &limited); err != nil || len(limited.Chats) != 3 {
		t.Errorf("limit 3 served %d chats (%v), want 3", len(limited.Chats), err)
	}
}

// A newest chat that alone does not fit in one frame is served cut: its preview text, then its
// preview's file name, then its name, each on a code point boundary and ending in "…".
func TestAChatTooLongForOneFrameIsCutOnACodePointBoundary(t *testing.T) {
	huge := strings.Repeat("é", MaxFrameLen/2+1)
	cases := []struct {
		name  string
		chat  Chat
		check func(Chat) string
	}{
		{"text", Chat{JID: "15551230001@s.whatsapp.net", Name: "Ada",
			LastMessage: &Message{ID: "big", Kind: KindText, Text: huge, Timestamp: "2026-10-18T17:58:00Z"}},
			func(c Chat) string {
				if c.Name != "Ada" || !strings.HasSuffix(c.LastMessage.Text, "é…") {
					return "want the name whole and the text cut ending in é…"
				}
				return ""
			}},
		{"file name", Chat{JID: "15551230001@s.whatsapp.net", Name: "Ada",
			LastMessage: &Message{ID: "big", Kind: KindDocument, Text: huge, Timestamp: "2026-10-18T17:58:00Z",
				Media: &Media{Mime: "application/pdf", Size: 9, FileName: huge}}},
			func(c Chat) string {
				if c.LastMessage.Text != "…" || !strings.HasSuffix(c.LastMessage.Media.FileName, "é…") {
					return "want the text … and the file name cut ending in é…"
				}
				return ""
			}},
		{"name", Chat{JID: "15551230001@s.whatsapp.net", Name: huge}, func(c Chat) string {
			if !strings.HasSuffix(c.Name, "é…") {
				return "want the name cut ending in é…"
			}
			return ""
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			reply := chatsReply(t, []Chat{tc.chat, longChat(1, 100)}, MaxChatLimit)
			if len(reply) > MaxFrameLen {
				t.Fatalf("chats reply is %d bytes, over the frame limit", len(reply))
			}
			if !utf8.Valid(reply) {
				t.Fatal("chats reply is not valid UTF-8: a cut fell inside a code point")
			}
			var got ChatsResponse
			if err := json.Unmarshal(reply, &got); err != nil || len(got.Chats) != 1 || got.Chats[0].JID != tc.chat.JID {
				t.Fatalf("reply = %.80s (%v), want only the cut chat", reply, err)
			}
			if msg := tc.check(got.Chats[0]); msg != "" {
				t.Error(msg)
			}
			// The handler's chats are not changed: the cut is the reply's copy only.
			if m := tc.chat.LastMessage; (m != nil && (m.Text != huge || (m.Media != nil && m.Media.FileName != huge))) ||
				(m == nil && tc.chat.Name != huge) {
				t.Error("the chats reply changed the handler's chats")
			}
			// Cut as little as possible: one more code point would not fit.
			if len(reply) < MaxFrameLen-len("é")-1 {
				t.Errorf("reply is %d bytes, cut more than needed to fit %d", len(reply), MaxFrameLen)
			}
		})
	}
}

// The fit counts the comma between two chats: chats whose encodings alone add up to the frame, but
// not with their commas, are not all served.
func TestTheChatsFitCountsTheCommasBetweenChats(t *testing.T) {
	var chats []Chat
	for i := 0; i < MaxChatLimit; i++ {
		chats = append(chats, longChat(i, 5000))
	}
	empty, err := json.Marshal(ChatsResponse{V: Version, Op: OpChats, Chats: []Chat{}})
	if err != nil {
		t.Fatal(err)
	}
	sum := len(empty)
	for _, c := range chats {
		b, err := json.Marshal(c)
		if err != nil {
			t.Fatal(err)
		}
		sum += len(b)
	}
	// Without their commas the chats fill the frame exactly.
	chats[0].LastMessage.Text += strings.Repeat("x", MaxFrameLen-sum)
	reply := chatsReply(t, chats, MaxChatLimit)
	var got ChatsResponse
	if err := json.Unmarshal(reply, &got); err != nil || len(reply) > MaxFrameLen || len(got.Chats) != MaxChatLimit-1 {
		t.Fatalf("reply is %d bytes with %d chats (%v), want %d chats within %d bytes",
			len(reply), len(got.Chats), err, MaxChatLimit-1, MaxFrameLen)
	}
}

// fittingChats are chats replies that fit in one frame, from the contract fixture to exactly the
// frame limit, so their bytes pin what the server wrote before the fit existed.
func fittingChats(t *testing.T) map[string][]Chat {
	t.Helper()
	out := map[string][]Chat{"fixture": newFakeHandler(t).chats, "empty": nil}
	var under []Chat
	for i := 0; i < MaxChatLimit; i++ {
		c := longChat(i, 4800)
		c.Aliases = []string{fmt.Sprintf("15551239%03d@s.whatsapp.net", i)}
		c.Phone = "…0001"
		c.Muted, c.Pinned = i%2 == 0, i%5 == 0
		c.LastMessage.Reactions = []Reaction{{Emoji: "👍", FromMe: true}}
		under = append(under, c)
	}
	out["under"] = under
	// Exactly MaxFrameLen bytes: the first chat's text is padded to fill the frame.
	exact := []Chat{longChat(0, 100)}
	for i := 1; i < MaxChatLimit; i++ {
		exact = append(exact, longChat(i, 5000))
	}
	b, err := json.Marshal(ChatsResponse{V: Version, Op: OpChats, Chats: exact})
	if err != nil {
		t.Fatal(err)
	}
	exact[0].LastMessage.Text += strings.Repeat("x", MaxFrameLen-len(b))
	out["exact"] = exact
	return out
}

// Chats replies that fit in one frame are served byte for byte as before the fit existed (sha256
// of each frame as the server wrote it at 455d283).
func TestChatsRepliesThatFitAreByteIdenticalToBefore(t *testing.T) {
	want := map[string]string{
		"fixture": "bc5f1b96f09995e781c2e88c78448abde487e75275871b537cf09b373d8a757c",
		"empty":   "6f2f48474f19448ed718e502c2f947997cf9dd91bc7160d78a86a1000bfb96c4",
		"under":   "e0265f7ef02deb6faa629d6abf95b426b29bee5f5e7dec4002c9f502b0717ca5",
		"exact":   "d0d594d7e2be42df8703659d97ecf7485000801e40712c6aaa329caa9a63470b",
	}
	for name, chats := range fittingChats(t) {
		reply := chatsReply(t, chats, MaxChatLimit)
		if name == "exact" && len(reply) != MaxFrameLen {
			t.Errorf("exact: %d bytes, want %d", len(reply), MaxFrameLen)
		}
		if got := sha(reply); got != want[name] {
			t.Errorf("%s: sha256 %s (%d bytes), want %s", name, got, len(reply), want[name])
		}
	}
}

// eventFrame publishes ev to one subscriber and returns the frame it reads, then checks that the
// subscriber still receives the next event.
func eventFrame(t *testing.T, ev any) []byte {
	t.Helper()
	b := NewBroker()
	_, br := subscribed(t, startEventServer(t, newFakeHandler(t), b))
	if n := publish(t, b, ev); n != 1 {
		t.Fatalf("event queued for %d subscribers, want 1", n)
	}
	frame := recvRaw(t, br)
	publish(t, b, eventMessage("next", "after"))
	if got := decodedJSON(t, recvRaw(t, br)).(map[string]any)["message"].(map[string]any)["id"]; got != "next" {
		t.Fatalf("next event after it = %v, want the next message", got)
	}
	return frame
}

// An incoming message whose event does not fit in one frame reaches the subscriber cut, its text
// then its file name on a code point boundary ending in "…", instead of a frame too long to read.
func TestAnEventMessageTooLongForOneFrameIsCut(t *testing.T) {
	huge := strings.Repeat("é", MaxFrameLen/2+1)
	text := eventMessage("big", huge)
	text.Message.Thumb, text.Message.ThumbMime = []byte("\xff\xd8\xff fake thumbnail"), "image/jpeg"
	doc := eventMessage("big", huge)
	doc.Message.Kind = KindDocument
	doc.Message.Media = &Media{Mime: "application/pdf", Size: 9, FileName: huge}
	cases := []struct {
		name  string
		ev    EventMessage
		check func(Message) string
	}{
		{"text", text, func(m Message) string {
			if !strings.HasSuffix(m.Text, "é…") || string(m.Thumb) != string(text.Message.Thumb) {
				return "want the text cut ending in é… and the thumbnail kept"
			}
			return ""
		}},
		{"file name", doc, func(m Message) string {
			if m.Text != "…" || !strings.HasSuffix(m.Media.FileName, "é…") {
				return "want the text … and the file name cut ending in é…"
			}
			return ""
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			frame := eventFrame(t, tc.ev)
			if len(frame) > MaxFrameLen {
				t.Fatalf("event frame is %d bytes, over the %d byte frame limit", len(frame), MaxFrameLen)
			}
			if !utf8.Valid(frame) {
				t.Fatal("event frame is not valid UTF-8: a cut fell inside a code point")
			}
			var got EventMessage
			if err := json.Unmarshal(frame, &got); err != nil || got.Op != OpEventMessage || got.JID != tc.ev.JID || got.Message.ID != "big" {
				t.Fatalf("event = %.80s (%v), want the big message", frame, err)
			}
			if msg := tc.check(got.Message); msg != "" {
				t.Error(msg)
			}
			if len(frame) < MaxFrameLen-len("é")-1 {
				t.Errorf("event is %d bytes, cut more than needed to fit %d", len(frame), MaxFrameLen)
			}
			// The value published is not changed: the cut is the subscribers' copy only.
			if tc.ev.Message.Text != huge || (tc.ev.Message.Media != nil && tc.ev.Message.Media.FileName != huge) {
				t.Error("Publish changed the caller's event")
			}
		})
	}
}

// fittingEvents are events that fit in one frame, up to exactly the frame limit, so their bytes pin
// what subscribers received before the fit existed.
func fittingEvents(t *testing.T) map[string]any {
	t.Helper()
	group := eventMessage("G1", "hi <there> & \"you\"")
	group.JID = "120363000000000000@g.us"
	group.Message.Sender, group.Message.ReplyTo = "Ada", "G0"
	group.Message.Reactions = []Reaction{{Emoji: "🎉"}}
	image := eventMessage("I1", "the venue")
	image.Message.Kind, image.Message.Media = KindImage, &Media{Mime: "image/jpeg", Size: 26}
	image.Message.Thumb, image.Message.ThumbMime, image.Message.Width, image.Message.Height = []byte("\xff\xd8\xff thumb"), "image/jpeg", 640, 480
	exact := eventMessage("X1", "")
	b, err := json.Marshal(exact)
	if err != nil {
		t.Fatal(err)
	}
	exact.Message.Text = strings.Repeat("x", MaxFrameLen-len(b))
	return map[string]any{
		"small": eventMessage("S1", "hello"), "group": group, "image": image, "exact": exact,
		"chat":   EventChat{V: Version, Op: OpEventChat, Chat: longChat(1, 100)},
		"unread": EventUnread{V: Version, Op: OpEventUnread, Unread: 3, JID: "15551230001@s.whatsapp.net", ChatUnread: 1},
	}
}

// Events that fit in one frame reach subscribers byte for byte as before the fit existed (sha256
// of each frame as a subscriber read it at 455d283).
func TestEventsThatFitAreByteIdenticalToBefore(t *testing.T) {
	want := map[string]string{
		"small":  "3da93fb27fdd17050ba52bdade55ed109fce4f493bd2cd433a53cc43893cdcc7",
		"group":  "eeec289d9f75847eabc14f15016034a9d654a2a1480534a247aac760cbb845e9",
		"image":  "df64f16fa57071bf59ffee51035c47d4dc6c16f6c0983cc8aad54cebfe8c179a",
		"exact":  "5b6dd49eaf30afdb19230a61eea9dce9965ae90321ee58ad6b965a6e42e8ddfe",
		"chat":   "79af79b77301cbc25bd189c469fd7d4ecf44aebd8263ff0df7cf8143b231bae5",
		"unread": "4cbc40dc97f58be1d9acf25f138b459528a2005b78e3f264e26d6d244626bb68",
	}
	for name, ev := range fittingEvents(t) {
		frame := eventFrame(t, ev)
		if name == "exact" && len(frame) != MaxFrameLen {
			t.Errorf("exact: %d bytes, want %d", len(frame), MaxFrameLen)
		}
		if got := sha(frame); got != want[name] {
			t.Errorf("%s: sha256 %s (%d bytes), want %s", name, got, len(frame), want[name])
		}
	}
}
