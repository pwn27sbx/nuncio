package wa

import (
	"context"
	"errors"
	"fmt"
	"log"
	"strings"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/media"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// mediaTimeout bounds one download; it is shorter than ipc.MediaTimeout, so a client always gets
// the daemon's answer instead of its own timeout.
const mediaTimeout = 60 * time.Second

var (
	// ErrNoMedia: no message with that id is stored in that chat, or it is not a media message.
	ErrNoMedia = errors.New("no media message with that id in that chat")
	// ErrMediaOffline: the attachment is not cached and the session is not connected, so nothing
	// was downloaded.
	ErrMediaOffline = errors.New("the WhatsApp session is offline, so the media was not downloaded")
	// ErrMediaCorrupt: the downloaded bytes do not hash to the SHA-256 the message carries, so
	// nothing was cached.
	ErrMediaCorrupt = errors.New("the downloaded media does not match its SHA-256, so nothing was saved")
	// ErrMediaNotDownloaded: WhatsApp did not deliver the attachment. whatsmeow's own error is not
	// passed on, because it can carry a media URL.
	ErrMediaNotDownloaded = errors.New("WhatsApp did not deliver the media")
)

// mediaMessage is what the four media sub-messages of whatsmeow have in common.
type mediaMessage interface {
	proto.Message
	whatsmeow.DownloadableMessage
	GetMimetype() string
	GetFileLength() uint64
}

// mediaOf returns the media sub-message of msg with its kind, caption and file name, or false
// when msg carries none of the media kinds nuncio stores.
func mediaOf(msg *waE2E.Message) (kind string, sub mediaMessage, caption, fileName string, ok bool) {
	switch {
	case msg.GetImageMessage() != nil:
		m := msg.GetImageMessage()
		return store.KindImage, m, m.GetCaption(), "", true
	case msg.GetVideoMessage() != nil:
		m := msg.GetVideoMessage()
		return store.KindVideo, m, m.GetCaption(), "", true
	case msg.GetAudioMessage() != nil:
		return store.KindAudio, msg.GetAudioMessage(), "", "", true
	case msg.GetDocumentMessage() != nil:
		m := msg.GetDocumentMessage()
		return store.KindDocument, m, m.GetCaption(), m.GetFileName(), true
	}
	return "", nil, "", "", false
}

// decodeRef rebuilds the media sub-message a stored message of kind kept as its MediaRef.
func decodeRef(kind string, ref []byte) (mediaMessage, error) {
	var m mediaMessage
	switch kind {
	case store.KindImage:
		m = &waE2E.ImageMessage{}
	case store.KindVideo:
		m = &waE2E.VideoMessage{}
	case store.KindAudio:
		m = &waE2E.AudioMessage{}
	case store.KindDocument:
		m = &waE2E.DocumentMessage{}
	default:
		return nil, ErrNoMedia
	}
	if err := proto.Unmarshal(ref, m); err != nil {
		return nil, fmt.Errorf("wa: the stored media reference is unreadable: %w", err)
	}
	return m, nil
}

// SetMedia makes the session serve DownloadMedia from a cache in dir, downloading through dl
// (the whatsmeow client outside tests) only while linked. It does no I/O and downloads nothing.
func (s *Session) SetMedia(dir string, dl media.Downloader) {
	c := media.New(dir, linkedOnly{s: s, dl: dl})
	s.mu.Lock()
	s.media = c
	s.mu.Unlock()
}

// linkedOnly downloads only while the session is connected; a cache hit never reaches it.
type linkedOnly struct {
	s  *Session
	dl media.Downloader
}

func (l linkedOnly) Download(ctx context.Context, m whatsmeow.DownloadableMessage) ([]byte, error) {
	if l.s.State() != StateLinked {
		return nil, ErrMediaOffline
	}
	return l.dl.Download(ctx, m)
}

// DownloadMedia returns the cached path and mime type of the attachment of message id in the
// chat jid, downloading, decrypting and verifying it first when it is not cached yet. It is the
// only path to a download: storing a message never fetches anything. While unlinked it returns
// ipc.ErrNotLinked; an unknown id or a text message is ErrNoMedia; a download needed while offline
// is ErrMediaOffline (a cached file is still served); a SHA-256 mismatch is ErrMediaCorrupt with
// nothing cached; any other download failure is ErrMediaNotDownloaded. Logs carry the message id
// only, never a caption.
func (s *Session) DownloadMedia(jid, id string) (string, string, error) {
	if s.client == nil || s.State() == StateUnlinked {
		return "", "", ipc.ErrNotLinked
	}
	s.mu.Lock()
	cache := s.media
	s.mu.Unlock()
	if s.db == nil || cache == nil {
		return "", "", ErrNoMedia
	}
	m, err := store.GetMessage(s.db, jid, id)
	if errors.Is(err, store.ErrNoMessage) {
		return "", "", ErrNoMedia
	}
	if err != nil {
		return "", "", fmt.Errorf("wa: %w", err)
	}
	if len(m.MediaRef) == 0 {
		return "", "", ErrNoMedia
	}
	sub, err := decodeRef(m.Kind, m.MediaRef)
	if err != nil {
		return "", "", err
	}
	ctx, cancel := context.WithTimeout(context.Background(), mediaTimeout)
	defer cancel()
	path, err := cache.Fetch(ctx, media.Request{ChatJID: jid, MessageID: id, Ext: extOf(m.Mime), Media: sub})
	switch {
	case err == nil:
		return path, m.Mime, nil
	case errors.Is(err, ErrMediaOffline):
		return "", "", ErrMediaOffline
	case errors.Is(err, media.ErrSHA256Mismatch):
		log.Printf("wa: media %s does not match its SHA-256; nothing was cached", id)
		return "", "", ErrMediaCorrupt
	}
	// The error's type only: whatsmeow's text can carry a media URL.
	log.Printf("wa: media %s was not downloaded (%T)", id, errors.Unwrap(err))
	return "", "", ErrMediaNotDownloaded
}

// extOf is the cache file extension hinted by a mime type ("image/jpeg" gives "jpeg",
// "audio/ogg; codecs=opus" gives "ogg"); the media package drops anything unsafe.
func extOf(mime string) string {
	_, sub, _ := strings.Cut(mime, "/")
	sub, _, _ = strings.Cut(sub, ";")
	return strings.TrimSpace(sub)
}
