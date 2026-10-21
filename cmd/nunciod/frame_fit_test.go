package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waE2E"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// ask writes one request frame on conn and returns the reply line.
func ask(t *testing.T, conn net.Conn, br *bufio.Reader, request string) string {
	t.Helper()
	if _, err := io.WriteString(conn, request+"\n"); err != nil {
		t.Fatalf("write %.60s: %v", request, err)
	}
	reply := readFrame(t, conn, br, 10*time.Second)
	if reply == "" {
		t.Fatalf("no reply to %.60s", request)
	}
	return reply
}

// A messages reply whose texts alone do not fit in one frame no longer closes the connection: the
// daemon answers with the newest messages that fit, the connection serves the next request, and a
// message left out is still found by search. One text larger than a frame is served cut.
func TestAMessagesReplyTooLongForOneFrameKeepsTheConnection(t *testing.T) {
	const chat = "15551230001@s.whatsapp.net"
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	for i := 0; i < ipc.MaxChatLimit; i++ {
		id := fmt.Sprintf("word%03d", i)
		m := store.Message{ID: id, ChatJID: chat, Kind: store.KindText, Timestamp: t0.Add(time.Duration(i) * time.Second),
			Text: id + " " + strings.Repeat("x", 5250-len(id)-1)}
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	d, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	conn, br := subscriber(t, serveSocket(t, d, ipc.NewBroker()))

	reply := ask(t, conn, br, `{"v":1,"op":"messages","jid":"`+chat+`","limit":200}`)
	if len(reply) > ipc.MaxFrameLen {
		t.Fatalf("messages reply is %d bytes, over the %d byte frame limit", len(reply), ipc.MaxFrameLen)
	}
	var got ipc.MessagesResponse
	if err := json.Unmarshal([]byte(reply), &got); err != nil || got.Op != ipc.OpMessages {
		t.Fatalf("messages reply = %.80s (%v), want a messages frame", reply, err)
	}
	n := len(got.Messages)
	if n == 0 || n == ipc.MaxChatLimit || got.Messages[0].ID != "word199" || got.Messages[n-1].ID != fmt.Sprintf("word%03d", 200-n) {
		t.Fatalf("reply keeps %d messages from %s, want the newest that fit, from word199", n, got.Messages[0].ID)
	}
	if status := ask(t, conn, br, `{"v":1,"op":"status"}`); !strings.Contains(status, `"op":"status"`) {
		t.Fatalf("status after the long reply = %.80s, want a status frame on the same connection", status)
	}
	// The oldest message, left out of the reply, is still reachable.
	found := ask(t, conn, br, `{"v":1,"op":"search","jid":"`+chat+`","query":"word000","limit":200}`)
	if !strings.Contains(found, `"id":"word000"`) {
		t.Errorf("search for the oldest message = %.80s, want word000", found)
	}

	// One incoming text of 1.2 MiB (the store sets no limit) is served cut to fit, and the
	// connection stays usable.
	huge := store.Message{ID: "huge", ChatJID: chat, Kind: store.KindText, Timestamp: t0.Add(time.Hour),
		Text: strings.Repeat("é", (12<<20)/10/2+1)}
	if _, err := store.InsertMessage(db, huge); err != nil {
		t.Fatal(err)
	}
	reply = ask(t, conn, br, `{"v":1,"op":"messages","jid":"`+chat+`","limit":200}`)
	if len(reply) > ipc.MaxFrameLen {
		t.Fatalf("reply with a 1.2 MiB text is %d bytes, over the frame limit", len(reply))
	}
	got = ipc.MessagesResponse{}
	if err := json.Unmarshal([]byte(reply), &got); err != nil || len(got.Messages) == 0 || got.Messages[0].ID != "huge" ||
		!strings.HasSuffix(got.Messages[0].Text, "é…") {
		t.Fatalf("reply = %.80s (%v), want the huge message first, cut and ending in …", reply, err)
	}
	if status := ask(t, conn, br, `{"v":1,"op":"status"}`); !strings.Contains(status, `"op":"status"`) {
		t.Errorf("status after the cut reply = %.80s, want a status frame", status)
	}
}

// A trimmed messages reply whose left-out message is a photo with a thumbnail is served, and the
// daemon keeps serving the connection (before, the thumbnail pass indexed past the kept messages
// and the panic took the daemon down).
func TestATrimmedReplyWithAnOldPhotoKeepsTheDaemonServing(t *testing.T) {
	const chat = "15551230001@s.whatsapp.net"
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	ref, err := proto.Marshal(&waE2E.ImageMessage{Mimetype: proto.String("image/jpeg"),
		JPEGThumbnail: []byte("\xff\xd8\xff\xe0 fake embedded jpeg thumbnail"), Width: proto.Uint32(640), Height: proto.Uint32(480)})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.InsertMessage(db, store.Message{ID: "photo", ChatJID: chat, Kind: store.KindImage, Mime: "image/jpeg",
		Size: 26, Timestamp: t0, MediaRef: ref}); err != nil {
		t.Fatal(err)
	}
	for i := 1; i < ipc.MaxChatLimit; i++ {
		id := fmt.Sprintf("word%03d", i)
		m := store.Message{ID: id, ChatJID: chat, Kind: store.KindText, Timestamp: t0.Add(time.Duration(i) * time.Second),
			Text: id + " " + strings.Repeat("x", 5300-len(id)-1)}
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	d, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	conn, br := subscriber(t, serveSocket(t, d, ipc.NewBroker()))
	reply := ask(t, conn, br, `{"v":1,"op":"messages","jid":"`+chat+`","limit":200}`)
	if len(reply) > ipc.MaxFrameLen || !strings.Contains(reply, `"id":"word199"`) || strings.Contains(reply, `"id":"photo"`) {
		t.Fatalf("messages reply (%d bytes) = %.80s, want the newest texts that fit without the old photo", len(reply), reply)
	}
	if status := ask(t, conn, br, `{"v":1,"op":"status"}`); !strings.Contains(status, `"op":"status"`) {
		t.Fatalf("status after the trimmed reply = %.80s, want a status frame", status)
	}
}
