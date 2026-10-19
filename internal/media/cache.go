package media

import (
	"bytes"
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// ErrSHA256Mismatch means the downloaded bytes do not hash to the SHA-256
// the message carries (or it carries none); nothing is cached.
var ErrSHA256Mismatch = errors.New("media: sha256 mismatch")

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
// it is not cached yet. The bytes are written only after their SHA-256
// matches req.Media.GetFileSHA256().
func (c *Cache) Fetch(ctx context.Context, req Request) (string, error) {
	if err := os.MkdirAll(c.dir, 0o700); err != nil {
		return "", fmt.Errorf("media: create cache dir: %w", err)
	}
	if err := os.Chmod(c.dir, 0o700); err != nil {
		return "", fmt.Errorf("media: restrict cache dir: %w", err)
	}
	path := filepath.Join(c.dir, fileName(req))
	if fi, err := os.Lstat(path); err == nil && fi.Mode().IsRegular() {
		return path, nil
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
	if err := writePrivate(c.dir, path, data); err != nil {
		return "", fmt.Errorf("media: cache %s: %w", req.MessageID, err)
	}
	return path, nil
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
