package wa

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/media"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// fakeDownloader stands in for whatsmeow's Client.Download, which fetches the ciphertext and
// decrypts it with the media key (proto/wire.md and internal/media delegate both to whatsmeow), so
// it returns the known decrypted bytes. It records what it was asked for and never touches the
// network.
type fakeDownloader struct {
	mu    sync.Mutex
	plain []byte
	err   error
	asked []whatsmeow.DownloadableMessage
}

func (f *fakeDownloader) Download(_ context.Context, m whatsmeow.DownloadableMessage) ([]byte, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.asked = append(f.asked, m)
	return f.plain, f.err
}

func (f *fakeDownloader) calls() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.asked)
}

var mediaBytes = []byte("fake decrypted photo bytes")

func sum(b []byte) []byte { s := sha256.Sum256(b); return s[:] }

func imageMessage(fileSHA []byte) *waE2E.Message {
	return &waE2E.Message{ImageMessage: &waE2E.ImageMessage{
		Caption:       proto.String("the venue"),
		Mimetype:      proto.String("image/jpeg"),
		FileLength:    proto.Uint64(uint64(len(mediaBytes))),
		DirectPath:    proto.String("/v/t62.fake/0001"),
		MediaKey:      bytes.Repeat([]byte{7}, 32),
		FileSHA256:    fileSHA,
		FileEncSHA256: bytes.Repeat([]byte{9}, 32),
	}}
}

// mediaSession is a linked session over a real store with a media cache in a temp dir served by
// a fake downloader.
func mediaSession(t *testing.T) (*Session, *fakeClient, *fakeDownloader, string) {
	t.Helper()
	s, fake, _ := startedSession(t)
	dl := &fakeDownloader{plain: mediaBytes}
	dir := filepath.Join(t.TempDir(), "media")
	s.SetMedia(dir, dl)
	return s, fake, dl, dir
}

func TestIncomingMediaIsStoredWithItsDescriptionAndReference(t *testing.T) {
	doc := &waE2E.DocumentMessage{Caption: proto.String("the menu"), Mimetype: proto.String("application/pdf"),
		FileLength: proto.Uint64(1200), FileName: proto.String("menu.pdf"), DirectPath: proto.String("/v/t62.fake/0002"),
		MediaKey: bytes.Repeat([]byte{1}, 32), FileSHA256: sum([]byte("pdf"))}
	audio := &waE2E.AudioMessage{Mimetype: proto.String("audio/ogg; codecs=opus"), FileLength: proto.Uint64(3300),
		DirectPath: proto.String("/v/t62.fake/0003"), MediaKey: bytes.Repeat([]byte{2}, 32)}
	video := &waE2E.VideoMessage{Caption: proto.String("look"), Mimetype: proto.String("video/mp4"),
		FileLength: proto.Uint64(9000), DirectPath: proto.String("/v/t62.fake/0004")}
	img := imageMessage(sum(mediaBytes))
	for _, tc := range []struct {
		name string
		msg  *waE2E.Message
		sub  proto.Message
		want store.Message
	}{
		{"image", img, img.GetImageMessage(), store.Message{Kind: store.KindImage, Text: "the venue", Mime: "image/jpeg", Size: int64(len(mediaBytes))}},
		{"document", &waE2E.Message{DocumentMessage: doc}, doc, store.Message{Kind: store.KindDocument, Text: "the menu", Mime: "application/pdf", Size: 1200, FileName: "menu.pdf"}},
		{"audio", &waE2E.Message{AudioMessage: audio}, audio, store.Message{Kind: store.KindAudio, Mime: "audio/ogg; codecs=opus", Size: 3300}},
		{"video", &waE2E.Message{VideoMessage: video}, video, store.Message{Kind: store.KindVideo, Text: "look", Mime: "video/mp4", Size: 9000}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, dl, dir := mediaSession(t)
			fake.emit(messageEvent(ada, "M1", false, t0, tc.msg))

			got, err := store.GetMessage(s.Store(), ada.String(), "M1")
			if err != nil {
				t.Fatalf("GetMessage: %v", err)
			}
			if got.Kind != tc.want.Kind || got.Text != tc.want.Text || got.Mime != tc.want.Mime ||
				got.Size != tc.want.Size || got.FileName != tc.want.FileName {
				t.Errorf("stored %+v, want %+v", got, tc.want)
			}
			ref := proto.Clone(tc.sub)
			proto.Reset(ref)
			if err := proto.Unmarshal(got.MediaRef, ref); err != nil || !proto.Equal(ref, tc.sub) {
				t.Errorf("stored reference decodes to %v (%v), want %v", ref, err, tc.sub)
			}
			if n := unread(t, s.Store(), ada.String()); n != 1 {
				t.Errorf("unread = %d, want 1", n)
			}
			// Storing is not a request: nothing is downloaded and the cache is not created.
			if n := dl.calls(); n != 0 {
				t.Errorf("storing a media message called the downloader %d times, want 0", n)
			}
			if _, err := os.Stat(dir); !errors.Is(err, os.ErrNotExist) {
				t.Errorf("storing a media message touched the cache dir: %v", err)
			}
		})
	}
}

func TestAStoredMediaMessagePublishesItsMedia(t *testing.T) {
	_, fake, events := subscribedSession(t)
	fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum(mediaBytes))))
	assertEvents(t, events(),
		`{"v":1,"op":"event_message","jid":"15551230001@s.whatsapp.net","message":{"id":"IMG1","kind":"image","text":"the venue","from_me":false,"timestamp":"2026-10-18T17:55:04Z","media":{"mime":"image/jpeg","size":26,"file_name":""}}}`,
		`{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":1}`,
	)
}

func modeOf(t *testing.T, p string) os.FileMode {
	t.Helper()
	fi, err := os.Stat(p)
	if err != nil {
		t.Fatal(err)
	}
	return fi.Mode().Perm()
}

func TestDownloadMediaCachesTheVerifiedBytesOnce(t *testing.T) {
	s, fake, dl, dir := mediaSession(t)
	msg := imageMessage(sum(mediaBytes))
	fake.emit(messageEvent(ada, "IMG1", false, t0, msg))

	path, mime, err := s.DownloadMedia(ada.String(), "IMG1")
	if err != nil {
		t.Fatalf("DownloadMedia: %v", err)
	}
	if mime != "image/jpeg" || filepath.Dir(path) != dir {
		t.Errorf("DownloadMedia = %q, %q; want a file in %s and image/jpeg", path, mime, dir)
	}
	if got, err := os.ReadFile(path); err != nil || !bytes.Equal(got, mediaBytes) {
		t.Errorf("cached bytes = %q, %v; want %q", got, err, mediaBytes)
	}
	if m := modeOf(t, dir); m != 0o700 {
		t.Errorf("cache dir mode %o, want 700", m)
	}
	if m := modeOf(t, path); m != 0o600 {
		t.Errorf("cached file mode %o, want 600", m)
	}
	// The downloader got the stored reference back: key, path and hashes.
	if dl.calls() != 1 || !proto.Equal(dl.asked[0].(proto.Message), msg.GetImageMessage()) {
		t.Fatalf("downloader asked %v, want the stored image once", dl.asked)
	}

	again, _, err := s.DownloadMedia(ada.String(), "IMG1")
	if err != nil || again != path {
		t.Errorf("second DownloadMedia = %q, %v; want %q", again, err, path)
	}
	if n := dl.calls(); n != 1 {
		t.Errorf("downloader called %d times for two requests, want 1 (the second is a cache hit)", n)
	}
}

func TestDownloadMediaRejectsASHA256MismatchAndCachesNothing(t *testing.T) {
	s, fake, _, dir := mediaSession(t)
	fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum([]byte("other bytes")))))

	path, _, err := s.DownloadMedia(ada.String(), "IMG1")
	if !errors.Is(err, ErrMediaCorrupt) || path != "" {
		t.Fatalf("DownloadMedia = %q, %v; want ErrMediaCorrupt and no path", path, err)
	}
	des, err := os.ReadDir(dir)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		t.Fatal(err)
	}
	if len(des) != 0 {
		t.Errorf("cache dir holds %d entries after a mismatch, want none", len(des))
	}
}

func TestDownloadMediaCorners(t *testing.T) {
	t.Run("unknown id and a text message", func(t *testing.T) {
		s, fake, dl, _ := mediaSession(t)
		fake.emit(textEvent(ada, "TXT1", false, t0, "plain"))
		for _, id := range []string{"NOPE", "TXT1"} {
			if _, _, err := s.DownloadMedia(ada.String(), id); !errors.Is(err, ErrNoMedia) {
				t.Errorf("DownloadMedia(%s) = %v, want ErrNoMedia", id, err)
			}
		}
		if n := dl.calls(); n != 0 {
			t.Errorf("downloader called %d times, want 0", n)
		}
	})
	t.Run("unlinked", func(t *testing.T) {
		s := NewSession(nil, nil, nil)
		dl := &fakeDownloader{plain: mediaBytes}
		s.SetMedia(filepath.Join(t.TempDir(), "media"), dl)
		if _, _, err := s.DownloadMedia(ada.String(), "IMG1"); !errors.Is(err, ipc.ErrNotLinked) {
			t.Errorf("DownloadMedia while unlinked = %v, want ErrNotLinked", err)
		}
		if n := dl.calls(); n != 0 {
			t.Errorf("downloader called %d times, want 0", n)
		}
	})
	t.Run("offline", func(t *testing.T) {
		s, fake, dl, dir := mediaSession(t)
		fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum(mediaBytes))))
		fake.emit(&waevents.Disconnected{})
		if _, _, err := s.DownloadMedia(ada.String(), "IMG1"); !errors.Is(err, ErrMediaOffline) {
			t.Errorf("DownloadMedia while offline = %v, want ErrMediaOffline", err)
		}
		if n := dl.calls(); n != 0 {
			t.Errorf("downloader called %d times while offline, want 0", n)
		}
		// Once cached, the file is served offline too: it needs no connection.
		fake.emit(&waevents.Connected{})
		path, _, err := s.DownloadMedia(ada.String(), "IMG1")
		if err != nil {
			t.Fatalf("DownloadMedia once linked: %v", err)
		}
		fake.emit(&waevents.Disconnected{})
		if again, _, err := s.DownloadMedia(ada.String(), "IMG1"); err != nil || again != path {
			t.Errorf("cached DownloadMedia while offline = %q, %v; want %q", again, err, path)
		}
		if filepath.Dir(path) != dir || dl.calls() != 1 {
			t.Errorf("path %q, downloader calls %d; want a file in %s and 1 call", path, dl.calls(), dir)
		}
	})
	t.Run("a download that fails", func(t *testing.T) {
		s, fake, dl, _ := mediaSession(t)
		dl.err = errors.New("media host said 404 for https://mmg.example/v/t62.fake/0001")
		fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum(mediaBytes))))
		_, _, err := s.DownloadMedia(ada.String(), "IMG1")
		if !errors.Is(err, ErrMediaNotDownloaded) {
			t.Errorf("DownloadMedia = %v, want ErrMediaNotDownloaded", err)
		}
		if err != nil && bytes.Contains([]byte(err.Error()), []byte("mmg.example")) {
			t.Errorf("error %q passes whatsmeow's text on", err)
		}
	})
}

// A wipe closes the media cache; a download_media that meets it says so, instead of blaming
// WhatsApp.
func TestDownloadMediaDuringAWipeSaysNuncioIsWiping(t *testing.T) {
	s, fake, dl, dir := mediaSession(t)
	fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum(mediaBytes))))
	media.Close(dir)
	_, _, err := s.DownloadMedia(ada.String(), "IMG1")
	if err == nil || err.Error() != "nuncio is wiping its data, so the media was not downloaded" {
		t.Errorf("DownloadMedia after a wipe = %v; want the wiping message", err)
	}
	if !errors.Is(err, ErrMediaWiped) {
		t.Errorf("DownloadMedia after a wipe = %v; want ErrMediaWiped", err)
	}
	if n := dl.calls(); n != 0 {
		t.Errorf("downloader called %d times, want 0", n)
	}
}

// A media cache nuncio cannot write is nuncio's own failure, not WhatsApp's, and is said so;
// nothing is downloaded.
func TestDownloadMediaIntoAnUnwritableCacheSaysSo(t *testing.T) {
	s, fake, _ := startedSession(t)
	parent := filepath.Join(t.TempDir(), "ro")
	if err := os.Mkdir(parent, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(parent, 0o700) })
	dl := &fakeDownloader{plain: mediaBytes}
	s.SetMedia(filepath.Join(parent, "media"), dl)
	fake.emit(messageEvent(ada, "IMG1", false, t0, imageMessage(sum(mediaBytes))))
	_, _, err := s.DownloadMedia(ada.String(), "IMG1")
	if err == nil || err.Error() != "nuncio could not write its media cache, so the media was not downloaded" {
		t.Errorf("DownloadMedia = %v; want the media cache message", err)
	}
	if n := dl.calls(); n != 0 {
		t.Errorf("downloader called %d times, want 0", n)
	}
}
