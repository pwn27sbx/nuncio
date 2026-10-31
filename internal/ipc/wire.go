// Package ipc implements the nuncio v1 wire protocol: NDJSON frames over an
// AF_UNIX stream socket, as specified in proto/wire.md.
//
// The types in this file are the Go half of the contract. Both the daemon and
// the Rust clients are checked against the same fixtures in proto/fixtures/,
// so the two languages cannot drift apart silently.
package ipc

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"strings"
	"sync"
	"time"
)

const (
	// Version is the only protocol version this package speaks.
	Version = 1
	// MaxFrameLen is the largest frame body, excluding the newline that
	// terminates it. A longer frame is a protocol error.
	MaxFrameLen = 1 << 20
)

// Bounds of the chats and messages request limit.
const (
	MinChatLimit = 1
	MaxChatLimit = 200
)

// MaxTextLen is the longest text a send frame may carry, in bytes.
const MaxTextLen = 65536

// Frame ops.
const (
	OpHello   = "hello"
	OpHelloOK = "hello_ok"
	OpStatus  = "status"
	OpChats   = "chats"
	OpError   = "error"

	OpMessages   = "messages"
	OpSend       = "send"
	OpSent       = "sent"
	OpSendFailed = "send_failed"
	OpRetry      = "retry"
	OpSubscribe  = "subscribe"
	OpSubscribed = "subscribed"
	OpFocus      = "focus"
	OpFocused    = "focused"
	OpWipe       = "wipe"
	OpWiped      = "wiped"

	OpDownloadMedia = "download_media"
	OpMediaReady    = "media_ready"

	OpSearch  = "search"
	OpReact   = "react"
	OpReacted = "reacted"

	OpMarkRead   = "mark_read"
	OpMarkedRead = "marked_read"

	// The device-linking flow: the daemon generates the QR and the clients only draw it (added
	// in the link-from-the-app work).
	OpLinkStart     = "link_start"
	OpLinkCancel    = "link_cancel"
	OpLinkStarted   = "link_started"
	OpLinkCancelled = "link_cancelled"
	OpLinkQR        = "link_qr"
	OpLinkState     = "link_state"

	OpEventMessage    = "event_message"
	OpEventConnection = "event_connection"
	OpEventUnread     = "event_unread"
	OpEventFocus      = "event_focus"
	OpEventChat       = "event_chat"
)

// Values of the message kind field.
const (
	KindText     = "text"
	KindImage    = "image"
	KindAudio    = "audio"
	KindVideo    = "video"
	KindDocument = "document"
	// KindSticker is a WhatsApp sticker: a media kind, downloaded only on request like an image
	// (T15 Part 2.5). A client that does not know it treats it like any other media kind.
	KindSticker = "sticker"
)

// Values of the sent frame's state field.
const (
	SendQueued = "queued"
	SendSent   = "sent"
)

// Values of a message's state field: set only on a send of mine that is not sent yet. A message
// without it was sent (or received).
const (
	MessageQueued  = "queued"
	MessageNotSent = "not_sent"
)

// Values of the link_state frame's state field: where a device-linking flow stands.
const (
	// LinkWaiting: the flow runs and a link_qr frame is (or will be) published; a later one
	// replaces it while the flow waits.
	LinkWaiting = "waiting"
	// LinkLinked: the device is paired; the session's own frames follow.
	LinkLinked = "linked"
	// LinkFailed: the flow ended without linking, with one fixed sentence in its reason.
	LinkFailed = "failed"
)

// Error codes.
const (
	CodeVersionMismatch = "version_mismatch"
	CodeBadFrame        = "bad_frame"
	CodeNotLinked       = "not_linked"
	CodeInternal        = "internal"
	// CodeNotConfirmed answers a send or retry whose acknowledgement from WhatsApp timed out: it may
	// have been delivered, so it is neither sent nor send_failed (added in M2).
	CodeNotConfirmed = "not_confirmed"
)

// Values of the session field.
const (
	SessionUnlinked = "unlinked"
	SessionLinked   = "linked"
	SessionOffline  = "offline"
)

// TimestampFormat is the only timestamp format the protocol uses: RFC 3339 in
// UTC, with a Z suffix.
const TimestampFormat = time.RFC3339

// Timestamp renders t in the protocol's timestamp format.
func Timestamp(t time.Time) string { return t.UTC().Format(TimestampFormat) }

var (
	// ErrFrameTooLong reports a frame larger than MaxFrameLen.
	ErrFrameTooLong = errors.New("ipc: frame exceeds 1 MiB")
	// ErrBadFrame reports a frame that is not a single JSON object.
	ErrBadFrame = errors.New("ipc: malformed frame")
	// ErrNotLinked is what a Handler's Send returns when no device is paired; ServeConn answers it
	// with the not_linked error frame, whose message is this error's text.
	ErrNotLinked = errors.New("no WhatsApp device is linked")
	// ErrNotConfirmed is what a Handler's Send returns when WhatsApp did not acknowledge the
	// message in time. The message may have been delivered, so it is neither sent nor not sent:
	// ServeConn answers a not_confirmed error with this text, never send_failed.
	ErrNotConfirmed = errors.New("not confirmed: WhatsApp did not acknowledge in time; it may have been delivered")
	// ErrWiped is what ServeConn returns after it answered a wipe with wiped: the daemon's data is
	// gone and it must stop serving.
	ErrWiped = errors.New("ipc: the daemon's data was wiped; it stops serving")
)

// MaxQueryLen bounds a search query in bytes; MaxEmojiLen bounds a reaction's emoji in bytes (a
// long ZWJ sequence with skin tones fits easily).
const (
	MaxQueryLen = 1024
	MaxEmojiLen = 64
)

// Hello is the first frame a client sends. The daemon refuses anything else.
type Hello struct {
	V      int    `json:"v"`
	Op     string `json:"op"`
	Client string `json:"client"`
}

// StatusRequest asks for the daemon's current state.
type StatusRequest struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// ChatsRequest asks for the chat list. Limit is MinChatLimit..MaxChatLimit.
type ChatsRequest struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	Limit int    `json:"limit"`
}

// MessagesRequest asks for up to Limit messages of one chat, newest first. Limit is
// MinChatLimit..MaxChatLimit.
type MessagesRequest struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	JID   string `json:"jid"`
	Limit int    `json:"limit"`
}

// SendRequest sends one text message to one chat. ClientID is chosen by the client and
// identifies the send in the reply, so a retry can never become a duplicate.
type SendRequest struct {
	V        int    `json:"v"`
	Op       string `json:"op"`
	JID      string `json:"jid"`
	Text     string `json:"text"`
	ClientID string `json:"client_id"`
	// ReplyTo, when set, is the id of a message of that chat the send quotes (added in M2).
	ReplyTo string `json:"reply_to,omitempty"`
}

// SearchRequest asks for up to Limit stored messages of one chat whose text contains every word of
// Query, newest first. Limit is MinChatLimit..MaxChatLimit. The answer is a MessagesResponse.
type SearchRequest struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	JID   string `json:"jid"`
	Query string `json:"query"`
	Limit int    `json:"limit"`
}

// ReactRequest sets my reaction on message ID of a chat; an empty Emoji removes it.
type ReactRequest struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	JID   string `json:"jid"`
	ID    string `json:"id"`
	Emoji string `json:"emoji"`
}

// Reacted answers a ReactRequest that WhatsApp accepted.
type Reacted struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	JID   string `json:"jid"`
	ID    string `json:"id"`
	Emoji string `json:"emoji"`
}

// MarkReadRequest marks every unread message of chat JID read, here and on WhatsApp. A UI sends it
// when the user opens the chat: it is the explicit view after which read receipts go out.
type MarkReadRequest struct {
	V   int    `json:"v"`
	Op  string `json:"op"`
	JID string `json:"jid"`
}

// MarkedRead answers a MarkReadRequest once the chat is read: Unread is the new total unread count.
type MarkedRead struct {
	V      int    `json:"v"`
	Op     string `json:"op"`
	JID    string `json:"jid"`
	Unread int    `json:"unread"`
}

// RetryRequest sends again, once, a send of this daemon that ended not sent, under its first
// ClientID and with its first WhatsApp message id, so it can never become a second message. It is
// the only way a not-sent message is sent again: nothing retries on its own.
type RetryRequest struct {
	V        int    `json:"v"`
	Op       string `json:"op"`
	ClientID string `json:"client_id"`
}

// SubscribeRequest asks the daemon to push event frames on this connection.
type SubscribeRequest struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// LinkStartRequest starts the device-linking flow: the daemon generates the QR and publishes it to
// subscribed connections, and the client only draws it. Consent is true only when the user gave
// their explicit yes in the client; false is a refusal that links nothing.
type LinkStartRequest struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Consent bool   `json:"consent"`
}

// LinkCancelRequest stops a linking flow in progress; it leaves no device, store or keyring entry
// behind.
type LinkCancelRequest struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// FocusRequest asks the running UIs to open one chat.
type FocusRequest struct {
	V   int    `json:"v"`
	Op  string `json:"op"`
	JID string `json:"jid"`
}

// WipeRequest asks the daemon to delete everything it keeps for the device (keyring items,
// database, media cache, exports), remove its socket and exit. With Unlink it first tells WhatsApp
// to unlink the device (`nuncio logout`); without it WhatsApp is never contacted (`nuncio purge`).
type WipeRequest struct {
	V      int    `json:"v"`
	Op     string `json:"op"`
	Unlink bool   `json:"unlink"`
}

// DownloadMediaRequest asks the daemon to download the attachment of message ID in chat JID
// into its media cache, unless it is cached already. Nothing is downloaded without it.
type DownloadMediaRequest struct {
	V   int    `json:"v"`
	Op  string `json:"op"`
	JID string `json:"jid"`
	ID  string `json:"id"`
}

// HelloOK answers a Hello.
type HelloOK struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Daemon  string `json:"daemon"`
	Session string `json:"session"`
	Unread  int    `json:"unread"`
}

// StatusResponse answers a StatusRequest. StopReason is as in EventConnection.
type StatusResponse struct {
	V          int    `json:"v"`
	Op         string `json:"op"`
	Daemon     string `json:"daemon"`
	Linked     bool   `json:"linked"`
	Session    string `json:"session"`
	Unread     int    `json:"unread"`
	StopReason string `json:"stop_reason,omitempty"`
}

// ChatsResponse answers a ChatsRequest.
type ChatsResponse struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	Chats []Chat `json:"chats"`
}

// MessagesResponse answers a MessagesRequest.
type MessagesResponse struct {
	V        int       `json:"v"`
	Op       string    `json:"op"`
	JID      string    `json:"jid"`
	Messages []Message `json:"messages"`
}

// Sent answers a SendRequest that was accepted: State is SendSent, with the WhatsApp ID and
// timestamp, or SendQueued while offline, with neither.
type Sent struct {
	V         int    `json:"v"`
	Op        string `json:"op"`
	ClientID  string `json:"client_id"`
	State     string `json:"state"`
	ID        string `json:"id,omitempty"`
	Timestamp string `json:"timestamp,omitempty"`
}

// SendFailed answers a SendRequest that was not sent. The message stays visible as not sent.
type SendFailed struct {
	V        int    `json:"v"`
	Op       string `json:"op"`
	ClientID string `json:"client_id"`
	Message  string `json:"message"`
}

// Subscribed answers a SubscribeRequest; event frames follow on the same connection.
type Subscribed struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// LinkStarted answers a LinkStartRequest whose Consent was true: the flow runs and its link_qr and
// link_state frames follow on subscribed connections.
type LinkStarted struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// LinkCancelled answers a LinkCancelRequest: the flow stopped and no device was linked.
type LinkCancelled struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// Focused answers a FocusRequest. Delivered is how many subscribed UIs got the event; 0 means
// no UI is running.
type Focused struct {
	V         int    `json:"v"`
	Op        string `json:"op"`
	JID       string `json:"jid"`
	Delivered int    `json:"delivered"`
}

// Wiped answers a WipeRequest once everything is deleted and the socket is gone; the daemon then
// closes the connection and exits. Unlinked says whether WhatsApp confirmed the unlink.
type Wiped struct {
	V        int    `json:"v"`
	Op       string `json:"op"`
	Unlinked bool   `json:"unlinked"`
}

// MediaReady answers a DownloadMediaRequest: Path is the decrypted, verified file in the daemon's
// private media cache (the bytes never travel over the socket) and Mime its type.
type MediaReady struct {
	V    int    `json:"v"`
	Op   string `json:"op"`
	JID  string `json:"jid"`
	ID   string `json:"id"`
	Path string `json:"path"`
	Mime string `json:"mime"`
}

// EventMessage is pushed to subscribers when a message is stored.
type EventMessage struct {
	V       int     `json:"v"`
	Op      string  `json:"op"`
	JID     string  `json:"jid"`
	Message Message `json:"message"`
}

// EventConnection is pushed to subscribers when the session state changes. StopReason is set
// once WhatsApp stopped the session for good, or while nunciod keeps the stored device closed
// (StopKeyringLocked, StopDeviceUnreadable), one of the Stop* values, and omitted otherwise, so a
// frame without a stop is the same bytes as before it existed.
type EventConnection struct {
	V          int    `json:"v"`
	Op         string `json:"op"`
	Session    string `json:"session"`
	StopReason string `json:"stop_reason,omitempty"`
}

// The stop_reason values: why WhatsApp stopped the session, or why nunciod keeps the stored
// device closed, fixed words chosen here and never WhatsApp's own text, codes or ban expiry, nor a
// key, id or name.
const (
	StopLoggedOut       = "logged_out"
	StopTemporaryBan    = "temporary_ban"
	StopStreamReplaced  = "stream_replaced"
	StopClientOutdated  = "client_outdated"
	StopConnectRejected = "connect_rejected"
	// StopKeyringLocked: the keyring (Secret Service) is locked, absent or unavailable; nunciod
	// tries again on its backoff, so unlocking it is enough.
	StopKeyringLocked = "keyring_locked"
	// StopDeviceUnreadable: the data key is missing or another one, or a sealed row fails its
	// check; unlocking does not fix it.
	StopDeviceUnreadable = "device_unreadable"
	// StopOther is any other stop.
	StopOther = "other"
)

// EventUnread is pushed to subscribers when an unread count changes: Unread is the total,
// ChatUnread the count of the chat JID.
type EventUnread struct {
	V          int    `json:"v"`
	Op         string `json:"op"`
	Unread     int    `json:"unread"`
	JID        string `json:"jid"`
	ChatUnread int    `json:"chat_unread"`
}

// EventFocus is pushed to subscribers when a client asks to open a chat.
type EventFocus struct {
	V   int    `json:"v"`
	Op  string `json:"op"`
	JID string `json:"jid"`
}

// EventChat is pushed to subscribers when app-state sync from another device changes a chat's pin,
// archive or mute: Chat is the chat as it now is, as `chats` would list it (added in M2).
type EventChat struct {
	V    int    `json:"v"`
	Op   string `json:"op"`
	Chat Chat   `json:"chat"`
}

// EventLinkQR publishes one QR code of a device-linking flow: Size modules per side and Rows its
// matrix, row 0 first, each row Size characters of "0" or "1". It carries no quiet zone, so a
// client draws a 4-module margin around it. The matrix is ephemeral pairing material and must never
// be logged; Size and Rows are its only representation on the wire, so no payload string travels.
type EventLinkQR struct {
	V    int      `json:"v"`
	Op   string   `json:"op"`
	Size int      `json:"size"`
	Rows []string `json:"rows"`
}

// EventLinkState publishes where a device-linking flow stands: State is one of the Link* values,
// with Reason set (one fixed sentence) only when State is LinkFailed.
type EventLinkState struct {
	V      int    `json:"v"`
	Op     string `json:"op"`
	State  string `json:"state"`
	Reason string `json:"reason,omitempty"`
}

// Error reports a protocol or request failure. Code is one of the Code*
// constants; Message is for logs, not for users.
type Error struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Code    string `json:"code"`
	Message string `json:"message"`
}

// Chat is the M0 chat shape, plus muted (M1, from app-state sync), aliases (T15, ADR 0001): the
// phone-number chat JIDs merged into this chat, listed only while WhatsApp's LID mapping still
// points them here, and absent when there are none; and phone (ADR 0002): the contact's phone number
// masked by MaskPhone, absent for a group and when the number is not known.
type Chat struct {
	JID         string   `json:"jid"`
	Name        string   `json:"name"`
	Unread      int      `json:"unread"`
	Archived    bool     `json:"archived"`
	Pinned      bool     `json:"pinned"`
	Muted       bool     `json:"muted"`
	Aliases     []string `json:"aliases,omitempty"`
	Phone       string   `json:"phone,omitempty"`
	LastMessage *Message `json:"last_message,omitempty"`
}

// MaskPhone keeps only the last four digits of a phone number ("…0001"), so a display fallback
// never shows a full number; a number of four digits or fewer masks to "…".
func MaskPhone(number string) string {
	if len(number) <= 4 {
		return "…"
	}
	return "…" + number[len(number)-4:]
}

// Message is one chat message. Kind is one of the Kind* constants; Media is set for the media
// kinds and absent for text.
type Message struct {
	ID        string `json:"id"`
	Kind      string `json:"kind"`
	Text      string `json:"text"`
	FromMe    bool   `json:"from_me"`
	Timestamp string `json:"timestamp"`
	Media     *Media `json:"media,omitempty"`
	// State is MessageQueued or MessageNotSent for a send of mine that is not sent yet, with the
	// ClientID it was sent with (what a retry names); both are absent once it is sent.
	State    string `json:"state,omitempty"`
	ClientID string `json:"client_id,omitempty"`
	// ReplyTo is the id of the message this one quotes; Reactions are the current reactions on it,
	// mine first. Both are absent when empty (added in M2).
	ReplyTo   string     `json:"reply_to,omitempty"`
	Reactions []Reaction `json:"reactions,omitempty"`
	// Sender names the author of an incoming message in a group chat: her name (address book,
	// business, push name), else her number masked by MaskPhone, else "unknown"; never a LID's
	// digits or a full number. Absent in a direct chat, on mine and on a message stored before
	// senders were recorded (added in M4).
	Sender string `json:"sender,omitempty"`
	// Thumb is the thumbnail WhatsApp embeds in an image (JPEG) or sticker (PNG) message, base64 on
	// the wire, with its ThumbMime; Width and Height are the picture's size in pixels and Animated
	// marks an animated sticker. All are read from the message itself, never downloaded, and absent
	// when empty, so a message without them encodes as before (T15 Part 2.5).
	Thumb     []byte `json:"thumb,omitempty"`
	ThumbMime string `json:"thumb_mime,omitempty"`
	Width     int    `json:"width,omitempty"`
	Height    int    `json:"height,omitempty"`
	Animated  bool   `json:"animated,omitempty"`
}

// Reaction is one sender's current reaction on a message.
type Reaction struct {
	Emoji  string `json:"emoji"`
	FromMe bool   `json:"from_me"`
}

// Media describes a message's attachment. The bytes are never on the wire: they are downloaded
// on demand into the daemon's media cache.
type Media struct {
	Mime     string `json:"mime"`
	Size     int64  `json:"size"`
	FileName string `json:"file_name"`
}

// Status is the daemon state, reported both over the socket protocol and over
// the D-Bus face.
type Status struct {
	Daemon  string
	Linked  bool
	Session string
	Unread  int
	// StopReason is the socket's stop_reason ("" while not stopped). It is not part of the D-Bus
	// Status reply, whose signature stays (sbsi).
	StopReason string `dbus:"-"`
}

// Handler serves the requests of one connection.
type Handler interface {
	// Hello records a client handshake and returns the current state.
	Hello(client string) Status
	// Status returns the current state.
	Status() Status
	// Chats returns every known chat, newest first. ServeConn applies the
	// limit the client asked for.
	Chats() []Chat
	// Messages returns up to limit messages of the chat jid, newest first; none for a chat it
	// does not know. ServeConn has already checked jid and limit.
	Messages(jid string, limit int) ([]Message, error)
	// Send sends text to the chat jid once. ServeConn has already checked jid, text and clientID,
	// and fills in the reply's V, Op and ClientID. It returns ErrNotLinked when no device is
	// paired, which ServeConn answers with not_linked, and ErrNotConfirmed when the outcome is
	// unknown, which ServeConn answers with a not_confirmed error; any other error is a send_failed reply
	// whose message is the error's text, so it must be short and carry no number or message text.
	Send(jid, text, clientID string) (Sent, error)
	// Retry sends again, once, the send stored under clientID when it ended not sent, and answers
	// its stored state otherwise without sending anything. ServeConn has already checked that
	// clientID is not empty and answers the result exactly like Send's.
	Retry(clientID string) (Sent, error)
	// Wipe deletes everything the daemon keeps for the device and removes its socket; with unlink
	// it first tells WhatsApp to unlink the device and deletes nothing if that fails. It returns
	// ErrNotLinked for an unlink with no paired device, which ServeConn answers with not_linked;
	// any other error is an internal error whose message is the error's text, so it must carry no
	// number. After a nil return ServeConn answers wiped and returns ErrWiped.
	Wipe(unlink bool) error
	// DownloadMedia returns the cached path and mime type of the attachment of message id in the
	// chat jid, downloading it first if it is not cached. ServeConn has already checked jid and
	// that id is not empty. It returns ErrNotLinked when no device is paired, which ServeConn
	// answers with not_linked; any other error is an internal error whose message is the error's
	// text, so it must carry no number and no caption.
	DownloadMedia(jid, id string) (path, mime string, err error)
}

// Searcher, Replier and Reactor are the M2 operations. They are separate from Handler so that a
// Handler written before them still compiles; ServeConn answers search, react and a send with
// reply_to with bad_frame when its Handler lacks the one needed.
type Searcher interface {
	// Search returns up to limit messages of the chat jid whose text contains every word of query,
	// newest first. ServeConn has already checked jid, query and limit.
	Search(jid, query string, limit int) ([]Message, error)
}

// Replier sends a reply: like Handler.Send, quoting the message replyTo of the same chat.
type Replier interface {
	SendReply(jid, text, clientID, replyTo string) (Sent, error)
}

// Reactor sets my reaction on message id of the chat jid (empty emoji removes it). It returns
// ErrNotLinked when no device is paired, which ServeConn answers with not_linked; any other error is
// an internal error whose message is the error's text, so it must carry no number and no emoji.
type Reactor interface {
	React(jid, id, emoji string) error
}

// MarkReader marks a chat read when a UI opens it, which is the explicit view: it sends WhatsApp the
// read receipts of the chat's unread incoming messages (respecting the account's read-receipt
// privacy), then clears them locally and returns the new total unread count. It is separate from
// Handler, like Searcher, so a Handler written before it still compiles; ServeConn answers mark_read
// with bad_frame when its Handler lacks it. It returns ErrNotLinked when no device is paired, which
// ServeConn answers with not_linked; any other error is an internal error whose message is the
// error's text, so it must carry no number and no message text.
type MarkReader interface {
	MarkRead(jid string) (unread int, err error)
}

// ChatResolver names the chat a request's jid stands for (ADR 0001, one chat per contact): a
// contact's phone-number JID is an alias of her LID chat once the daemon knows the pair. ServeConn
// asks it for the jid of every request that names a chat (messages, send, search, react, mark_read,
// focus, download_media), after checking that jid, and then serves and answers the request with
// the chat it returns, so a reply carries that jid. It is optional, like Searcher: without it, or
// when it returns something that is not a chat JID, the jid is served as the client named it.
type ChatResolver interface {
	CanonicalJID(jid string) string
}

// canonical is jid as h's ChatResolver resolves it, or jid itself.
func canonical(h Handler, jid string) string {
	r, ok := h.(ChatResolver)
	if !ok {
		return jid
	}
	if c := r.CanonicalJID(jid); ValidJID(c) {
		return c
	}
	return jid
}

// ValidJID reports whether jid names a WhatsApp chat the protocol carries: a user
// (`digits@s.whatsapp.net`, or `digits@lid` for a hidden-number user) or a group (`digits@g.us`, or
// `digits-digits@g.us` for an old group id). Device suffixes, broadcasts and other servers are not
// chats.
func ValidJID(jid string) bool {
	user, server, ok := strings.Cut(jid, "@")
	if !ok {
		return false
	}
	switch server {
	case "s.whatsapp.net", "lid":
		return digits(user)
	case "g.us":
		first, second, hyphen := strings.Cut(user, "-")
		return digits(first) && (!hyphen || digits(second))
	}
	return false
}

// digits reports whether s is one or more ASCII digits.
func digits(s string) bool { return s != "" && strings.Trim(s, "0123456789") == "" }

// WriteFrame writes v as one NDJSON frame.
func WriteFrame(w io.Writer, v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return fmt.Errorf("ipc: marshal frame: %w", err)
	}
	if len(data) > MaxFrameLen {
		return fmt.Errorf("%w: %d bytes", ErrFrameTooLong, len(data))
	}
	if _, err := w.Write(append(data, '\n')); err != nil {
		return fmt.Errorf("ipc: write frame: %w", err)
	}
	return nil
}

// Reader decodes NDJSON frames, enforcing MaxFrameLen.
type Reader struct {
	scanner *bufio.Scanner
}

// NewReader returns a Reader that reads frames from r.
func NewReader(r io.Reader) *Reader {
	scanner := bufio.NewScanner(r)
	// One byte past the cap, so a frame of exactly MaxFrameLen bytes is read
	// whole and judged by the explicit length check in Next, not by the
	// scanner's buffer limit.
	scanner.Buffer(make([]byte, 0, 64*1024), MaxFrameLen+1)
	return &Reader{scanner: scanner}
}

// Next returns the next frame's bytes, without the terminating newline. The
// slice is only valid until the next call. It returns io.EOF at the end of
// the stream, ErrFrameTooLong when a frame exceeds MaxFrameLen and ErrBadFrame
// when the line is not a single JSON object.
func (r *Reader) Next() ([]byte, error) {
	if !r.scanner.Scan() {
		if err := r.scanner.Err(); err != nil {
			if errors.Is(err, bufio.ErrTooLong) {
				return nil, ErrFrameTooLong
			}
			return nil, err
		}
		return nil, io.EOF
	}
	line := r.scanner.Bytes()
	if len(line) > MaxFrameLen {
		return nil, ErrFrameTooLong
	}
	if len(bytes.TrimSpace(line)) == 0 {
		return nil, fmt.Errorf("%w: empty frame", ErrBadFrame)
	}
	return line, nil
}

// Read decodes the next frame into v.
func (r *Reader) Read(v any) error {
	line, err := r.Next()
	if err != nil {
		return err
	}
	if err := json.Unmarshal(line, v); err != nil {
		return fmt.Errorf("%w: %v", ErrBadFrame, err)
	}
	return nil
}

// envelope is the part of a frame the daemon needs before it knows the op.
type envelope struct {
	V  int    `json:"v"`
	Op string `json:"op"`
}

// PanicMessage is the message of the internal error answered to a request whose handling
// panicked; the connection closes after it.
const PanicMessage = "the daemon failed while answering this request, so it closed this connection"

// ErrPanicked is returned by ServeConnEvents when handling a request panicked. The panic was
// already logged by the type of its value, and the connection is closed.
var ErrPanicked = errors.New("ipc: a request panicked")

// ServeConn serves one client connection until the peer closes it: hello
// first, then requests. It returns nil for a clean close and an error
// describing why it stopped otherwise.
func ServeConn(conn io.ReadWriteCloser, h Handler) error {
	return ServeConnEvents(conn, h, nil)
}

// ServeConnEvents is ServeConn with events: after `subscribe` the connection also receives every
// frame published to events, interleaved with its replies. A subscriber that falls
// SubscriberQueue frames behind is dropped and its connection closed. With a nil events broker,
// subscribe is answered and no event ever follows.
func ServeConnEvents(conn io.ReadWriteCloser, h Handler, events *Broker) (err error) {
	defer func() { _ = conn.Close() }()
	rd := NewReader(conn)
	// Replies and events interleave on one socket, so every frame is written whole under out's
	// lock.
	out := &lockedWriter{w: conn}
	var cancel func()
	defer func() {
		if cancel != nil {
			cancel()
		}
	}()
	// One recover for the whole connection: a bug while one request is answered ends this
	// connection only, never the daemon, its other connections or the session.
	defer func() {
		r := recover()
		if r == nil {
			return
		}
		// The value's type only, and no stack: its text or a stack's arguments can quote a
		// message or a number.
		log.Printf("ipc: a request panicked (%T); its connection was closed", r)
		// A panic while out's lock was held (subscribe) leaves it locked: then no reply.
		if out.mu.TryLock() {
			_ = WriteFrame(out.w, Error{V: Version, Op: OpError, Code: CodeInternal, Message: PanicMessage})
			out.mu.Unlock()
		}
		err = ErrPanicked
	}()

	first, err := rd.Next()
	if err != nil {
		if errors.Is(err, io.EOF) {
			return nil
		}
		// The stream cannot be resynchronised after a framing error.
		return refuse(out, CodeBadFrame, err.Error())
	}

	var hello Hello
	if err := json.Unmarshal(first, &hello); err != nil {
		return refuse(out, CodeBadFrame, fmt.Sprintf("malformed hello frame: %v", err))
	}
	if hello.V != Version {
		return refuse(out, CodeVersionMismatch, "unsupported protocol version")
	}
	if hello.Op != OpHello {
		return refuse(out, CodeBadFrame, fmt.Sprintf("first frame must be %q", OpHello))
	}

	st := h.Hello(hello.Client)
	if err := WriteFrame(out, HelloOK{
		V: Version, Op: OpHelloOK, Daemon: st.Daemon, Session: st.Session, Unread: st.Unread,
	}); err != nil {
		return err
	}

	for {
		line, err := rd.Next()
		if err != nil {
			if errors.Is(err, io.EOF) {
				return nil
			}
			// A frame past the size cap leaves the stream desynchronised.
			return refuse(out, CodeBadFrame, err.Error())
		}

		var env envelope
		if err := json.Unmarshal(line, &env); err != nil {
			// The line boundary was intact, so the connection survives.
			if err := WriteFrame(out, badFrame("malformed frame")); err != nil {
				return err
			}
			continue
		}
		if env.V != Version {
			return refuse(out, CodeVersionMismatch, "unsupported protocol version")
		}

		switch env.Op {
		case OpStatus:
			st := h.Status()
			if err := WriteFrame(out, StatusResponse{
				V: Version, Op: OpStatus, Daemon: st.Daemon,
				Linked: st.Linked, Session: st.Session, Unread: st.Unread, StopReason: st.StopReason,
			}); err != nil {
				return err
			}
		case OpChats:
			var req ChatsRequest
			if err := json.Unmarshal(line, &req); err != nil {
				if err := WriteFrame(out, badFrame("malformed chats frame")); err != nil {
					return err
				}
				continue
			}
			if req.Limit < MinChatLimit || req.Limit > MaxChatLimit {
				msg := fmt.Sprintf("limit must be between %d and %d", MinChatLimit, MaxChatLimit)
				if err := WriteFrame(out, badFrame(msg)); err != nil {
					return err
				}
				continue
			}
			chats := h.Chats()
			if len(chats) > req.Limit {
				chats = chats[:req.Limit]
			}
			if chats == nil {
				chats = []Chat{}
			}
			// Long previews or names can make the list longer than a frame: it keeps its newest chats.
			chats = fitChats(chats)
			if err := WriteFrame(out, ChatsResponse{V: Version, Op: OpChats, Chats: chats}); err != nil {
				return err
			}
		case OpMessages:
			var req MessagesRequest
			if err := json.Unmarshal(line, &req); err != nil {
				if err := WriteFrame(out, badFrame("malformed messages frame")); err != nil {
					return err
				}
				continue
			}
			if req.Limit < MinChatLimit || req.Limit > MaxChatLimit {
				msg := fmt.Sprintf("limit must be between %d and %d", MinChatLimit, MaxChatLimit)
				if err := WriteFrame(out, badFrame(msg)); err != nil {
					return err
				}
				continue
			}
			if !ValidJID(req.JID) {
				if err := WriteFrame(out, badFrame("jid is not a WhatsApp chat")); err != nil {
					return err
				}
				continue
			}
			req.JID = canonical(h, req.JID)
			msgs, err := h.Messages(req.JID, req.Limit)
			if err != nil {
				if err := WriteFrame(out, Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()}); err != nil {
					return err
				}
				continue
			}
			if len(msgs) > req.Limit {
				msgs = msgs[:req.Limit]
			}
			if msgs == nil {
				msgs = []Message{}
			}
			if err := WriteFrame(out, MessagesResponse{V: Version, Op: OpMessages, JID: req.JID, Messages: msgs}); err != nil {
				return err
			}
		case OpSubscribe:
			// The reply is written under out's lock, so no event can overtake it; a second
			// subscribe answers again without a second subscription.
			out.mu.Lock()
			if cancel == nil {
				var frames <-chan []byte
				frames, cancel = events.Subscribe(func() { _ = conn.Close() })
				go writeEvents(out, frames, func() { _ = conn.Close() })
			}
			err := WriteFrame(out.w, Subscribed{V: Version, Op: OpSubscribed})
			out.mu.Unlock()
			if err != nil {
				return err
			}
		case OpSend:
			if err := WriteFrame(out, serveSend(line, h)); err != nil {
				return err
			}
		case OpSearch:
			if err := WriteFrame(out, serveSearch(line, h)); err != nil {
				return err
			}
		case OpReact:
			if err := WriteFrame(out, serveReact(line, h)); err != nil {
				return err
			}
		case OpMarkRead:
			if err := WriteFrame(out, serveMarkRead(line, h)); err != nil {
				return err
			}
		case OpRetry:
			if err := WriteFrame(out, serveRetry(line, h)); err != nil {
				return err
			}
		case OpFocus:
			var req FocusRequest
			if err := json.Unmarshal(line, &req); err != nil || !ValidJID(req.JID) {
				if err := WriteFrame(out, badFrame("focus needs the jid of a WhatsApp chat")); err != nil {
					return err
				}
				continue
			}
			req.JID = canonical(h, req.JID)
			// Every subscribed UI gets the event, this connection included if it subscribed.
			delivered := events.Publish(EventFocus{V: Version, Op: OpEventFocus, JID: req.JID})
			if err := WriteFrame(out, Focused{V: Version, Op: OpFocused, JID: req.JID, Delivered: delivered}); err != nil {
				return err
			}
		case OpWipe:
			var req WipeRequest
			if err := json.Unmarshal(line, &req); err != nil {
				if err := WriteFrame(out, badFrame("malformed wipe frame")); err != nil {
					return err
				}
				continue
			}
			err := h.Wipe(req.Unlink)
			switch {
			case errors.Is(err, ErrNotLinked):
				err = WriteFrame(out, Error{V: Version, Op: OpError, Code: CodeNotLinked, Message: ErrNotLinked.Error()})
			case err != nil:
				err = WriteFrame(out, Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()})
			default:
				// The reply is written before the connection closes, so the client reads it whole.
				if err := WriteFrame(out, Wiped{V: Version, Op: OpWiped, Unlinked: req.Unlink}); err != nil {
					return err
				}
				return ErrWiped
			}
			if err != nil {
				return err
			}
		case OpDownloadMedia:
			if err := WriteFrame(out, serveDownloadMedia(line, h)); err != nil {
				return err
			}
		case OpHello:
			if err := WriteFrame(out, badFrame("hello is only valid as the first frame")); err != nil {
				return err
			}
		default:
			if err := WriteFrame(out, badFrame(fmt.Sprintf("unknown op %q", env.Op))); err != nil {
				return err
			}
		}
	}
}

// serveSend checks a send frame and asks h to send it, returning the reply frame. A refused frame
// never reaches h, so nothing is stored or sent.
func serveSend(line []byte, h Handler) any {
	var req SendRequest
	if err := json.Unmarshal(line, &req); err != nil {
		return badFrame("malformed send frame")
	}
	switch {
	case req.ClientID == "":
		return badFrame("send needs a client_id")
	case !ValidJID(req.JID):
		return badFrame("jid is not a WhatsApp chat")
	case strings.TrimSpace(req.Text) == "":
		return badFrame("send needs a text that is not empty")
	case len(req.Text) > MaxTextLen:
		return badFrame(fmt.Sprintf("text is longer than %d bytes", MaxTextLen))
	}
	req.JID = canonical(h, req.JID)
	if req.ReplyTo != "" {
		r, ok := h.(Replier)
		if !ok {
			return badFrame("this daemon cannot send replies")
		}
		sent, err := r.SendReply(req.JID, req.Text, req.ClientID, req.ReplyTo)
		return sendReply(req.ClientID, sent, err)
	}
	sent, err := h.Send(req.JID, req.Text, req.ClientID)
	return sendReply(req.ClientID, sent, err)
}

// serveSearch checks a search frame and asks h for the matches, returning the reply frame. A
// refused frame never reaches h.
func serveSearch(line []byte, h Handler) any {
	var req SearchRequest
	if err := json.Unmarshal(line, &req); err != nil {
		return badFrame("malformed search frame")
	}
	s, ok := h.(Searcher)
	switch {
	case !ok:
		return badFrame("this daemon cannot search")
	case req.Limit < MinChatLimit || req.Limit > MaxChatLimit:
		return badFrame(fmt.Sprintf("limit must be between %d and %d", MinChatLimit, MaxChatLimit))
	case !ValidJID(req.JID):
		return badFrame("jid is not a WhatsApp chat")
	case strings.TrimSpace(req.Query) == "":
		return badFrame("search needs a query that is not empty")
	case len(req.Query) > MaxQueryLen:
		return badFrame(fmt.Sprintf("query is longer than %d bytes", MaxQueryLen))
	}
	req.JID = canonical(h, req.JID)
	msgs, err := s.Search(req.JID, req.Query, req.Limit)
	if err != nil {
		return Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()}
	}
	if len(msgs) > req.Limit {
		msgs = msgs[:req.Limit]
	}
	if msgs == nil {
		msgs = []Message{}
	}
	return MessagesResponse{V: Version, Op: OpMessages, JID: req.JID, Messages: msgs}
}

// serveReact checks a react frame and asks h to send the reaction, returning the reply frame. A
// refused frame never reaches h, so nothing is sent.
func serveReact(line []byte, h Handler) any {
	var req ReactRequest
	if err := json.Unmarshal(line, &req); err != nil {
		return badFrame("malformed react frame")
	}
	r, ok := h.(Reactor)
	switch {
	case !ok:
		return badFrame("this daemon cannot react")
	case !ValidJID(req.JID):
		return badFrame("jid is not a WhatsApp chat")
	case req.ID == "":
		return badFrame("react needs the id of a message")
	case len(req.Emoji) > MaxEmojiLen:
		return badFrame(fmt.Sprintf("emoji is longer than %d bytes", MaxEmojiLen))
	}
	req.JID = canonical(h, req.JID)
	err := r.React(req.JID, req.ID, req.Emoji)
	if errors.Is(err, ErrNotLinked) {
		return Error{V: Version, Op: OpError, Code: CodeNotLinked, Message: ErrNotLinked.Error()}
	}
	if err != nil {
		return Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()}
	}
	return Reacted{V: Version, Op: OpReacted, JID: req.JID, ID: req.ID, Emoji: req.Emoji}
}

// serveMarkRead checks a mark_read frame and asks h to mark the chat read, returning the reply
// frame. A refused frame never reaches h, so no receipt is sent.
func serveMarkRead(line []byte, h Handler) any {
	var req MarkReadRequest
	if err := json.Unmarshal(line, &req); err != nil {
		return badFrame("malformed mark_read frame")
	}
	m, ok := h.(MarkReader)
	switch {
	case !ok:
		return badFrame("this daemon cannot mark a chat read")
	case !ValidJID(req.JID):
		return badFrame("jid is not a WhatsApp chat")
	}
	req.JID = canonical(h, req.JID)
	unread, err := m.MarkRead(req.JID)
	if errors.Is(err, ErrNotLinked) {
		return Error{V: Version, Op: OpError, Code: CodeNotLinked, Message: ErrNotLinked.Error()}
	}
	if err != nil {
		return Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()}
	}
	return MarkedRead{V: Version, Op: OpMarkedRead, JID: req.JID, Unread: unread}
}

// serveRetry checks a retry frame and asks h to retry it, returning the reply frame.
func serveRetry(line []byte, h Handler) any {
	var req RetryRequest
	if err := json.Unmarshal(line, &req); err != nil || req.ClientID == "" {
		return badFrame("retry needs the client_id of a send")
	}
	sent, err := h.Retry(req.ClientID)
	return sendReply(req.ClientID, sent, err)
}

// sendReply is the reply frame to a send or a retry of clientID.
func sendReply(clientID string, sent Sent, err error) any {
	if errors.Is(err, ErrNotLinked) {
		return Error{V: Version, Op: OpError, Code: CodeNotLinked, Message: ErrNotLinked.Error()}
	}
	if errors.Is(err, ErrNotConfirmed) {
		return Error{V: Version, Op: OpError, Code: CodeNotConfirmed, Message: ErrNotConfirmed.Error()}
	}
	if err != nil {
		return SendFailed{V: Version, Op: OpSendFailed, ClientID: clientID, Message: err.Error()}
	}
	sent.V, sent.Op, sent.ClientID = Version, OpSent, clientID
	return sent
}

// serveDownloadMedia checks a download_media frame and asks h for the attachment, returning the
// reply frame. A refused frame never reaches h, so nothing is downloaded.
func serveDownloadMedia(line []byte, h Handler) any {
	var req DownloadMediaRequest
	if err := json.Unmarshal(line, &req); err != nil {
		return badFrame("malformed download_media frame")
	}
	switch {
	case !ValidJID(req.JID):
		return badFrame("jid is not a WhatsApp chat")
	case req.ID == "":
		return badFrame("download_media needs the id of a message")
	}
	req.JID = canonical(h, req.JID)
	path, mime, err := h.DownloadMedia(req.JID, req.ID)
	if errors.Is(err, ErrNotLinked) {
		return Error{V: Version, Op: OpError, Code: CodeNotLinked, Message: ErrNotLinked.Error()}
	}
	if err != nil {
		return Error{V: Version, Op: OpError, Code: CodeInternal, Message: err.Error()}
	}
	return MediaReady{V: Version, Op: OpMediaReady, JID: req.JID, ID: req.ID, Path: path, Mime: mime}
}

// lockedWriter writes each frame whole, so replies and events never interleave mid-frame.
type lockedWriter struct {
	mu sync.Mutex
	w  io.Writer
}

func (l *lockedWriter) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.w.Write(p)
}

// writeEvents copies queued event frames to the connection until the subscription ends. A write
// that blocks holds only this connection's lock; dropping the subscriber closes the connection,
// which ends the write. A panic while one frame is written drops only this subscriber: drop closes
// its connection, which ends the subscription.
func writeEvents(out *lockedWriter, frames <-chan []byte, drop func()) {
	defer func() {
		if r := recover(); r != nil {
			// The value's type only, and no stack: either can quote the event's text.
			log.Printf("ipc: an event write panicked (%T); its subscriber was dropped", r)
			drop()
		}
	}()
	for frame := range frames {
		if _, err := out.Write(frame); err != nil {
			// The connection is broken; the read loop sees it too and ends the subscription.
			return
		}
	}
}

func badFrame(message string) Error {
	return Error{V: Version, Op: OpError, Code: CodeBadFrame, Message: message}
}

// refuse answers with an error frame and reports the reason to the caller. The
// caller closes the connection.
func refuse(conn io.Writer, code, message string) error {
	_ = WriteFrame(conn, Error{V: Version, Op: OpError, Code: code, Message: message})
	return fmt.Errorf("ipc: refused connection: %s: %s", code, message)
}
