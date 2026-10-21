package ipc

import (
	"bytes"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

// fixturesDir is the shared contract directory both languages read.
func fixturesDir(t *testing.T) string {
	t.Helper()
	dir := filepath.Join("..", "..", "proto", "fixtures")
	if _, err := os.Stat(dir); err != nil {
		t.Fatalf("fixtures directory %s: %v", dir, err)
	}
	return dir
}

func readFixture(t *testing.T, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(fixturesDir(t), name))
	if err != nil {
		t.Fatalf("read fixture %s: %v", name, err)
	}
	return bytes.TrimRight(data, "\n")
}

// decodedJSON is a JSON document decoded into generic Go values, with numbers
// kept as json.Number so ints stay ints.
func decodedJSON(t *testing.T, data []byte) any {
	t.Helper()
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		t.Fatalf("decode %s: %v", data, err)
	}
	if _, err := dec.Token(); err != io.EOF {
		t.Fatalf("decode %s: trailing data after the JSON document", data)
	}
	return v
}

// sameJSON compares two JSON documents semantically: key order and whitespace
// are not part of the contract, values are.
func sameJSON(t *testing.T, a, b []byte) bool {
	t.Helper()
	return reflect.DeepEqual(decodedJSON(t, a), decodedJSON(t, b))
}

func assertSameJSON(t *testing.T, got, want []byte) {
	t.Helper()
	if !sameJSON(t, got, want) {
		t.Fatalf("JSON mismatch:\n got %s\nwant %s", got, want)
	}
}

func TestSameJSONComparesSemantically(t *testing.T) {
	cases := []struct {
		name string
		a, b string
		want bool
	}{
		{name: "identical", a: `{"v":1,"op":"status"}`, b: `{"v":1,"op":"status"}`, want: true},
		{name: "key order ignored", a: `{"v":1,"op":"status"}`, b: `{"op":"status","v":1}`, want: true},
		{name: "whitespace ignored", a: `{"v":1,"op":"status"}`, b: "{\n  \"v\": 1,\n  \"op\": \"status\"\n}", want: true},
		{name: "different value", a: `{"v":1}`, b: `{"v":2}`, want: false},
		{name: "missing key", a: `{"v":1,"unread":0}`, b: `{"v":1}`, want: false},
		{name: "false is not absent", a: `{"linked":false}`, b: `{}`, want: false},
		{name: "zero is not absent", a: `{"unread":0}`, b: `{}`, want: false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := sameJSON(t, []byte(tc.a), []byte(tc.b)); got != tc.want {
				t.Fatalf("sameJSON(%s, %s) = %v, want %v", tc.a, tc.b, got, tc.want)
			}
		})
	}
}

type fixtureCase struct {
	file string
	// into decodes the fixture into the wire type that side expects.
	into func() any
	// build constructs the same value in Go, which is what proves the two
	// languages agree on the shape rather than each agreeing with itself.
	build func() any
}

func fixtureCases() []fixtureCase {
	return []fixtureCase{
		{
			file:  "hello_client.json",
			into:  func() any { return &Hello{} },
			build: func() any { return Hello{V: 1, Op: OpHello, Client: "nuncio-tui/0.1.0"} },
		},
		{
			file:  "status_request.json",
			into:  func() any { return &StatusRequest{} },
			build: func() any { return StatusRequest{V: 1, Op: OpStatus} },
		},
		{
			file:  "chats_request.json",
			into:  func() any { return &ChatsRequest{} },
			build: func() any { return ChatsRequest{V: 1, Op: OpChats, Limit: 40} },
		},
		{
			file:  "hello_ok_daemon.json",
			into:  func() any { return &HelloOK{} },
			build: func() any { return HelloOK{V: 1, Op: OpHelloOK, Daemon: "nunciod/0.1.0", Session: SessionUnlinked} },
		},
		{
			file: "status_response.json",
			into: func() any { return &StatusResponse{} },
			build: func() any {
				return StatusResponse{V: 1, Op: OpStatus, Daemon: "nunciod/0.1.0", Session: SessionUnlinked}
			},
		},
		{
			file:  "chats_response.json",
			into:  func() any { return &ChatsResponse{} },
			build: func() any { return ChatsResponse{V: 1, Op: OpChats, Chats: fixtureChatList()} },
		},
		{
			file: "error_version_mismatch.json",
			into: func() any { return &Error{} },
			build: func() any {
				return Error{V: 1, Op: OpError, Code: CodeVersionMismatch, Message: "unsupported protocol version"}
			},
		},
		{
			file: "error_not_linked.json",
			into: func() any { return &Error{} },
			build: func() any {
				return Error{V: 1, Op: OpError, Code: CodeNotLinked, Message: "no WhatsApp device is linked"}
			},
		},
		{
			file: "error_not_confirmed.json",
			into: func() any { return &Error{} },
			build: func() any {
				return Error{V: 1, Op: OpError, Code: CodeNotConfirmed, Message: ErrNotConfirmed.Error()}
			},
		},
		{
			file: "messages_request.json",
			into: func() any { return &MessagesRequest{} },
			build: func() any {
				return MessagesRequest{V: 1, Op: OpMessages, JID: "15551230001@s.whatsapp.net", Limit: 50}
			},
		},
		{
			file: "messages_response.json",
			into: func() any { return &MessagesResponse{} },
			build: func() any {
				return MessagesResponse{V: 1, Op: OpMessages, JID: "15551230001@s.whatsapp.net", Messages: []Message{
					{ID: "3EB0AAA1", Kind: KindText, Text: "See you at 18:00", Timestamp: "2026-10-18T17:55:04Z"},
					{
						ID: "3EB0AAA0", Kind: KindImage, Text: "the venue", Timestamp: "2026-10-18T17:50:30Z",
						Media: &Media{Mime: "image/jpeg", Size: 48213},
					},
				}}
			},
		},
		{
			file: "send_request.json",
			into: func() any { return &SendRequest{} },
			build: func() any {
				return SendRequest{V: 1, Op: OpSend, JID: "15551230001@s.whatsapp.net", Text: "On my way", ClientID: "c-7f3a9e21"}
			},
		},
		{
			file: "sent_response.json",
			into: func() any { return &Sent{} },
			build: func() any {
				return Sent{V: 1, Op: OpSent, ClientID: "c-7f3a9e21", State: SendSent, ID: "3EB0BBB1", Timestamp: "2026-10-18T17:56:10Z"}
			},
		},
		{
			file:  "sent_queued_response.json",
			into:  func() any { return &Sent{} },
			build: func() any { return Sent{V: 1, Op: OpSent, ClientID: "c-7f3a9e22", State: SendQueued} },
		},
		{
			file: "send_failed_response.json",
			into: func() any { return &SendFailed{} },
			build: func() any {
				return SendFailed{V: 1, Op: OpSendFailed, ClientID: "c-7f3a9e23", Message: "the server did not acknowledge the message"}
			},
		},
		{
			file:  "subscribe_request.json",
			into:  func() any { return &SubscribeRequest{} },
			build: func() any { return SubscribeRequest{V: 1, Op: OpSubscribe} },
		},
		{
			file:  "subscribed_response.json",
			into:  func() any { return &Subscribed{} },
			build: func() any { return Subscribed{V: 1, Op: OpSubscribed} },
		},
		{
			file:  "focus_request.json",
			into:  func() any { return &FocusRequest{} },
			build: func() any { return FocusRequest{V: 1, Op: OpFocus, JID: "15551230002@s.whatsapp.net"} },
		},
		{
			file:  "focused_response.json",
			into:  func() any { return &Focused{} },
			build: func() any { return Focused{V: 1, Op: OpFocused, JID: "15551230002@s.whatsapp.net", Delivered: 1} },
		},
		{
			file:  "wipe_request.json",
			into:  func() any { return &WipeRequest{} },
			build: func() any { return WipeRequest{V: 1, Op: OpWipe, Unlink: true} },
		},
		{
			file:  "wiped_response.json",
			into:  func() any { return &Wiped{} },
			build: func() any { return Wiped{V: 1, Op: OpWiped, Unlinked: true} },
		},
		{
			file: "event_message.json",
			into: func() any { return &EventMessage{} },
			build: func() any {
				return EventMessage{V: 1, Op: OpEventMessage, JID: "15551230001@s.whatsapp.net", Message: Message{
					ID: "3EB0AAA3", Kind: KindText, Text: "Running late", Timestamp: "2026-10-18T17:58:00Z",
				}}
			},
		},
		{
			file: "event_message_not_sent.json",
			into: func() any { return &EventMessage{} },
			build: func() any {
				return EventMessage{V: 1, Op: OpEventMessage, JID: "15551230001@s.whatsapp.net", Message: Message{
					ID: "3EB0BBB3", Kind: KindText, Text: "On my way", FromMe: true, Timestamp: "2026-10-18T17:56:12Z",
					State: MessageNotSent, ClientID: "c-7f3a9e23",
				}}
			},
		},
		{
			// An image with the thumbnail embedded in the message itself and its size (T15 Part 2.5).
			file: "event_message_image.json",
			into: func() any { return &EventMessage{} },
			build: func() any {
				return EventMessage{V: 1, Op: OpEventMessage, JID: "15551230001@s.whatsapp.net", Message: Message{
					ID: "3EB0AAA4", Kind: KindImage, Text: "the venue", Timestamp: "2026-10-18T17:59:00Z",
					Media: &Media{Mime: "image/jpeg", Size: 48213}, Thumb: []byte("\xff\xd8\xff\xe0fake jpeg thumbnail"),
					ThumbMime: "image/jpeg", Width: 1280, Height: 960,
				}}
			},
		},
		{
			// An animated sticker: kind sticker, no text, its PNG thumbnail.
			file: "event_message_sticker.json",
			into: func() any { return &EventMessage{} },
			build: func() any {
				return EventMessage{V: 1, Op: OpEventMessage, JID: "15551230001@s.whatsapp.net", Message: Message{
					ID: "3EB0AAA5", Kind: KindSticker, Timestamp: "2026-10-18T17:59:30Z",
					Media: &Media{Mime: "image/webp", Size: 18000}, Thumb: []byte("\x89PNG\r\n\x1a\nfake png thumbnail"),
					ThumbMime: "image/png", Width: 512, Height: 512, Animated: true,
				}}
			},
		},
		{
			// A group's messages: each incoming one names its author in `sender` (a name, else the
			// masked number); mine carries none.
			file: "messages_group_response.json",
			into: func() any { return &MessagesResponse{} },
			build: func() any {
				return MessagesResponse{V: 1, Op: OpMessages, JID: "120363000000000001@g.us", Messages: []Message{
					{ID: "3EB0CCC3", Kind: KindText, Text: "On my way", FromMe: true, Timestamp: "2026-10-18T18:02:00Z"},
					{ID: "3EB0CCC2", Kind: KindText, Text: "Who brings the rope?", Timestamp: "2026-10-18T18:01:00Z", Sender: "…0002"},
					{ID: "3EB0CCC1", Kind: KindText, Text: "Meet at the crag", Timestamp: "2026-10-18T18:00:00Z", Sender: "Ana"},
				}}
			},
		},
		{
			file: "event_message_group.json",
			into: func() any { return &EventMessage{} },
			build: func() any {
				return EventMessage{V: 1, Op: OpEventMessage, JID: "120363000000000001@g.us", Message: Message{
					ID: "3EB0CCC4", Kind: KindText, Text: "Running late", Timestamp: "2026-10-18T18:03:00Z", Sender: "Ana",
				}}
			},
		},
		{
			file:  "retry_request.json",
			into:  func() any { return &RetryRequest{} },
			build: func() any { return RetryRequest{V: 1, Op: OpRetry, ClientID: "c-7f3a9e23"} },
		},
		{
			file:  "event_connection.json",
			into:  func() any { return &EventConnection{} },
			build: func() any { return EventConnection{V: 1, Op: OpEventConnection, Session: SessionOffline} },
		},
		{
			file: "event_unread.json",
			into: func() any { return &EventUnread{} },
			build: func() any {
				return EventUnread{V: 1, Op: OpEventUnread, Unread: 20, JID: "15551230001@s.whatsapp.net", ChatUnread: 3}
			},
		},
		{
			file: "event_chat.json",
			into: func() any { return &EventChat{} },
			build: func() any {
				ada := fixtureChatList()[0]
				ada.Muted = true
				return EventChat{V: 1, Op: OpEventChat, Chat: ada}
			},
		},
		{
			// ADR 0001: the LID chat with the phone number merged into it.
			file: "event_chat_aliases.json",
			into: func() any { return &EventChat{} },
			build: func() any {
				return EventChat{V: 1, Op: OpEventChat, Chat: Chat{
					JID: "100000000000001@lid", Name: "Ada Lovelace", Unread: 1,
					Aliases: []string{"15551230001@s.whatsapp.net"},
					LastMessage: &Message{ID: "3EB0AAA3", Kind: "text", Text: "reply from ada",
						Timestamp: "2026-10-18T18:01:00Z"},
				}}
			},
		},
		{
			// A contact with no name: the masked phone number is what a client shows.
			file: "event_chat_phone.json",
			into: func() any { return &EventChat{} },
			build: func() any {
				return EventChat{V: 1, Op: OpEventChat, Chat: Chat{JID: "100000000000002@lid", Phone: MaskPhone("15551230002")}}
			},
		},
		{
			file: "download_media_request.json",
			into: func() any { return &DownloadMediaRequest{} },
			build: func() any {
				return DownloadMediaRequest{V: 1, Op: OpDownloadMedia, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA0"}
			},
		},
		{
			file: "media_ready_response.json",
			into: func() any { return &MediaReady{} },
			build: func() any {
				return MediaReady{V: 1, Op: OpMediaReady, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA0",
					Path: fixtureMediaPath, Mime: "image/jpeg"}
			},
		},
		{
			file:  "event_focus.json",
			into:  func() any { return &EventFocus{} },
			build: func() any { return EventFocus{V: 1, Op: OpEventFocus, JID: "15551230002@s.whatsapp.net"} },
		},
		{
			file: "search_request.json",
			into: func() any { return &SearchRequest{} },
			build: func() any {
				return SearchRequest{V: 1, Op: OpSearch, JID: "15551230001@s.whatsapp.net", Query: "venue", Limit: 50}
			},
		},
		{
			file: "search_response.json",
			into: func() any { return &MessagesResponse{} },
			build: func() any {
				return MessagesResponse{V: 1, Op: OpMessages, JID: "15551230001@s.whatsapp.net", Messages: []Message{
					{ID: "3EB0AAA2", Kind: "text", Text: "Which venue?", FromMe: true, Timestamp: "2026-10-18T17:56:40Z",
						ReplyTo: "3EB0AAA0", Reactions: []Reaction{{Emoji: "👍", FromMe: false}}},
					{ID: "3EB0AAA0", Kind: "image", Text: "the venue", Timestamp: "2026-10-18T17:50:30Z",
						Media: &Media{Mime: "image/jpeg", Size: 48213}},
				}}
			},
		},
		{
			file: "send_reply_request.json",
			into: func() any { return &SendRequest{} },
			build: func() any {
				return SendRequest{V: 1, Op: OpSend, JID: "15551230001@s.whatsapp.net", Text: "Which venue?",
					ClientID: "c-7f3a9e22", ReplyTo: "3EB0AAA0"}
			},
		},
		{
			file: "react_request.json",
			into: func() any { return &ReactRequest{} },
			build: func() any {
				return ReactRequest{V: 1, Op: OpReact, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA1", Emoji: "👍"}
			},
		},
		{
			file: "react_remove_request.json",
			into: func() any { return &ReactRequest{} },
			build: func() any {
				return ReactRequest{V: 1, Op: OpReact, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA1"}
			},
		},
		{
			file: "reacted_response.json",
			into: func() any { return &Reacted{} },
			build: func() any {
				return Reacted{V: 1, Op: OpReacted, JID: "15551230001@s.whatsapp.net", ID: "3EB0AAA1", Emoji: "👍"}
			},
		},
		{
			file: "mark_read_request.json",
			into: func() any { return &MarkReadRequest{} },
			build: func() any {
				return MarkReadRequest{V: 1, Op: OpMarkRead, JID: "15551230001@s.whatsapp.net"}
			},
		},
		{
			file: "marked_read_response.json",
			into: func() any { return &MarkedRead{} },
			build: func() any {
				return MarkedRead{V: 1, Op: OpMarkedRead, JID: "15551230001@s.whatsapp.net", Unread: 17}
			},
		},
	}
}

// fixtureMediaPath is the path in media_ready_response.json.
const fixtureMediaPath = "/home/ada/.local/share/nuncio/media/e9603b3a3952bcc1847052b2b73de4ac145b06a2c5a615735b7440e41d65ad12.jpeg"

// fixtureChatList is the chat list of proto/fixtures/chats_response.json,
// constructed by hand so a shape disagreement fails here and not in the Rust
// client.
func fixtureChatList() []Chat {
	return []Chat{
		{
			JID:      "15551230001@s.whatsapp.net",
			Name:     "Ada Lovelace",
			Unread:   2,
			Pinned:   true,
			Archived: false,
			LastMessage: &Message{
				ID: "3EB0AAA1", Kind: "text", Text: "See you at 18:00",
				FromMe: false, Timestamp: "2026-10-18T17:55:04Z",
			},
		},
		{
			JID:    "15551230002@s.whatsapp.net",
			Name:   "Grace Hopper",
			Unread: 0,
			LastMessage: &Message{
				ID: "3EB0AAA2", Kind: "text", Text: "The compiler is done",
				FromMe: true, Timestamp: "2026-10-18T16:02:11Z",
			},
		},
		{
			JID:      "120363000000000000@g.us",
			Name:     "Omarchy devs",
			Unread:   17,
			Archived: true,
			Muted:    true,
		},
	}
}

// TestFixturesRoundTripThroughWireTypes is the contract test: every fixture
// decodes into the wire type and re-encodes to the same JSON.
func TestFixturesRoundTripThroughWireTypes(t *testing.T) {
	files, err := filepath.Glob(filepath.Join(fixturesDir(t), "*.json"))
	if err != nil {
		t.Fatalf("glob fixtures: %v", err)
	}
	covered := map[string]bool{}
	for _, tc := range fixtureCases() {
		covered[tc.file] = true
	}
	for _, path := range files {
		base := filepath.Base(path)
		if !covered[base] {
			t.Errorf("fixture %s has no contract case: add it to fixtureCases", base)
		}
	}

	for _, tc := range fixtureCases() {
		t.Run(tc.file, func(t *testing.T) {
			want := readFixture(t, tc.file)
			dst := tc.into()
			if err := json.Unmarshal(want, dst); err != nil {
				t.Fatalf("unmarshal %s into %T: %v", tc.file, dst, err)
			}
			got, err := json.Marshal(dst)
			if err != nil {
				t.Fatalf("marshal %T: %v", dst, err)
			}
			assertSameJSON(t, got, want)
		})
	}
}

// TestWireTypesBuildEveryFixture is the other half of the contract: the Go
// types can *produce* the frames, not just parse them.
func TestWireTypesBuildEveryFixture(t *testing.T) {
	for _, tc := range fixtureCases() {
		t.Run(tc.file, func(t *testing.T) {
			want := readFixture(t, tc.file)
			got, err := json.Marshal(tc.build())
			if err != nil {
				t.Fatalf("marshal %T: %v", tc.build(), err)
			}
			assertSameJSON(t, got, want)
		})
	}
}

func TestFixturesCarryVersionOne(t *testing.T) {
	known := map[string]bool{
		OpHello: true, OpHelloOK: true, OpStatus: true, OpChats: true, OpError: true,
		OpMessages: true, OpSend: true, OpSent: true, OpSendFailed: true, OpSubscribe: true,
		OpSubscribed: true, OpFocus: true, OpFocused: true, OpEventMessage: true,
		OpEventConnection: true, OpEventUnread: true, OpEventFocus: true, OpWipe: true, OpWiped: true,
		OpDownloadMedia: true, OpMediaReady: true, OpRetry: true, OpSearch: true, OpReact: true, OpReacted: true,
		OpMarkRead: true, OpMarkedRead: true, OpEventChat: true,
	}
	for _, tc := range fixtureCases() {
		t.Run(tc.file, func(t *testing.T) {
			raw := decodedJSON(t, readFixture(t, tc.file)).(map[string]any)
			if v, _ := raw["v"].(json.Number); v.String() != "1" {
				t.Errorf("v = %v, want 1", raw["v"])
			}
			op, _ := raw["op"].(string)
			if !known[op] {
				t.Errorf("op = %q, want one of the protocol ops", op)
			}
		})
	}
}

func TestChatsFixtureOmitsEmptyLastMessage(t *testing.T) {
	var resp ChatsResponse
	if err := json.Unmarshal(readFixture(t, "chats_response.json"), &resp); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(resp.Chats) != 3 {
		t.Fatalf("got %d chats, want 3", len(resp.Chats))
	}
	if resp.Chats[2].LastMessage != nil {
		t.Fatalf("third chat has a last message, want none")
	}
	got, err := json.Marshal(resp)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if bytes.Contains(got, []byte(`"last_message":null`)) {
		t.Fatalf("re-encoded chat list has an explicit null last_message: %s", got)
	}
	if !bytes.Contains(got, []byte(`"jid":"120363000000000000@g.us"`)) {
		t.Fatalf("re-encoded chat list lost the group chat: %s", got)
	}
}

func TestFixtureTimestampsAreRFC3339UTC(t *testing.T) {
	var resp ChatsResponse
	if err := json.Unmarshal(readFixture(t, "chats_response.json"), &resp); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	seen := 0
	for _, chat := range resp.Chats {
		if chat.LastMessage == nil {
			continue
		}
		seen++
		ts, err := time.Parse(TimestampFormat, chat.LastMessage.Timestamp)
		if err != nil {
			t.Fatalf("timestamp %q of %s: %v", chat.LastMessage.Timestamp, chat.JID, err)
		}
		if ts.Location() != time.UTC {
			t.Errorf("timestamp %q of %s is not UTC", chat.LastMessage.Timestamp, chat.JID)
		}
		if got := Timestamp(ts); got != chat.LastMessage.Timestamp {
			t.Errorf("Timestamp(%v) = %q, want %q", ts, got, chat.LastMessage.Timestamp)
		}
	}
	if seen != 2 {
		t.Fatalf("checked %d timestamps, want 2", seen)
	}
}

// TestMessageOmitsAbsentMedia keeps text messages free of a media key: absent stays absent, as
// with last_message.
func TestMessageOmitsAbsentMedia(t *testing.T) {
	got, err := json.Marshal(Message{ID: "x", Kind: KindText, Text: "hi", Timestamp: "2026-10-18T17:55:04Z"})
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(got, []byte(`"media"`)) {
		t.Fatalf("a text message carries a media key: %s", got)
	}
	// A sent message carries no outbox state either: absent means sent (or received).
	for _, key := range []string{`"state"`, `"client_id"`} {
		if bytes.Contains(got, []byte(key)) {
			t.Fatalf("a sent message carries %s: %s", key, got)
		}
	}
}

// A chat with no aliases is encoded exactly as before `aliases` existed: chats_response.json byte for
// byte (ADR 0001 adds the key only when there are aliases).
func TestAChatWithoutAliasesEncodesByteForByteAsBefore(t *testing.T) {
	want := strings.TrimSuffix(string(readFixture(t, "chats_response.json")), "\n")
	got, err := json.Marshal(ChatsResponse{V: 1, Op: OpChats, Chats: fixtureChatList()})
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != want {
		t.Errorf("chats =\n %s\nwant\n %s", got, want)
	}
}

// A message without a preview is encoded exactly as before thumb, thumb_mime, width, height and
// animated existed: messages_response.json and search_response.json byte for byte.
func TestAMessageWithoutAPreviewEncodesByteForByteAsBefore(t *testing.T) {
	for _, file := range []string{"messages_response.json", "search_response.json", "event_message.json"} {
		want := strings.TrimSuffix(string(readFixture(t, file)), "\n")
		var tc *fixtureCase
		for _, c := range fixtureCases() {
			if c.file == file {
				c := c
				tc = &c
			}
		}
		got, err := json.Marshal(tc.build())
		if err != nil {
			t.Fatal(err)
		}
		if string(got) != want {
			t.Errorf("%s =\n %s\nwant\n %s", file, got, want)
		}
	}
}

// MaskPhone keeps the last four digits only (the CLI's rule), so no full number reaches the wire.
func TestMaskPhoneKeepsTheLastFourDigitsOnly(t *testing.T) {
	for _, tc := range []struct{ in, want string }{
		{"15551230002", "…0002"},
		{"1234", "…"},
		{"", "…"},
	} {
		if got := MaskPhone(tc.in); got != tc.want {
			t.Errorf("MaskPhone(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}
