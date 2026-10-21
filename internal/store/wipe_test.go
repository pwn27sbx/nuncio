package store

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/media"
)

// populatedDataDir is a nuncio data dir as a running daemon leaves it: the database with its WAL
// and SHM files, a media cache and an exports dir, each holding a file.
func populatedDataDir(t *testing.T) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "nuncio")
	db, err := Open(filepath.Join(dir, DBFile))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	// SQLite removes the WAL and SHM files on a clean close, so they are recreated as a crashed
	// daemon would leave them.
	for _, name := range []string{DBFile + "-wal", DBFile + "-shm"} {
		writeFile(t, filepath.Join(dir, name))
	}
	writeFile(t, filepath.Join(dir, MediaDir, "0f3a.jpg"))
	writeFile(t, filepath.Join(dir, ExportsDir, "chat.txt"))
	return dir
}

func writeFile(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("synthetic"), 0o600); err != nil {
		t.Fatal(err)
	}
}

// fullKeyring holds every device item under the nuncio service.
func fullKeyring(t *testing.T) *fakeKeyring {
	t.Helper()
	kr := newFakeKeyring()
	for _, name := range keyItems {
		if err := kr.Set(KeyringService, name, "synthetic"); err != nil {
			t.Fatal(err)
		}
	}
	return kr
}

func assertGone(t *testing.T, path string) {
	t.Helper()
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Errorf("%s still exists after Wipe (lstat: %v)", path, err)
	}
}

func TestWipeRemovesTheDatabaseItsWALAndSHM(t *testing.T) {
	dir := populatedDataDir(t)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	for _, name := range []string{DBFile, DBFile + "-wal", DBFile + "-shm"} {
		assertGone(t, filepath.Join(dir, name))
	}
}

func TestWipeRemovesTheMediaCacheAndExports(t *testing.T) {
	dir := populatedDataDir(t)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	assertGone(t, filepath.Join(dir, MediaDir))
	assertGone(t, filepath.Join(dir, ExportsDir))
	// Nothing else was there, so the data dir itself goes too.
	assertGone(t, dir)
}

func TestWipeEmptiesTheKeyring(t *testing.T) {
	kr := fullKeyring(t)
	if err := Wipe(populatedDataDir(t), kr); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	if n := kr.count(); n != 0 {
		t.Errorf("keyring holds %d items after Wipe, want 0", n)
	}
}

func TestWipeTwiceIsNotAnError(t *testing.T) {
	dir := populatedDataDir(t)
	kr := fullKeyring(t)
	if err := Wipe(dir, kr); err != nil {
		t.Fatalf("first Wipe: %v", err)
	}
	if err := Wipe(dir, kr); err != nil {
		t.Errorf("second Wipe = %v, want nil", err)
	}
}

// A symlink inside the data dir is removed as a link; what it points at, outside the data dir,
// is never touched.
func TestWipeNeverDeletesOutsideTheDataDir(t *testing.T) {
	dir := populatedDataDir(t)
	outside := t.TempDir()
	keep := filepath.Join(outside, "keep.txt")
	writeFile(t, keep)
	writeFile(t, filepath.Join(outside, "nuncio.db"))
	if err := os.RemoveAll(filepath.Join(dir, MediaDir)); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dir, MediaDir)); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(filepath.Join(dir, DBFile+"-wal")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(outside, "nuncio.db"), filepath.Join(dir, DBFile+"-wal")); err != nil {
		t.Fatal(err)
	}

	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	for _, p := range []string{keep, filepath.Join(outside, "nuncio.db")} {
		if _, err := os.Stat(p); err != nil {
			t.Errorf("%s outside the data dir was touched: %v", p, err)
		}
	}
	assertGone(t, filepath.Join(dir, MediaDir))
	assertGone(t, filepath.Join(dir, DBFile+"-wal"))
}

// A data dir that is itself a symlink is refused: wiping through it would delete files somewhere
// else.
func TestWipeRefusesADataDirThatIsASymlink(t *testing.T) {
	real := populatedDataDir(t)
	link := filepath.Join(t.TempDir(), "nuncio")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	kr := fullKeyring(t)
	if err := Wipe(link, kr); err == nil {
		t.Fatal("Wipe through a symlinked data dir = nil, want an error")
	}
	if _, err := os.Stat(filepath.Join(real, DBFile)); err != nil {
		t.Errorf("the database behind the link was touched: %v", err)
	}
	if n := kr.count(); n != len(keyItems) {
		t.Errorf("keyring holds %d items after a refused Wipe, want %d", n, len(keyItems))
	}
}

// Files nuncio does not own are left alone, and so is the data dir that holds them.
func TestWipeKeepsFilesItDoesNotOwn(t *testing.T) {
	dir := populatedDataDir(t)
	other := filepath.Join(dir, "notes.txt")
	writeFile(t, other)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	if _, err := os.Stat(other); err != nil {
		t.Errorf("%s was deleted: %v", other, err)
	}
	assertGone(t, filepath.Join(dir, DBFile))
}

// wipeMedia stands in for an attachment: only the fields whatsmeow's Download reads.
type wipeMedia struct{ sum []byte }

func (m wipeMedia) GetDirectPath() string    { return "/v/t62.fake/0000" }
func (m wipeMedia) GetMediaKey() []byte      { return bytes.Repeat([]byte{7}, 32) }
func (m wipeMedia) GetFileSHA256() []byte    { return m.sum }
func (m wipeMedia) GetFileEncSHA256() []byte { return bytes.Repeat([]byte{9}, 32) }

var wipePlain = []byte("fake decrypted attachment bytes")

func wipeRequest(id string) media.Request {
	sum := sha256.Sum256(wipePlain)
	return media.Request{ChatJID: "10000000001@s.whatsapp.net", MessageID: id, Ext: "jpg",
		Media: wipeMedia{sum: sum[:]}}
}

// gatedDownloader returns the known bytes; when gate is set it first reports the call on started
// and blocks until gate is closed.
type gatedDownloader struct {
	started chan struct{}
	gate    chan struct{}
	calls   atomic.Int32
}

func (g *gatedDownloader) Download(_ context.Context, _ whatsmeow.DownloadableMessage) ([]byte, error) {
	g.calls.Add(1)
	if g.gate != nil {
		close(g.started)
		<-g.gate
	}
	return wipePlain, nil
}

func TestADownloadStillCachesIntoALiveDataDir(t *testing.T) {
	dir := populatedDataDir(t)
	cache := media.New(filepath.Join(dir, MediaDir), &gatedDownloader{})
	path, err := cache.Fetch(context.Background(), wipeRequest("3EB0FAKE01"))
	if err != nil {
		t.Fatalf("Fetch: %v", err)
	}
	if got, err := os.ReadFile(path); err != nil || !bytes.Equal(got, wipePlain) {
		t.Fatalf("cached file = %q, %v; want the downloaded bytes", got, err)
	}
	if filepath.Dir(path) != filepath.Join(dir, MediaDir) {
		t.Errorf("cached at %s, want inside %s", path, filepath.Join(dir, MediaDir))
	}
}

func TestADownloadAfterAWipeRecreatesNothing(t *testing.T) {
	dir := populatedDataDir(t)
	dl := &gatedDownloader{}
	cache := media.New(filepath.Join(dir, MediaDir), dl)
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	if path, err := cache.Fetch(context.Background(), wipeRequest("3EB0FAKE02")); err == nil {
		t.Errorf("Fetch after Wipe = %s, nil; want an error", path)
	} else if !errors.Is(err, media.ErrClosed) {
		t.Errorf("Fetch after Wipe = %v; want media.ErrClosed", err)
	}
	if n := dl.calls.Load(); n != 0 {
		t.Errorf("Fetch after Wipe downloaded %d times, want 0", n)
	}
	assertGone(t, dir)
}

func TestADownloadRacingAWipeLeavesNothingBehind(t *testing.T) {
	dir := populatedDataDir(t)
	dl := &gatedDownloader{started: make(chan struct{}), gate: make(chan struct{})}
	cache := media.New(filepath.Join(dir, MediaDir), dl)
	done := make(chan error, 1)
	go func() {
		_, err := cache.Fetch(context.Background(), wipeRequest("3EB0FAKE03"))
		done <- err
	}()
	<-dl.started // the download is in flight
	if err := Wipe(dir, newFakeKeyring()); err != nil {
		t.Fatalf("Wipe: %v", err)
	}
	close(dl.gate) // the bytes arrive only after the wipe has run
	if err := <-done; err == nil {
		t.Error("Fetch racing a Wipe = nil; want an error")
	} else if !errors.Is(err, media.ErrClosed) {
		t.Errorf("Fetch racing a Wipe = %v; want media.ErrClosed", err)
	}
	assertGone(t, dir)
}

// undeletableDataDir makes the final rmdir of the data dir fail with errno, as it does under the
// packaged systemd unit (ReadWritePaths bind-mounts the dir onto itself inside a read-only tree).
func undeletableDataDir(t *testing.T, errno error) {
	t.Helper()
	orig := removeDataDir
	removeDataDir = func(name string) error { return &os.PathError{Op: "remove", Path: name, Err: errno} }
	t.Cleanup(func() { removeDataDir = orig })
}

// Under the sandbox the data dir is a mount point (EBUSY) in a read-only tree (EROFS): its
// contents are gone and the dir itself cannot go, which is a complete wipe.
func TestWipeAcceptsAnEmptyDataDirTheSandboxKeeps(t *testing.T) {
	for _, errno := range []error{syscall.EBUSY, syscall.EROFS} {
		t.Run(errno.Error(), func(t *testing.T) {
			dir := populatedDataDir(t)
			kr := fullKeyring(t)
			undeletableDataDir(t, errno)
			if err := Wipe(dir, kr); err != nil {
				t.Fatalf("Wipe = %v, want nil", err)
			}
			entries, err := os.ReadDir(dir)
			if err != nil {
				t.Fatalf("read the kept data dir: %v", err)
			}
			if len(entries) != 0 {
				t.Errorf("the kept data dir still holds %d entries", len(entries))
			}
			if n := kr.count(); n != 0 {
				t.Errorf("keyring holds %d items after Wipe, want 0", n)
			}
		})
	}
}

// Any other failure to remove the emptied data dir is still reported. EPERM and EACCES are not
// what the sandbox does: they mean a parent nuncio cannot write to, a real permission problem.
func TestWipeReportsAnOtherFailureToRemoveTheDataDir(t *testing.T) {
	for _, errno := range []syscall.Errno{syscall.EIO, syscall.EPERM, syscall.EACCES} {
		t.Run(errno.Error(), func(t *testing.T) {
			dir := populatedDataDir(t)
			undeletableDataDir(t, errno)
			err := Wipe(dir, newFakeKeyring())
			if !errors.Is(err, errno) {
				t.Fatalf("Wipe = %v, want the %v from removing the data dir", err, errno)
			}
			if !strings.Contains(err.Error(), "store: remove data dir: ") {
				t.Errorf("Wipe = %q, want it to say it could not remove the data dir", err)
			}
		})
	}
}

// A file of nuncio's that could not be deleted is a failure, even when the data dir itself is one
// the sandbox keeps.
func TestWipeReportsAFileItCouldNotDeleteUnderTheSandbox(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions")
	}
	dir := populatedDataDir(t)
	locked := filepath.Join(dir, MediaDir, "ab")
	stuck := filepath.Join(locked, "0f3b.jpg")
	writeFile(t, stuck)
	if err := os.Chmod(locked, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o700) })
	undeletableDataDir(t, syscall.EBUSY)

	if err := Wipe(dir, newFakeKeyring()); err == nil {
		t.Fatal("Wipe = nil, want an error for the file it could not delete")
	}
	if _, err := os.Lstat(stuck); err != nil {
		t.Fatalf("precondition: %s should still exist: %v", stuck, err)
	}
}

// A kept data dir whose emptiness cannot be checked is not accepted as wiped.
func TestWipeReportsAKeptDataDirItCannotRead(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root ignores directory permissions")
	}
	dir := populatedDataDir(t)
	orig := removeDataDir
	removeDataDir = func(name string) error {
		if err := os.Chmod(name, 0o300); err != nil {
			t.Fatal(err)
		}
		return &os.PathError{Op: "remove", Path: name, Err: syscall.EBUSY}
	}
	t.Cleanup(func() { removeDataDir = orig; _ = os.Chmod(dir, 0o700) })
	if err := Wipe(dir, newFakeKeyring()); err == nil {
		t.Fatal("Wipe = nil, want an error: the kept data dir could not be read")
	}
}
