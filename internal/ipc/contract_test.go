package ipc

import (
	"bytes"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"reflect"
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
	}
}

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
	known := map[string]bool{OpHello: true, OpHelloOK: true, OpStatus: true, OpChats: true, OpError: true}
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
