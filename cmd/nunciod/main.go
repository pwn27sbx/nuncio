// Command nunciod is the nuncio daemon. It owns the one linked-device session
// and serves the NDJSON wire protocol (proto/wire.md) on
// $XDG_RUNTIME_DIR/nuncio/core.sock. Its session-bus face exists for
// activation, single instance and the unread count only: messages and media
// never travel over D-Bus.
//
// The daemon, not `nuncio link`, owns the session: if the store holds a linked
// device it connects; if not it stays unlinked and never starts a link. With a
// nuncio.db it stores incoming text messages and answers chats, messages and
// unread counts from it; without one it answers the shared fixture chat list.
package main

import (
	"context"
	"database/sql"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"log"
	"net"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/godbus/dbus/v5"
	"github.com/godbus/dbus/v5/prop"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// daemonVersion is the value of the `daemon` field in every frame.
const daemonVersion = "nunciod/0.1.0"

func main() {
	flag.Parse()
	log.SetPrefix("nunciod: ")
	log.SetFlags(0)
	if err := run(); err != nil {
		log.Print(err)
		os.Exit(1)
	}
}

func run() error {
	socket, err := ipc.DefaultSocketPath()
	if err != nil {
		return err
	}

	// The session bus is optional: without it the daemon still serves the
	// socket, and single instance falls back to the socket check in listen.
	bus, err := dbus.SessionBus()
	if err != nil {
		log.Printf("no session bus (%v): the D-Bus face is off and single instance rests on the socket alone", err)
		bus = nil
	} else if nameIsOwned(bus) {
		log.Printf("another nunciod already owns %s; exiting", ipc.DaemonName)
		return nil
	}

	ln, err := listen(socket)
	if errors.Is(err, errAlreadyRunning) {
		log.Printf("another nunciod is already serving %s; exiting", socket)
		return nil
	}
	if err != nil {
		return err
	}
	defer func() {
		_ = ln.Close()
		_ = os.Remove(socket)
	}()

	dbPath, err := wa.DBPath()
	if err != nil {
		return err
	}
	// Opened only once the socket is ours, so a daemon that loses the race
	// never touches the store. A missing nuncio.db means unlinked; nothing is
	// created to find that out.
	session, err := wa.OpenSession(context.Background(), dbPath)
	if err != nil {
		return err
	}
	defer session.Close()
	// Every connection that subscribes gets the session's events (new messages, unread and
	// connection changes) through this broker.
	events := ipc.NewBroker()
	session.SetEvents(events)
	d, err := newDaemon(session, session.Store())
	if err != nil {
		return err
	}

	if bus != nil {
		if err := exportDBus(bus, d); err != nil {
			return err
		}
		reply, err := bus.RequestName(ipc.DaemonName, dbus.NameFlagDoNotQueue)
		if err != nil {
			return fmt.Errorf("request the D-Bus name %s: %w", ipc.DaemonName, err)
		}
		if reply != dbus.RequestNameReplyPrimaryOwner {
			log.Printf("another nunciod already owns %s; exiting", ipc.DaemonName)
			return nil
		}
	}

	// Connect only once this process is the one instance, so a second daemon
	// never opens a competing session. A failed connect leaves it offline.
	if err := session.Start(); err != nil {
		log.Print(err)
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	log.Printf("serving %s", socket)
	return serve(ctx, ln, d, events)
}

// errAlreadyRunning reports a daemon that answers on the socket already.
var errAlreadyRunning = errors.New("socket is already served")

// listen creates the daemon's socket, replacing a file left behind by a dead
// daemon and refusing to disturb a live one.
func listen(socket string) (net.Listener, error) {
	dir := filepath.Dir(socket)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("create %s: %w", dir, err)
	}
	// MkdirAll leaves the mode of an existing directory alone, so 0700 is
	// enforced explicitly.
	if err := os.Chmod(dir, 0o700); err != nil {
		return nil, fmt.Errorf("chmod %s: %w", dir, err)
	}

	if socketIsLive(socket) {
		return nil, errAlreadyRunning
	}
	if _, err := os.Lstat(socket); err == nil {
		// Nothing answers on it, so it is left over from a dead daemon.
		if err := os.Remove(socket); err != nil {
			return nil, fmt.Errorf("remove the stale socket %s: %w", socket, err)
		}
	} else if !errors.Is(err, fs.ErrNotExist) {
		return nil, err
	}

	ln, err := net.Listen("unix", socket)
	if err != nil {
		return nil, fmt.Errorf("listen on %s: %w", socket, err)
	}
	if err := os.Chmod(socket, 0o600); err != nil {
		_ = ln.Close()
		return nil, fmt.Errorf("chmod %s: %w", socket, err)
	}
	return ln, nil
}

// socketIsLive reports whether something answers on the socket. A socket file
// whose owner died refuses the connection.
func socketIsLive(socket string) bool {
	conn, err := net.DialTimeout("unix", socket, 200*time.Millisecond)
	if err != nil {
		return false
	}
	_ = conn.Close()
	return true
}

func nameIsOwned(bus *dbus.Conn) bool {
	call := bus.Object("org.freedesktop.DBus", dbus.ObjectPath("/org/freedesktop/DBus")).
		Call("org.freedesktop.DBus.GetNameOwner", 0, ipc.DaemonName)
	// An unowned name answers with NameHasNoOwner, which is the answer we
	// want; any other failure surfaces when we request the name.
	if call.Err != nil {
		return false
	}
	var owner string
	return call.Store(&owner) == nil && owner != ""
}

func exportDBus(bus *dbus.Conn, d *daemon) error {
	// The D-Bus face is a separate type because godbus only exports methods
	// whose last return value is *dbus.Error, and the daemon's ipc.Handler
	// signature cannot carry that.
	if err := bus.Export(&dbusFace{d}, dbus.ObjectPath(ipc.ObjectPath), ipc.Interface); err != nil {
		return fmt.Errorf("export %s on the session bus: %w", ipc.ObjectPath, err)
	}
	// UnreadCount is the daemon's only property: the store's total unread at
	// startup (0 while unlinked). Keeping it current and emitting the
	// UnreadChanged signal the contract names come with event streaming.
	if _, err := prop.Export(bus, dbus.ObjectPath(ipc.ObjectPath), map[string]map[string]*prop.Prop{
		ipc.Interface: {
			"UnreadCount": {Value: int32(d.Status().Unread), Writable: false, Emit: prop.EmitTrue},
		},
	}); err != nil {
		return fmt.Errorf("export the D-Bus properties of %s: %w", ipc.ObjectPath, err)
	}
	return nil
}

// dbusFace is the thin session-bus face of the daemon: the Status method and,
// through prop.Export, the UnreadCount property. Message bodies and media never
// travel over D-Bus.
type dbusFace struct {
	d *daemon
}

func (f *dbusFace) Status() (ipc.Status, *dbus.Error) {
	return f.d.Status(), nil
}

func serve(ctx context.Context, ln net.Listener, h ipc.Handler, events *ipc.Broker) error {
	go func() {
		<-ctx.Done()
		_ = ln.Close()
	}()
	for {
		conn, err := ln.Accept()
		if err != nil {
			if errors.Is(err, net.ErrClosed) {
				return nil
			}
			return err
		}
		go func() {
			if err := ipc.ServeConnEvents(conn, h, events); err != nil {
				log.Printf("connection: %v", err)
			}
		}()
	}
}

// daemon is the ipc.Handler behind both the socket protocol and the D-Bus
// face.
type daemon struct {
	version string
	session sessionState
	// db is the message store, nil while there is no nuncio.db (unlinked).
	db    *sql.DB
	chats []ipc.Chat
}

// sessionState is what the daemon reads from its WhatsApp session.
type sessionState interface {
	State() wa.State
}

// newDaemon serves session and, when db is not nil, the chats, messages and
// unread counts stored in db.
func newDaemon(session sessionState, db *sql.DB) (*daemon, error) {
	chats, err := fixtureChats()
	if err != nil {
		return nil, err
	}
	return &daemon{version: daemonVersion, session: session, db: db, chats: chats}, nil
}

// Hello records the handshake and answers with the session's current state.
func (d *daemon) Hello(client string) ipc.Status {
	log.Printf("hello from %s", client)
	return d.Status()
}

// Status is also the D-Bus method of the same name. Linked means paired, so an
// offline session is still linked. Unread is the store's total, 0 without one.
func (d *daemon) Status() ipc.Status {
	st := d.session.State()
	return ipc.Status{Daemon: d.version, Linked: st != wa.StateUnlinked, Session: string(st), Unread: d.unread()}
}

func (d *daemon) unread() int {
	if d.db == nil {
		return 0
	}
	n, err := store.TotalUnread(d.db)
	if err != nil {
		log.Print(err)
		return 0
	}
	return n
}

// Chats lists the stored chats, newest message first, each with its unread
// count and last message. Without a store it is the fixture list, the answer
// while unlinked. A store failure is logged and answers no chats.
func (d *daemon) Chats() []ipc.Chat {
	if d.db == nil {
		return d.chats
	}
	rows, err := store.ListChats(d.db, ipc.MaxChatLimit)
	if err != nil {
		log.Print(err)
		return nil
	}
	chats := make([]ipc.Chat, 0, len(rows))
	for _, r := range rows {
		c := ipc.Chat{JID: r.JID, Name: r.Name, Unread: r.Unread, Archived: r.Archived, Pinned: r.Pinned}
		last, err := store.ListMessages(d.db, r.JID, 1)
		if err != nil {
			log.Print(err)
			return nil
		}
		if len(last) == 1 {
			m := wireMessage(last[0])
			c.LastMessage = &m
		}
		chats = append(chats, c)
	}
	return chats
}

// Messages lists up to limit stored messages of a chat, newest first; none
// without a store or for a chat it has never seen.
func (d *daemon) Messages(jid string, limit int) ([]ipc.Message, error) {
	if d.db == nil {
		return nil, nil
	}
	rows, err := store.ListMessages(d.db, jid, limit)
	if err != nil {
		return nil, err
	}
	msgs := make([]ipc.Message, 0, len(rows))
	for _, r := range rows {
		msgs = append(msgs, wireMessage(r))
	}
	return msgs, nil
}

// wireMessage is a stored text message in its wire shape. Media fields are a
// later task: only text is stored for now.
func wireMessage(m store.Message) ipc.Message {
	return ipc.Message{ID: m.ID, Kind: m.Kind, Text: m.Text, FromMe: m.FromMe, Timestamp: ipc.Timestamp(m.Timestamp)}
}
