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

// Client is a decoded connection to the daemon, past the hello handshake.
type Client struct {
	conn net.Conn
	rd   *Reader
}

// Dial connects to the daemon's socket at path, sends hello and returns a
// ready client.
func Dial(path, clientName string) (*Client, error) {
	conn, err := net.Dial("unix", path)
	if err != nil {
		return nil, fmt.Errorf("ipc: dial %s: %w", path, err)
	}
	c := &Client{conn: conn, rd: NewReader(conn)}
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

// Close closes the connection.
func (c *Client) Close() error { return c.conn.Close() }

// readReply decodes the next frame into v, turning an error frame into a
// *ProtocolError.
func (c *Client) readReply(v any) error {
	line, err := c.rd.Next()
	if err != nil {
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
