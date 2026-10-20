package media

import (
	"context"
	"crypto/sha256"
	"encoding/hex"

	"go.mau.fi/whatsmeow"
)

// Downloader fetches and decrypts one attachment. *whatsmeow.Client
// implements it with Client.Download.
type Downloader interface {
	Download(ctx context.Context, msg whatsmeow.DownloadableMessage) ([]byte, error)
}

var _ Downloader = (*whatsmeow.Client)(nil)

// Request names one attachment. ChatJID and MessageID only key the cache
// file; they never become a path component. Ext is a hint such as "jpg" and
// is dropped unless it is short and alphanumeric.
type Request struct {
	ChatJID   string
	MessageID string
	Ext       string
	// Legacy is the Ext an earlier build asked for. Before downloading, the file it cached (named
	// as fileName names it, so the bare hash when Legacy is not safe) is renamed to the current name
	// and served, instead of fetched again.
	Legacy string
	Media  whatsmeow.DownloadableMessage
}

// fileName derives the cache file name from the chat and message ids: a hex
// SHA-256, so no id can name a path component. A safe Ext is appended.
func fileName(req Request) string { return withExt(req, req.Ext) }

// withExt is the cache file name of req with ext, appended only when it is safe.
func withExt(req Request, ext string) string {
	sum := sha256.Sum256([]byte(req.ChatJID + "\x00" + req.MessageID))
	name := hex.EncodeToString(sum[:])
	if SafeExt(ext) {
		name += "." + ext
	}
	return name
}

// SafeExt reports whether ext may end a cache file name: 1 to 8 ASCII letters or digits.
func SafeExt(ext string) bool {
	if ext == "" || len(ext) > 8 {
		return false
	}
	for _, r := range ext {
		if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9') {
			return false
		}
	}
	return true
}
