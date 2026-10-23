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
	// ErrMediaCache: nuncio could not write its own media cache (a local disk failure), so nothing
	// was downloaded or saved.
	ErrMediaCache = errors.New("nuncio could not write its media cache, so the media was not downloaded")
	// ErrMediaWiped: a wipe closed the media cache (the daemon exits right after it), so nothing
	// was downloaded or saved.
	ErrMediaWiped = errors.New("nuncio is wiping its data, so the media was not downloaded")
)

// mediaMessage is what the media sub-messages of whatsmeow nuncio stores have in common.
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
	case msg.GetStickerMessage() != nil:
		// A sticker has no caption; it is downloaded only on request, like an image.
		return store.KindSticker, msg.GetStickerMessage(), "", "", true
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
	case store.KindSticker:
		m = &waE2E.StickerMessage{}
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
// nothing cached; a cache a wipe closed is ErrMediaWiped, one nuncio cannot write ErrMediaCache;
// any other download failure is ErrMediaNotDownloaded. Logs carry the message id only, never a
// caption.
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
	jid = s.CanonicalJID(jid)
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
	path, err := cache.Fetch(ctx, media.Request{ChatJID: jid, MessageID: id, Ext: extFor(m), Legacy: legacyExt(m.Mime), Media: sub})
	switch {
	case err == nil:
		return path, m.Mime, nil
	case errors.Is(err, ErrMediaOffline):
		return "", "", ErrMediaOffline
	case errors.Is(err, media.ErrClosed):
		return "", "", ErrMediaWiped
	case errors.Is(err, media.ErrCache):
		// The error's type only: the path names the data dir. No message id either.
		log.Printf("wa: media could not be cached (%s)", ErrKind(err))
		return "", "", ErrMediaCache
	case errors.Is(err, media.ErrSHA256Mismatch):
		log.Print("wa: media does not match its SHA-256; nothing was cached")
		return "", "", ErrMediaCorrupt
	}
	// The error's type only: whatsmeow's text can carry a media URL.
	log.Printf("wa: media was not downloaded (%s)", ErrKind(err))
	return "", "", ErrMediaNotDownloaded
}

// mimeExts maps the mime types whose subtype is not their usual file extension.
var mimeExts = map[string]string{
	"text/plain":               "txt",
	"image/jpeg":               "jpg",
	"audio/mpeg":               "mp3",
	"audio/mp4":                "m4a",
	"audio/ogg":                "ogg",
	"video/quicktime":          "mov",
	"text/csv":                 "csv",
	"application/msword":       "doc",
	"application/vnd.ms-excel": "xls",
	"application/vnd.openxmlformats-officedocument.wordprocessingml.document":   "docx",
	"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet":         "xlsx",
	"application/vnd.openxmlformats-officedocument.presentationml.presentation": "pptx",
}

// openableExts are the only extensions a cached file may carry when they come from the sender (a
// document's file name or a mime subtype): documents, archives and media, never an executable,
// script or launcher (sh, desktop, jar, AppImage, exe, py, ...), since a UI hands the cached file to
// xdg-open. Compared lower-cased.
var openableExts = map[string]bool{}

func init() {
	for _, e := range strings.Fields(`pdf txt csv tsv md rtf odt ods odp doc docx xls xlsx ppt pptx epub
		vcf ics zip gz tar 7z rar jpg jpeg png gif webp heic heif bmp tif tiff
		mp3 m4a ogg oga opus wav flac aac amr mp4 m4v mov mkv webm avi 3gp`) {
		openableExts[e] = true
	}
}

// openable reports whether ext is safe for the cache file name (media.SafeExt) and openable.
func openable(ext string) bool { return media.SafeExt(ext) && openableExts[strings.ToLower(ext)] }

// extFor is the cache file extension of a stored media message: for a document, the extension of
// its own file name when it is openable; otherwise the usual extension of its mime type (mimeExts),
// else the mime subtype ("audio/ogg; codecs=opus" gives "ogg") when it is openable; else none.
func extFor(m store.Message) string {
	if m.Kind == store.KindDocument {
		if i := strings.LastIndexByte(m.FileName, '.'); i >= 0 && openable(m.FileName[i+1:]) {
			return m.FileName[i+1:]
		}
	}
	mime := mimeOf(m.Mime)
	if ext, ok := mimeExts[mime]; ok {
		return ext
	}
	if _, sub, _ := strings.Cut(mime, "/"); openable(sub) {
		return sub
	}
	return ""
}

// mimeOf is the mime type without parameters, lower-cased.
func mimeOf(mime string) string {
	mime, _, _ = strings.Cut(mime, ";")
	return strings.ToLower(strings.TrimSpace(mime))
}

// legacyExt is exactly the extension builds before extFor asked the cache for (their extOf: the raw
// mime subtype, case kept, without parameters), so a file they cached is still found. The cache
// appends it only when media.SafeExt accepts it, as those builds did, and otherwise looks for the
// bare name.
func legacyExt(mime string) string {
	_, sub, _ := strings.Cut(mime, "/")
	sub, _, _ = strings.Cut(sub, ";")
	return strings.TrimSpace(sub)
}
