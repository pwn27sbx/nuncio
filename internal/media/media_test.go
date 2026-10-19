package media_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"os"
	"path/filepath"
	"testing"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/media"
)

// fakeMedia is a stand-in for an ImageMessage: only the fields whatsmeow's
// Download reads.
type fakeMedia struct{ fileSHA256 []byte }

func (m fakeMedia) GetDirectPath() string    { return "/v/t62.fake/0000" }
func (m fakeMedia) GetMediaKey() []byte      { return bytes.Repeat([]byte{7}, 32) }
func (m fakeMedia) GetFileSHA256() []byte    { return m.fileSHA256 }
func (m fakeMedia) GetFileEncSHA256() []byte { return bytes.Repeat([]byte{9}, 32) }

// fakeDownloader stands in for whatsmeow's network fetch + media-key
// decryption and returns known plaintext.
type fakeDownloader struct {
	plain []byte
	calls int
}

func (f *fakeDownloader) Download(_ context.Context, _ whatsmeow.DownloadableMessage) ([]byte, error) {
	f.calls++
	return f.plain, nil
}

var known = []byte("fake decrypted attachment bytes")

func hashOf(b []byte) []byte { s := sha256.Sum256(b); return s[:] }

func request(id string) media.Request {
	return media.Request{ChatJID: "10000000001@s.whatsapp.net", MessageID: id, Ext: "jpg",
		Media: fakeMedia{fileSHA256: hashOf(known)}}
}

func entries(t *testing.T, dir string) []string {
	t.Helper()
	des, err := os.ReadDir(dir)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		t.Fatal(err)
	}
	var names []string
	for _, de := range des {
		names = append(names, de.Name())
	}
	return names
}

func mode(t *testing.T, p string) os.FileMode {
	t.Helper()
	fi, err := os.Stat(p)
	if err != nil {
		t.Fatal(err)
	}
	return fi.Mode().Perm()
}

func TestNewFetchesNothingWithoutARequest(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "media")
	dl := &fakeDownloader{plain: known}
	_ = media.New(dir, dl)
	if dl.calls != 0 {
		t.Fatalf("downloader called %d times without a request", dl.calls)
	}
	if _, err := os.Stat(dir); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("New touched the disk: %v", err)
	}
}

func TestFetchCachesTheDecryptedBytes(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "media")
	p, err := media.New(dir, &fakeDownloader{plain: known}).Fetch(context.Background(), request("3EB0AAAA"))
	if err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(p)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, known) {
		t.Fatalf("cached %q, want %q", got, known)
	}
}

func TestFetchRejectsSHA256MismatchAndCachesNothing(t *testing.T) {
	for name, want := range map[string][]byte{
		"mismatch": hashOf([]byte("other bytes")),
		"missing":  nil,
	} {
		t.Run(name, func(t *testing.T) {
			dir := filepath.Join(t.TempDir(), "media")
			req := request("3EB0BBBB")
			req.Media = fakeMedia{fileSHA256: want}
			p, err := media.New(dir, &fakeDownloader{plain: known}).Fetch(context.Background(), req)
			if !errors.Is(err, media.ErrSHA256Mismatch) {
				t.Fatalf("err = %v, want ErrSHA256Mismatch", err)
			}
			if p != "" {
				t.Fatalf("path %q returned on error", p)
			}
			if names := entries(t, dir); len(names) != 0 {
				t.Fatalf("cache dir holds %v after a mismatch", names)
			}
		})
	}
}

func TestFetchUsesPrivateModes(t *testing.T) {
	for name, pre := range map[string]bool{"fresh dir": false, "existing 0755 dir": true} {
		t.Run(name, func(t *testing.T) {
			dir := filepath.Join(t.TempDir(), "media")
			if pre {
				if err := os.Mkdir(dir, 0o755); err != nil {
					t.Fatal(err)
				}
				if err := os.Chmod(dir, 0o755); err != nil {
					t.Fatal(err)
				}
			}
			p, err := media.New(dir, &fakeDownloader{plain: known}).Fetch(context.Background(), request("3EB0CCCC"))
			if err != nil {
				t.Fatal(err)
			}
			if m := mode(t, dir); m != 0o700 {
				t.Fatalf("dir mode %o, want 700", m)
			}
			if m := mode(t, p); m != 0o600 {
				t.Fatalf("file mode %o, want 600", m)
			}
		})
	}
}

func TestSecondFetchIsServedFromTheCache(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "media")
	dl := &fakeDownloader{plain: known}
	c := media.New(dir, dl)
	first, err := c.Fetch(context.Background(), request("3EB0DDDD"))
	if err != nil {
		t.Fatal(err)
	}
	second, err := c.Fetch(context.Background(), request("3EB0DDDD"))
	if err != nil {
		t.Fatal(err)
	}
	if dl.calls != 1 {
		t.Fatalf("downloader called %d times, want 1", dl.calls)
	}
	if first != second {
		t.Fatalf("paths differ: %q vs %q", first, second)
	}
}

func TestUnsafeIDsCannotEscapeTheCacheDir(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "media")
	req := request("../x")
	req.ChatJID = "../../y"
	req.Ext = "/../../z"
	p, err := media.New(dir, &fakeDownloader{plain: known}).Fetch(context.Background(), req)
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Dir(p) != dir {
		t.Fatalf("cached at %q, outside %q", p, dir)
	}
	if names := entries(t, root); len(names) != 1 || names[0] != "media" {
		t.Fatalf("data dir holds %v, want only media", names)
	}
}
