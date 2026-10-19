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
	"time"
)

const (
	// Version is the only protocol version this package speaks.
	Version = 1
	// MaxFrameLen is the largest frame body, excluding the newline that
	// terminates it. A longer frame is a protocol error.
	MaxFrameLen = 1 << 20
)

// Bounds of the chats request limit.
const (
	MinChatLimit = 1
	MaxChatLimit = 200
)

// Frame ops.
const (
	OpHello   = "hello"
	OpHelloOK = "hello_ok"
	OpStatus  = "status"
	OpChats   = "chats"
	OpError   = "error"
)

// Error codes.
const (
	CodeVersionMismatch = "version_mismatch"
	CodeBadFrame        = "bad_frame"
	CodeNotLinked       = "not_linked"
	CodeInternal        = "internal"
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

// HelloOK answers a Hello.
type HelloOK struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Daemon  string `json:"daemon"`
	Session string `json:"session"`
	Unread  int    `json:"unread"`
}

// StatusResponse answers a StatusRequest.
type StatusResponse struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Daemon  string `json:"daemon"`
	Linked  bool   `json:"linked"`
	Session string `json:"session"`
	Unread  int    `json:"unread"`
}

// ChatsResponse answers a ChatsRequest.
type ChatsResponse struct {
	V     int    `json:"v"`
	Op    string `json:"op"`
	Chats []Chat `json:"chats"`
}

// Error reports a protocol or request failure. Code is one of the Code*
// constants; Message is for logs, not for users.
type Error struct {
	V       int    `json:"v"`
	Op      string `json:"op"`
	Code    string `json:"code"`
	Message string `json:"message"`
}

// Chat is the M0 chat shape.
type Chat struct {
	JID         string   `json:"jid"`
	Name        string   `json:"name"`
	Unread      int      `json:"unread"`
	Archived    bool     `json:"archived"`
	Pinned      bool     `json:"pinned"`
	LastMessage *Message `json:"last_message,omitempty"`
}

// Message is the M0 message shape. Kind is "text" in M0; media kinds land in
// M1.
type Message struct {
	ID        string `json:"id"`
	Kind      string `json:"kind"`
	Text      string `json:"text"`
	FromMe    bool   `json:"from_me"`
	Timestamp string `json:"timestamp"`
}

// Status is the daemon state, reported both over the socket protocol and over
// the D-Bus face.
type Status struct {
	Daemon  string
	Linked  bool
	Session string
	Unread  int
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
}

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

// ServeConn serves one client connection until the peer closes it: hello
// first, then requests. It returns nil for a clean close and an error
// describing why it stopped otherwise.
func ServeConn(conn io.ReadWriteCloser, h Handler) error {
	defer func() { _ = conn.Close() }()
	rd := NewReader(conn)

	first, err := rd.Next()
	if err != nil {
		if errors.Is(err, io.EOF) {
			return nil
		}
		// The stream cannot be resynchronised after a framing error.
		return refuse(conn, CodeBadFrame, err.Error())
	}

	var hello Hello
	if err := json.Unmarshal(first, &hello); err != nil {
		return refuse(conn, CodeBadFrame, fmt.Sprintf("malformed hello frame: %v", err))
	}
	if hello.V != Version {
		return refuse(conn, CodeVersionMismatch, "unsupported protocol version")
	}
	if hello.Op != OpHello {
		return refuse(conn, CodeBadFrame, fmt.Sprintf("first frame must be %q", OpHello))
	}

	st := h.Hello(hello.Client)
	if err := WriteFrame(conn, HelloOK{
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
			return refuse(conn, CodeBadFrame, err.Error())
		}

		var env envelope
		if err := json.Unmarshal(line, &env); err != nil {
			// The line boundary was intact, so the connection survives.
			if err := WriteFrame(conn, badFrame("malformed frame")); err != nil {
				return err
			}
			continue
		}
		if env.V != Version {
			return refuse(conn, CodeVersionMismatch, "unsupported protocol version")
		}

		switch env.Op {
		case OpStatus:
			st := h.Status()
			if err := WriteFrame(conn, StatusResponse{
				V: Version, Op: OpStatus, Daemon: st.Daemon,
				Linked: st.Linked, Session: st.Session, Unread: st.Unread,
			}); err != nil {
				return err
			}
		case OpChats:
			var req ChatsRequest
			if err := json.Unmarshal(line, &req); err != nil {
				if err := WriteFrame(conn, badFrame("malformed chats frame")); err != nil {
					return err
				}
				continue
			}
			if req.Limit < MinChatLimit || req.Limit > MaxChatLimit {
				msg := fmt.Sprintf("limit must be between %d and %d", MinChatLimit, MaxChatLimit)
				if err := WriteFrame(conn, badFrame(msg)); err != nil {
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
			if err := WriteFrame(conn, ChatsResponse{V: Version, Op: OpChats, Chats: chats}); err != nil {
				return err
			}
		case OpHello:
			if err := WriteFrame(conn, badFrame("hello is only valid as the first frame")); err != nil {
				return err
			}
		default:
			if err := WriteFrame(conn, badFrame(fmt.Sprintf("unknown op %q", env.Op))); err != nil {
				return err
			}
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
