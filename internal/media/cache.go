package media

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
)

// ErrSHA256Mismatch means the downloaded bytes do not hash to the SHA-256
// the message carries (or it carries none); nothing is cached.
var ErrSHA256Mismatch = errors.New("media: sha256 mismatch")

// ErrClosed: a wipe closed the cache dir, so nothing was downloaded or written.
var ErrClosed = errors.New("media: the cache was wiped")

// ErrCache: the cache dir or a file in it could not be written (a local disk failure, not a
// download failure). The error's own text still says what failed.
var ErrCache = errors.New("media: the cache could not be written")

// cacheErr marks a local cache failure as ErrCache and keeps its text.
type cacheErr struct{ error }

func (e cacheErr) Is(target error) bool { return target == ErrCache }
func (e cacheErr) Unwrap() error        { return e.error }

// closing guards closed: every disk write of every Cache holds it, and Close takes it, so a wipe
// never runs while a write is creating the cache dir or a file in it.
var (
	closing sync.Mutex
	closed  = map[string]bool{}
)

// Close marks the cache dir dir closed for the rest of the process: it waits for a write already
// under way, and from then on every Fetch over dir fails with ErrClosed and creates nothing, not
// even dir or its parent. store.Wipe calls it before deleting the media cache, so a download
// racing a wipe cannot recreate the data dir.
func Close(dir string) {
	closing.Lock()
	closed[filepath.Clean(dir)] = true
	closing.Unlock()
}

func isClosed(dir string) bool {
	return closed[filepath.Clean(dir)]
}

// Cache serves attachments from dir, downloading each one at most once.
type Cache struct {
	dir string
	dl  Downloader
}

// New returns a cache over dir. It touches neither the disk nor the network.
func New(dir string, dl Downloader) *Cache {
	return &Cache{dir: dir, dl: dl}
}

// Fetch returns the path of the decrypted attachment, downloading it only if
// it is not cached yet under its Ext; a file an earlier build cached under its Legacy name is
// renamed to the Ext name and served. The bytes are written only after their SHA-256
// matches req.Media.GetFileSHA256(). Every Fetch first creates the cache dir and makes it 0700 (a
// cache hit too), so a dir that cannot be created fails before any download; once Close closed
// it, Fetch fails with ErrClosed and creates nothing.
func (c *Cache) Fetch(ctx context.Context, req Request) (string, error) {
	path := filepath.Join(c.dir, fileName(req))
	if err := c.prepare(); err != nil {
		if errors.Is(err, ErrClosed) {
			return "", fmt.Errorf("%w: %s not fetched", ErrClosed, req.MessageID)
		}
		return "", cacheErr{fmt.Errorf("media: %w", err)}
	}
	if fi, err := os.Lstat(path); err == nil && fi.Mode().IsRegular() {
		return path, nil
	}
	if old := filepath.Join(c.dir, withExt(req, req.Legacy)); old != path {
		adopted, err := c.adopt(old, path)
		if err != nil {
			if errors.Is(err, ErrClosed) {
				return "", fmt.Errorf("%w: %s not fetched", ErrClosed, req.MessageID)
			}
			return "", cacheErr{fmt.Errorf("media: %w", err)}
		}
		if adopted {
			return path, nil
		}
	}
	data, err := c.dl.Download(ctx, req.Media)
	if err != nil {
		return "", fmt.Errorf("media: download %s: %w", req.MessageID, err)
	}
	// whatsmeow already checks this hash after decrypting; checking again
	// here keeps the cache honest whatever the Downloader is.
	want := req.Media.GetFileSHA256()
	if sum := sha256.Sum256(data); len(want) != sha256.Size || !bytes.Equal(sum[:], want) {
		return "", fmt.Errorf("%w for %s", ErrSHA256Mismatch, req.MessageID)
	}
	if err := c.store(path, data); err != nil {
		err = fmt.Errorf("media: cache %s: %w", req.MessageID, err)
		if errors.Is(err, ErrClosed) {
			return "", err
		}
		return "", cacheErr{err}
	}
	return path, nil
}

// adopt moves a regular file an earlier build cached at old to path (mode 0600), under the lock and
// unless Close closed the dir, and reports whether path now holds it. A path that already exists is
// kept, and old left alone; a missing old, or one that is not a regular file, adopts nothing.
func (c *Cache) adopt(old, path string) (bool, error) {
	closing.Lock()
	defer closing.Unlock()
	if isClosed(c.dir) {
		return false, ErrClosed
	}
	if fi, err := os.Lstat(path); err == nil {
		return fi.Mode().IsRegular(), nil
	}
	fi, err := os.Lstat(old)
	if err != nil || !fi.Mode().IsRegular() {
		return false, nil
	}
	if err := os.Rename(old, path); err != nil {
		return false, fmt.Errorf("adopt a cached file: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return false, fmt.Errorf("restrict a cached file: %w", err)
	}
	return true, nil
}

// prepare creates the cache dir and makes it 0700, under the lock, unless Close closed it.
func (c *Cache) prepare() error {
	closing.Lock()
	defer closing.Unlock()
	if isClosed(c.dir) {
		return ErrClosed
	}
	return c.makeDir()
}

// makeDir creates the cache dir and makes it 0700; the caller holds closing.
func (c *Cache) makeDir() error {
	if err := os.MkdirAll(c.dir, 0o700); err != nil {
		return fmt.Errorf("create cache dir: %w", err)
	}
	if err := os.Chmod(c.dir, 0o700); err != nil {
		return fmt.Errorf("restrict cache dir: %w", err)
	}
	return nil
}

// store writes data to path, unless Close closed the dir meanwhile (the download is not held under
// the lock, so a wipe never waits for the network).
func (c *Cache) store(path string, data []byte) error {
	closing.Lock()
	defer closing.Unlock()
	if isClosed(c.dir) {
		return ErrClosed
	}
	if err := c.makeDir(); err != nil {
		return err
	}
	return writePrivate(c.dir, path, data)
}

// writePrivate writes data to a 0600 temp file in dir and renames it to
// path, so a reader never sees a partial file.
func writePrivate(dir, path string, data []byte) error {
	tmp, err := os.CreateTemp(dir, ".part-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name()) // no-op after a successful rename
	if err := tmp.Chmod(0o600); err != nil {
		tmp.Close()
		return err
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}
