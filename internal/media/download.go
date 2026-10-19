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
	Media     whatsmeow.DownloadableMessage
}

// fileName derives the cache file name from the chat and message ids: a hex
// SHA-256, so no id can name a path component. A safe Ext is appended.
func fileName(req Request) string {
	sum := sha256.Sum256([]byte(req.ChatJID + "\x00" + req.MessageID))
	name := hex.EncodeToString(sum[:])
	if safeExt(req.Ext) {
		name += "." + req.Ext
	}
	return name
}

func safeExt(ext string) bool {
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
