package wa

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// mediaLogID is the message id of every media log test: the log must never name it.
const mediaLogID = "3EB0MEDIASECRET"

// assertMediaLog checks that the log holds want and neither the message id nor the chat's JID.
func assertMediaLog(t *testing.T, got, want string) {
	t.Helper()
	if !strings.Contains(got, want) {
		t.Errorf("log = %q, want %q", got, want)
	}
	for _, leak := range []string{mediaLogID, "15551230001", "mmg.example", "permission denied"} {
		if strings.Contains(got, leak) {
			t.Errorf("log = %q carries %q", got, leak)
		}
	}
}

func TestAMediaTheCacheCannotHoldIsLoggedWithoutItsMessageID(t *testing.T) {
	s, fake, _ := startedSession(t)
	parent := filepath.Join(t.TempDir(), "ro")
	if err := os.Mkdir(parent, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(parent, 0o700) })
	s.SetMedia(filepath.Join(parent, "media"), &fakeDownloader{plain: mediaBytes})
	fake.emit(messageEvent(ada, mediaLogID, false, t0, imageMessage(sum(mediaBytes))))
	logs := captureLog(t)
	if _, _, err := s.DownloadMedia(ada.String(), mediaLogID); !errors.Is(err, ErrMediaCache) {
		t.Fatalf("DownloadMedia = %v, want ErrMediaCache", err)
	}
	// The type of the innermost cause (the read-only directory's errno), never "<nil>" nor a
	// wrapper's type.
	assertMediaLog(t, logs.String(), "wa: media could not be cached (syscall.Errno)")
}

func TestAMediaWithTheWrongSHA256IsLoggedWithoutItsMessageID(t *testing.T) {
	s, fake, _, _ := mediaSession(t)
	fake.emit(messageEvent(ada, mediaLogID, false, t0, imageMessage(sum([]byte("other bytes")))))
	logs := captureLog(t)
	if _, _, err := s.DownloadMedia(ada.String(), mediaLogID); !errors.Is(err, ErrMediaCorrupt) {
		t.Fatalf("DownloadMedia = %v, want ErrMediaCorrupt", err)
	}
	assertMediaLog(t, logs.String(), "wa: media does not match its SHA-256; nothing was cached")
}

func TestAMediaThatFailsToDownloadIsLoggedWithoutItsMessageID(t *testing.T) {
	s, fake, dl, _ := mediaSession(t)
	dl.err = fmt.Errorf("media host said 404 for https://mmg.example/v/t62.fake/0001: %w", mediaHostError{})
	fake.emit(messageEvent(ada, mediaLogID, false, t0, imageMessage(sum(mediaBytes))))
	logs := captureLog(t)
	if _, _, err := s.DownloadMedia(ada.String(), mediaLogID); !errors.Is(err, ErrMediaNotDownloaded) {
		t.Fatalf("DownloadMedia = %v, want ErrMediaNotDownloaded", err)
	}
	assertMediaLog(t, logs.String(), "wa: media was not downloaded (wa.mediaHostError)")
}

// mediaHostError is a download failure's innermost cause, as whatsmeow wraps its HTTP errors.
type mediaHostError struct{}

func (mediaHostError) Error() string { return "404 from mmg.example" }
