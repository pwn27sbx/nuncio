package ipc

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"time"

	"github.com/godbus/dbus/v5"
)

// The daemon's session-bus face. proto/wire.md fixes the bus name; the object
// path and the interface name are the daemon's choice and must match on both
// sides.
const (
	DaemonName = "io.github.pwn27sbx.nuncio"
	ObjectPath = "/io/github/pwn27sbx/nuncio"
	Interface  = "io.github.pwn27sbx.nuncio"
)

// ClientName is what the thin `nuncio` CLI sends in its hello frame.
const ClientName = "nuncio/0.1.0"

// DefaultSocketPath returns the daemon's socket path,
// $XDG_RUNTIME_DIR/nuncio/core.sock.
func DefaultSocketPath() (string, error) {
	dir := os.Getenv("XDG_RUNTIME_DIR")
	if dir == "" {
		return "", errors.New("ipc: XDG_RUNTIME_DIR is not set")
	}
	return filepath.Join(dir, "nuncio", "core.sock"), nil
}

// ProtocolError is an error frame the daemon sent back.
type ProtocolError struct {
	Code    string
	Message string
}

func (e *ProtocolError) Error() string {
	return fmt.Sprintf("ipc: daemon error %s: %s", e.Code, e.Message)
}

// DefaultReadTimeout bounds every read a Client makes, so a wedged daemon cannot wedge the CLI.
const DefaultReadTimeout = 5 * time.Second

// Client is a decoded connection to the daemon, past the hello handshake.
type Client struct {
	conn    net.Conn
	rd      *Reader
	timeout time.Duration
}

// Dial connects to the daemon's socket at path, sends hello and returns a
// ready client whose reads are bounded by DefaultReadTimeout.
func Dial(path, clientName string) (*Client, error) {
	return DialTimeout(path, clientName, DefaultReadTimeout)
}

// DialTimeout is Dial with an explicit bound on every read, the handshake included. A read that
// hits the bound fails with an error matching os.ErrDeadlineExceeded, and the client is then
// unusable: close it.
func DialTimeout(path, clientName string, timeout time.Duration) (*Client, error) {
	conn, err := net.Dial("unix", path)
	if err != nil {
		return nil, fmt.Errorf("ipc: dial %s: %w", path, err)
	}
	c := &Client{conn: conn, rd: NewReader(conn), timeout: timeout}
	if err := WriteFrame(conn, Hello{V: Version, Op: OpHello, Client: clientName}); err != nil {
		_ = conn.Close()
		return nil, err
	}
	var reply HelloOK
	if err := c.readReply(&reply); err != nil {
		_ = conn.Close()
		return nil, fmt.Errorf("ipc: handshake: %w", err)
	}
	if reply.Op != OpHelloOK {
		_ = conn.Close()
		return nil, fmt.Errorf("ipc: handshake: expected op %s, got %q", OpHelloOK, reply.Op)
	}
	return c, nil
}

// Connect is Dial with the D-Bus fallback: when the socket is not usable it
// asks the session bus for the daemon's name, which activates the daemon, and
// then dials again. It never starts a daemon itself.
func Connect(clientName string) (*Client, error) {
	path, err := DefaultSocketPath()
	if err != nil {
		return nil, err
	}
	c, dialErr := Dial(path, clientName)
	if dialErr == nil {
		return c, nil
	}
	if err := Activate(); err != nil {
		return nil, fmt.Errorf("%w; asking the session bus for %s failed too: %v", dialErr, DaemonName, err)
	}
	// Owning the name means the daemon is listening already, so the retry
	// exists only to absorb the last few milliseconds of activation.
	for attempt := 0; attempt < 25; attempt++ {
		if c, err := Dial(path, clientName); err == nil {
			return c, nil
		}
		time.Sleep(20 * time.Millisecond)
	}
	return nil, fmt.Errorf("ipc: %s took the bus name but %s never became usable", DaemonName, path)
}

// Activate asks the session bus for the daemon name, which starts the daemon
// if nothing owns the name yet.
func Activate() error {
	conn, err := sessionBus()
	if err != nil {
		return err
	}
	defer func() { _ = conn.Close() }()

	// The reply is irrelevant: the point is that the bus starts the daemon to
	// deliver the call.
	call := conn.Object(DaemonName, dbus.ObjectPath(ObjectPath)).Call(Interface+".Status", 0)
	if call.Err != nil {
		return fmt.Errorf("ipc: call %s.%s: %w", DaemonName, Interface+".Status", call.Err)
	}
	return nil
}

// sessionBus opens a private connection to the session bus. An activated
// service is told the bus address through DBUS_STARTER_ADDRESS as well as
// DBUS_SESSION_BUS_ADDRESS, so both are honoured.
func sessionBus() (*dbus.Conn, error) {
	if addr := os.Getenv("DBUS_STARTER_ADDRESS"); addr != "" {
		if conn, err := dialBus(addr); err == nil {
			return conn, nil
		}
	}
	conn, err := dbus.SessionBusPrivate()
	if err != nil {
		return nil, fmt.Errorf("ipc: connect to the session bus: %w", err)
	}
	if err := conn.Auth(nil); err != nil {
		_ = conn.Close()
		return nil, fmt.Errorf("ipc: authenticate on the session bus: %w", err)
	}
	if err := conn.Hello(); err != nil {
		_ = conn.Close()
		return nil, fmt.Errorf("ipc: hello on the session bus: %w", err)
	}
	return conn, nil
}

func dialBus(addr string) (*dbus.Conn, error) {
	conn, err := dbus.Dial(addr)
	if err != nil {
		return nil, err
	}
	if err := conn.Auth(nil); err != nil {
		_ = conn.Close()
		return nil, err
	}
	if err := conn.Hello(); err != nil {
		_ = conn.Close()
		return nil, err
	}
	return conn, nil
}

// Status asks the daemon for its state.
func (c *Client) Status() (StatusResponse, error) {
	if err := WriteFrame(c.conn, StatusRequest{V: Version, Op: OpStatus}); err != nil {
		return StatusResponse{}, err
	}
	var resp StatusResponse
	if err := c.readReply(&resp); err != nil {
		return StatusResponse{}, err
	}
	if resp.Op != OpStatus {
		return StatusResponse{}, fmt.Errorf("ipc: expected op %s, got %q", OpStatus, resp.Op)
	}
	return resp, nil
}

// Chats asks the daemon for up to limit chats.
func (c *Client) Chats(limit int) (ChatsResponse, error) {
	if err := WriteFrame(c.conn, ChatsRequest{V: Version, Op: OpChats, Limit: limit}); err != nil {
		return ChatsResponse{}, err
	}
	var resp ChatsResponse
	if err := c.readReply(&resp); err != nil {
		return ChatsResponse{}, err
	}
	if resp.Op != OpChats {
		return ChatsResponse{}, fmt.Errorf("ipc: expected op %s, got %q", OpChats, resp.Op)
	}
	return resp, nil
}

// Messages asks the daemon for up to limit messages of the chat jid, newest first.
func (c *Client) Messages(jid string, limit int) (MessagesResponse, error) {
	if err := WriteFrame(c.conn, MessagesRequest{V: Version, Op: OpMessages, JID: jid, Limit: limit}); err != nil {
		return MessagesResponse{}, err
	}
	var resp MessagesResponse
	if err := c.readReply(&resp); err != nil {
		return MessagesResponse{}, err
	}
	if resp.Op != OpMessages {
		return MessagesResponse{}, fmt.Errorf("ipc: expected op %s, got %q", OpMessages, resp.Op)
	}
	return resp, nil
}

// NotSentError is a send_failed reply: the daemon did not send the message.
type NotSentError struct {
	ClientID string
	Reason   string
}

func (e *NotSentError) Error() string { return "not sent: " + e.Reason }

// SendTimeout bounds the wait for a send reply: the daemon waits for WhatsApp's acknowledgement
// before it answers, which can take longer than an ordinary read.
const SendTimeout = 30 * time.Second

// Send asks the daemon to send text to the chat jid once. It returns the sent reply (state sent
// or queued), a *NotSentError when the daemon answered send_failed, or a *ProtocolError such as
// not_linked.
func (c *Client) Send(jid, text, clientID string) (Sent, error) {
	return c.sendFrame(SendRequest{V: Version, Op: OpSend, JID: jid, Text: text, ClientID: clientID})
}

// Retry asks the daemon to send again, once, the send it stored under clientID when it ended not
// sent; any other send only reports its stored state. The replies are those of Send.
func (c *Client) Retry(clientID string) (Sent, error) {
	return c.sendFrame(RetryRequest{V: Version, Op: OpRetry, ClientID: clientID})
}

// sendFrame writes a send or retry frame and reads its reply.
func (c *Client) sendFrame(frame any) (Sent, error) {
	if err := WriteFrame(c.conn, frame); err != nil {
		return Sent{}, err
	}
	// Both replies decode into one value: Sent's fields plus send_failed's message.
	var reply struct {
		Sent
		Message string `json:"message"`
	}
	timeout := c.timeout
	c.timeout = max(timeout, SendTimeout)
	err := c.readReply(&reply)
	c.timeout = timeout
	if err != nil {
		return Sent{}, err
	}
	switch reply.Op {
	case OpSent:
		return reply.Sent, nil
	case OpSendFailed:
		return Sent{}, &NotSentError{ClientID: reply.ClientID, Reason: reply.Message}
	}
	return Sent{}, fmt.Errorf("ipc: expected op %s or %s, got %q", OpSent, OpSendFailed, reply.Op)
}

// Focus asks the running UIs to open the chat jid. Delivered in the reply is how many subscribed
// UIs got the request; 0 means no UI is running.
func (c *Client) Focus(jid string) (Focused, error) {
	if err := WriteFrame(c.conn, FocusRequest{V: Version, Op: OpFocus, JID: jid}); err != nil {
		return Focused{}, err
	}
	var resp Focused
	if err := c.readReply(&resp); err != nil {
		return Focused{}, err
	}
	if resp.Op != OpFocused {
		return Focused{}, fmt.Errorf("ipc: expected op %s, got %q", OpFocused, resp.Op)
	}
	return resp, nil
}

// WipeTimeout bounds the wait for a wipe reply: with unlink the daemon first waits for WhatsApp
// to confirm the unlink.
const WipeTimeout = 30 * time.Second

// Wipe asks the daemon to delete everything it keeps for the device and exit; with unlink it
// first tells WhatsApp to unlink the device. It returns the wiped reply, or a *ProtocolError such
// as not_linked, in which case nothing was deleted.
func (c *Client) Wipe(unlink bool) (Wiped, error) {
	if err := WriteFrame(c.conn, WipeRequest{V: Version, Op: OpWipe, Unlink: unlink}); err != nil {
		return Wiped{}, err
	}
	var resp Wiped
	timeout := c.timeout
	c.timeout = max(timeout, WipeTimeout)
	err := c.readReply(&resp)
	c.timeout = timeout
	if err != nil {
		return Wiped{}, err
	}
	if resp.Op != OpWiped {
		return Wiped{}, fmt.Errorf("ipc: expected op %s, got %q", OpWiped, resp.Op)
	}
	return resp, nil
}

// MediaTimeout bounds the wait for a media_ready reply: the daemon downloads the attachment from
// WhatsApp before it answers, which can take longer than an ordinary read.
const MediaTimeout = 90 * time.Second

// DownloadMedia asks the daemon for the attachment of message id in the chat jid. It returns the
// media_ready reply, whose Path is the cached file, or a *ProtocolError such as not_linked.
func (c *Client) DownloadMedia(jid, id string) (MediaReady, error) {
	if err := WriteFrame(c.conn, DownloadMediaRequest{V: Version, Op: OpDownloadMedia, JID: jid, ID: id}); err != nil {
		return MediaReady{}, err
	}
	var resp MediaReady
	timeout := c.timeout
	c.timeout = max(timeout, MediaTimeout)
	err := c.readReply(&resp)
	c.timeout = timeout
	if err != nil {
		return MediaReady{}, err
	}
	if resp.Op != OpMediaReady {
		return MediaReady{}, fmt.Errorf("ipc: expected op %s, got %q", OpMediaReady, resp.Op)
	}
	return resp, nil
}

// Close closes the connection.
func (c *Client) Close() error { return c.conn.Close() }

// readReply decodes the next frame into v, turning an error frame into a
// *ProtocolError.
func (c *Client) readReply(v any) error {
	if err := c.conn.SetReadDeadline(time.Now().Add(c.timeout)); err != nil {
		return fmt.Errorf("ipc: set read deadline: %w", err)
	}
	line, err := c.rd.Next()
	if err != nil {
		if errors.Is(err, os.ErrDeadlineExceeded) {
			return fmt.Errorf("ipc: the daemon did not answer within %v: %w", c.timeout, err)
		}
		return err
	}
	var head struct {
		Op      string `json:"op"`
		Code    string `json:"code"`
		Message string `json:"message"`
	}
	if err := json.Unmarshal(line, &head); err != nil {
		return fmt.Errorf("%w: %v", ErrBadFrame, err)
	}
	if head.Op == OpError {
		return &ProtocolError{Code: head.Code, Message: head.Message}
	}
	if err := json.Unmarshal(line, v); err != nil {
		return fmt.Errorf("%w: %v", ErrBadFrame, err)
	}
	return nil
}
