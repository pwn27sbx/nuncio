package wa

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.mau.fi/whatsmeow/proto/waE2E"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// fakeJPEGThumb and fakePNGThumb stand for the thumbnails WhatsApp embeds in the message itself.
var (
	fakeJPEGThumb = []byte("\xff\xd8\xff\xe0 fake embedded jpeg thumbnail")
	fakePNGThumb  = []byte("\x89PNG\r\n\x1a\n fake embedded png thumbnail")
)

// jpegOf is a fake JPEG thumbnail of n bytes: the JPEG magic bytes, then padding.
func jpegOf(n int) []byte { return append([]byte{0xff, 0xd8, 0xff}, bytes.Repeat([]byte{1}, n-3)...) }

func b64(b []byte) string { return base64.StdEncoding.EncodeToString(b) }

func thumbImage(thumb []byte, w, h uint32) *waE2E.Message {
	m := imageMessage(sum(mediaBytes))
	m.ImageMessage.JPEGThumbnail = thumb
	if w != 0 {
		m.ImageMessage.Width = proto.Uint32(w)
	}
	if h != 0 {
		m.ImageMessage.Height = proto.Uint32(h)
	}
	return m
}

var stickerBytes = []byte("fake decrypted webp sticker bytes")

func stickerMessage(animated bool) *waE2E.Message {
	return &waE2E.Message{StickerMessage: &waE2E.StickerMessage{
		Mimetype:      proto.String("image/webp"),
		FileLength:    proto.Uint64(uint64(len(stickerBytes))),
		DirectPath:    proto.String("/v/t62.fake/0009"),
		MediaKey:      bytes.Repeat([]byte{5}, 32),
		FileSHA256:    sum(stickerBytes),
		FileEncSHA256: bytes.Repeat([]byte{6}, 32),
		Width:         proto.Uint32(512),
		Height:        proto.Uint32(512),
		IsAnimated:    proto.Bool(animated),
		PngThumbnail:  fakePNGThumb,
	}}
}

// An image message carries the thumbnail embedded in it, its mime type and the photo's size, read
// from the message itself: storing and publishing it downloads nothing.
func TestAnImageMessagePublishesItsEmbeddedThumbnailAndSize(t *testing.T) {
	s, fake, dl, _ := mediaSession(t)
	events := eventsOf(t, s)
	fake.emit(messageEvent(ada, "IMG1", false, t0, thumbImage(fakeJPEGThumb, 1280, 960)))

	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"IMG1","kind":"image","text":"the venue","from_me":false,"timestamp":"2026-10-18T17:55:04Z","media":{"mime":"image/jpeg","size":26,"file_name":""},"thumb":"`+b64(fakeJPEGThumb)+`","thumb_mime":"image/jpeg","width":1280,"height":960}}`,
		`{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":1}`,
	)
	// A reaction publishes the message again: it keeps its preview, read back from the store.
	s.publishStored(ada.String(), "IMG1")
	got := events()
	if len(got) != 1 || !strings.Contains(got[0], `"thumb":"`+b64(fakeJPEGThumb)+`","thumb_mime":"image/jpeg","width":1280,"height":960`) {
		t.Errorf("republished %q, want the image with its thumb, width and height", got)
	}
	if n := dl.calls(); n != 0 {
		t.Errorf("serving thumbnails called the downloader %d times, want 0", n)
	}
}

// An image without an embedded thumbnail, or with one larger than MaxThumb, carries no thumb; its
// width and height are still served. One without either is published exactly as before.
func TestAnEmptyOrOversizedThumbnailIsOmitted(t *testing.T) {
	for name, tc := range map[string]struct {
		msg  *waE2E.Message
		want string
	}{
		"no thumbnail": {thumbImage(nil, 800, 600), `,"width":800,"height":600}`},
		"oversized":    {thumbImage(jpegOf(MaxThumb+1), 800, 600), `,"width":800,"height":600}`},
		"at the cap":   {thumbImage(jpegOf(MaxThumb), 0, 0), `,"thumb":"` + b64(jpegOf(MaxThumb)) + `","thumb_mime":"image/jpeg"}`},
		"nothing":      {thumbImage(nil, 0, 0), `"file_name":""}}`},
	} {
		t.Run(name, func(t *testing.T) {
			_, fake, events := subscribedSession(t)
			fake.emit(messageEvent(ada, "IMG1", false, t0, tc.msg))
			got := events()
			if len(got) == 0 || !strings.HasSuffix(got[0], tc.want+"}") {
				t.Fatalf("published %q, want it to end with %s}", got, tc.want)
			}
			if name != "at the cap" && strings.Contains(got[0], `"thumb`) {
				t.Errorf("published %s, want no thumb", got[0])
			}
		})
	}
}

// A sticker, incoming or one of mine synced from another device, is stored as a message of kind
// sticker with no text and its media reference; it carries its embedded PNG thumbnail, its size
// and whether it is animated, and download_media serves it like an image, as a .webp file. Storing
// it downloads nothing.
func TestAStickerIsStoredAsKindStickerAndDownloadsOnRequest(t *testing.T) {
	for _, tc := range []struct {
		name     string
		fromMe   bool
		animated bool
		tail     string
	}{
		{"incoming animated", false, true, `,"animated":true}`},
		{"mine, static", true, false, `}`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, dl, dir := mediaSession(t)
			dl.plain = stickerBytes
			events := eventsOf(t, s)
			fake.emit(messageEvent(ada, "STK1", tc.fromMe, t0, stickerMessage(tc.animated)))

			got, err := store.GetMessage(s.Store(), ada.String(), "STK1")
			if err != nil {
				t.Fatalf("GetMessage: %v", err)
			}
			if got.Kind != store.KindSticker || got.Text != "" || got.Mime != "image/webp" || got.FromMe != tc.fromMe {
				t.Errorf("stored %+v, want a sticker of mime image/webp with no text", got)
			}
			ref := &waE2E.StickerMessage{}
			if err := proto.Unmarshal(got.MediaRef, ref); err != nil || !proto.Equal(ref, stickerMessage(tc.animated).GetStickerMessage()) {
				t.Errorf("stored reference decodes to %v (%v), want the sticker message", ref, err)
			}
			evs := events()
			if len(evs) == 0 || !strings.HasSuffix(evs[0], `"media":{"mime":"image/webp","size":33,"file_name":""},"thumb":"`+
				b64(fakePNGThumb)+`","thumb_mime":"image/png","width":512,"height":512`+tc.tail+`}`) {
				t.Fatalf("published %q, want the sticker with its png thumb, size and animated flag", evs)
			}
			if !strings.Contains(evs[0], `"kind":"sticker","text":""`) {
				t.Errorf("published %s, want kind sticker with no text", evs[0])
			}
			if n := dl.calls(); n != 0 {
				t.Fatalf("storing a sticker called the downloader %d times, want 0", n)
			}

			path, mime, err := s.DownloadMedia(ada.String(), "STK1")
			if err != nil {
				t.Fatalf("DownloadMedia: %v", err)
			}
			if mime != "image/webp" || filepath.Ext(path) != ".webp" || filepath.Dir(path) != dir {
				t.Errorf("DownloadMedia = %s %s, want a .webp file in %s", path, mime, dir)
			}
			if _, ok := dl.asked[0].(*waE2E.StickerMessage); !ok || dl.calls() != 1 {
				t.Errorf("downloader asked %d times for %T, want once for the sticker message", dl.calls(), dl.asked[0])
			}
		})
	}
}

// The newest message of a chat is listed in chats without its preview: a chat list stays as it
// was, whatever its last message embeds.
func TestAChatsLastMessageCarriesNoPreview(t *testing.T) {
	s, fake, _ := startedSession(t)
	fake.emit(messageEvent(ada, "IMG1", false, t0, thumbImage(fakeJPEGThumb, 1280, 960)))
	c, err := store.GetChat(s.Store(), ada.String())
	if err != nil {
		t.Fatal(err)
	}
	chat, err := WireChat(s.Store(), c, t0)
	if err != nil {
		t.Fatal(err)
	}
	want := ipc.Message{ID: "IMG1", Kind: ipc.KindImage, Text: "the venue", Timestamp: "2026-10-18T17:55:04Z",
		Media: &ipc.Media{Mime: "image/jpeg", Size: 26}}
	if chat.LastMessage == nil || jsonString(t, *chat.LastMessage) != jsonString(t, want) {
		t.Errorf("last_message = %+v, want %+v", chat.LastMessage, want)
	}
}

// WireMessages serves the stored messages with their previews, newest first, keeping the
// thumbnails of one reply within ThumbBudget, so a chat full of photos still fits in one frame:
// a thumbnail that does not fit is left out, its width and height are kept.
func TestWireMessagesKeepsThumbnailsWithinTheFrameBudget(t *testing.T) {
	big := jpegOf(MaxThumb)
	var rows []store.Message
	for i := 0; i < 2*ThumbBudget/MaxThumb; i++ {
		ref, err := proto.Marshal(thumbImage(big, 640, 480).GetImageMessage())
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, store.Message{ID: "IMG", Kind: store.KindImage, Mime: "image/jpeg", Timestamp: t0, MediaRef: ref})
	}
	rows = append(rows, store.Message{ID: "TXT", Kind: store.KindText, Text: "hi", Timestamp: t0})
	msgs := WireMessages(ada.String(), rows)
	total, with := 0, 0
	for i, m := range msgs {
		total += len(m.Thumb)
		if len(m.Thumb) > 0 {
			with++
			if i >= ThumbBudget/MaxThumb {
				t.Errorf("message %d carries a thumb; the newest ones should", i)
			}
		}
		if m.Kind == store.KindImage && (m.Width != 640 || m.Height != 480) {
			t.Errorf("message %d is %dx%d, want 640x480 with or without its thumb", i, m.Width, m.Height)
		}
	}
	if total > ThumbBudget || with != ThumbBudget/MaxThumb {
		t.Errorf("%d thumbs of %d bytes in all, want %d within %d", with, total, ThumbBudget/MaxThumb, ThumbBudget)
	}
	if err := ipc.WriteFrame(io.Discard, ipc.MessagesResponse{V: 1, Op: ipc.OpMessages, JID: ada.String(), Messages: msgs}); err != nil {
		t.Errorf("the messages frame cannot be written: %v", err)
	}
	if last := jsonString(t, msgs[len(msgs)-1]); last != `{"id":"TXT","kind":"text","text":"hi","from_me":false,"timestamp":"2026-10-18T17:55:04Z"}` {
		t.Errorf("text message = %s, want it byte-identical to before", last)
	}
}

// Logs never carry thumbnail bytes, captions or text, whether the reference is readable or not.
func TestServingPreviewsLogsNoThumbnailOrText(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	_, fake, events := subscribedSession(t)
	fake.emit(messageEvent(ada, "IMG1", false, t0, thumbImage(fakeJPEGThumb, 1, 1)))
	fake.emit(messageEvent(ada, "STK1", false, t0, stickerMessage(true)))
	_ = WireMessages(ada.String(), []store.Message{{ID: "BAD", Kind: store.KindImage, Text: "secret caption", MediaRef: []byte{0xff, 0xff}}})
	events()
	for _, leak := range []string{string(fakeJPEGThumb), b64(fakeJPEGThumb), string(fakePNGThumb), b64(fakePNGThumb), "the venue", "secret caption"} {
		if strings.Contains(buf.String(), leak) {
			t.Errorf("logs carry %q: %s", leak, buf.String())
		}
	}
}

func jsonString(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

// A thumbnail whose bytes are not what its label says (JPEG for an image, PNG for a sticker) is
// left out; a width or height outside 1..MaxSide is left out, so a client can size a box safely.
func TestAThumbnailThatIsNotItsTypeOrAnAbsurdSizeIsOmitted(t *testing.T) {
	sticker := stickerMessage(false)
	sticker.StickerMessage.PngThumbnail = fakeJPEGThumb
	for name, tc := range map[string]struct {
		msg  *waE2E.Message
		want string
	}{
		"image with png bytes":      {thumbImage(fakePNGThumb, 800, 600), `"file_name":""},"width":800,"height":600}`},
		"sticker with jpeg bytes":   {sticker, `"file_name":""},"width":512,"height":512}`},
		"width too large":           {thumbImage(nil, MaxSide+1, 600), `"file_name":""},"height":600}`},
		"height at the bound":       {thumbImage(nil, 800, MaxSide), `"file_name":""},"width":800,"height":16384}`},
		"three bytes of jpeg magic": {thumbImage([]byte{0xff, 0xd8, 0xff}, 1, 1), `"thumb":"/9j/","thumb_mime":"image/jpeg","width":1,"height":1}`},
	} {
		t.Run(name, func(t *testing.T) {
			_, fake, events := subscribedSession(t)
			fake.emit(messageEvent(ada, "IMG1", false, t0, tc.msg))
			got := events()
			if len(got) == 0 || !strings.HasSuffix(got[0], tc.want+"}") {
				t.Fatalf("published %q, want it to end with %s}", got, tc.want)
			}
		})
	}
}
