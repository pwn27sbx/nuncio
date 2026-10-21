// Command nunciod is the nuncio daemon. It owns the one linked-device session
// and serves the NDJSON wire protocol (proto/wire.md) on
// $XDG_RUNTIME_DIR/nuncio/core.sock. Its session-bus face exists for
// activation, single instance and the unread count only: messages and media
// never travel over D-Bus.
//
// The daemon, not `nuncio link`, owns the session: if the store holds a linked
// device it connects; if not it stays unlinked and never starts a link. With a
// nuncio.db it stores incoming text and media messages and answers chats, messages and
// unread counts from it; without one it answers the shared fixture chat list.
// Attachments are downloaded only on a download_media request, into the private
// media cache $XDG_DATA_HOME/nuncio/media. It searches the stored messages of a
// chat without contacting WhatsApp, sends a reply that quotes a stored message,
// and sets or removes my reaction on one, once and only while linked.
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
	"sync/atomic"
	"syscall"
	"time"

	"github.com/godbus/dbus/v5"
	"github.com/godbus/dbus/v5/introspect"
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
	// What a wipe (nuncio logout or purge) deletes: the device keys in the keyring, the data dir,
	// and finally this socket.
	d.dataDir, d.keyring, d.socket = filepath.Dir(dbPath), store.SecretService{}, socket

	if bus != nil {
		emit, err := exportDBus(bus, d)
		if err != nil {
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
		// UnreadCount follows the store from here on; the notifier stops when run returns.
		notifyCtx, stopNotify := context.WithCancel(context.Background())
		defer stopNotify()
		go d.notifyUnread(emit, unreadWindow).run(notifyCtx)
	}

	// Connect only once this process is the one instance, so a second daemon
	// never opens a competing session. A failed connect leaves it offline and
	// retries after the backoff delay.
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

// exportDBus exports the daemon's object and returns what keeps its UnreadCount current.
func exportDBus(bus *dbus.Conn, d *daemon) (unreadEmitter, error) {
	path := dbus.ObjectPath(ipc.ObjectPath)
	// The D-Bus face is a separate type because godbus only exports methods
	// whose last return value is *dbus.Error, and the daemon's ipc.Handler
	// signature cannot carry that.
	if err := bus.Export(&dbusFace{d}, path, ipc.Interface); err != nil {
		return nil, fmt.Errorf("export %s on the session bus: %w", ipc.ObjectPath, err)
	}
	// UnreadCount is the daemon's only property: the store's total unread (Status().Unread, 0
	// while unlinked), kept current by notifyUnread, which also emits UnreadChanged.
	props, err := prop.Export(bus, path, map[string]map[string]*prop.Prop{
		ipc.Interface: {ipc.UnreadCountProperty: unreadProp(d.Status().Unread)},
	})
	if err != nil {
		return nil, fmt.Errorf("export the D-Bus properties of %s: %w", ipc.ObjectPath, err)
	}
	if err := bus.Export(introspect.NewIntrospectable(introspectNode()), path, "org.freedesktop.DBus.Introspectable"); err != nil {
		return nil, fmt.Errorf("export the introspection data of %s: %w", ipc.ObjectPath, err)
	}
	return busUnread{conn: bus, props: props}, nil
}

// dbusFace is the thin session-bus face of the daemon: the Status method and,
// through prop.Export, the UnreadCount property, with the UnreadChanged signal
// (unread.go). Message bodies and media never travel over D-Bus.
type dbusFace struct {
	d *daemon
}

func (f *dbusFace) Status() (ipc.Status, *dbus.Error) {
	return f.d.Status(), nil
}

// serve accepts connections until ctx is done or a connection's wipe succeeded (ipc.ErrWiped):
// then the daemon has nothing left to serve and serve returns, so the daemon exits.
func serve(ctx context.Context, ln net.Listener, h ipc.Handler, events *ipc.Broker) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
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
			err := ipc.ServeConnEvents(conn, h, events)
			if errors.Is(err, ipc.ErrWiped) {
				log.Print("wiped on request; exiting")
				cancel()
				return
			}
			if err != nil {
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

	// dataDir, keyring and socket are what Wipe deletes; an empty dataDir or socket, or a nil
	// keyring, is refused by Wipe rather than skipped.
	dataDir string
	keyring store.Keyring
	socket  string
	// closed is set once Wipe closed the session and its store: from then on nothing is served
	// from the store, even if the wipe failed and the daemon keeps serving.
	closed atomic.Bool
	// unreadChanged, when not nil, flags the D-Bus face that the total unread may have moved
	// (notifyUnread); it never blocks.
	unreadChanged func()
}

// errClosed answers a store read after a wipe closed the store.
var errClosed = errors.New("the local data is being deleted; run nuncio purge --confirm to finish")

// sessionState is what the daemon reads from, and sends through, its WhatsApp session.
type sessionState interface {
	State() wa.State
	// Send sends one text message through the outbox under the client's client_id; Retry sends a
	// not-sent one again by its client_id.
	Send(jid, text, clientID string) (ipc.Sent, error)
	Retry(clientID string) (ipc.Sent, error)
	// SendReply is Send quoting the message replyTo of that chat; React sets my reaction on a
	// message (empty emoji removes it), never queued while offline.
	SendReply(jid, text, clientID, replyTo string) (ipc.Sent, error)
	React(jid, id, emoji string) error
	// MarkRead marks a chat read when a UI opened it (the explicit view): read receipts to WhatsApp,
	// then the store; it answers the new total unread count.
	MarkRead(jid string) (int, error)
	// Logout tells WhatsApp to unlink the device through the daemon's own session.
	Logout(ctx context.Context) error
	// Close disconnects the session and closes the store.
	Close()
	// DownloadMedia returns the cached path and mime type of a message's attachment, downloading
	// it on this request only.
	DownloadMedia(jid, id string) (path, mime string, err error)
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
	n, err := d.unreadCount()
	if err != nil {
		log.Print(err)
		return 0
	}
	return n
}

// unreadCount is the store's total unread: 0 without a store or once a wipe closed it, and the
// store's error when it cannot count.
func (d *daemon) unreadCount() (int, error) {
	if d.db == nil || d.closed.Load() {
		return 0, nil
	}
	return store.TotalUnread(d.db)
}

// Chats lists the stored chats, newest message first, each with its name, unread
// count, flags (muted only while its mute lasts), aliases, masked phone and last message. Without
// a store it is the fixture list, the answer while unlinked. A store failure,
// or a store a wipe closed, is logged and answers no chats.
func (d *daemon) Chats() []ipc.Chat {
	if d.db == nil {
		return d.chats
	}
	if d.closed.Load() {
		log.Print(errClosed)
		return nil
	}
	rows, err := store.ListChats(d.db, ipc.MaxChatLimit)
	if err != nil {
		log.Print(err)
		return nil
	}
	chats := make([]ipc.Chat, 0, len(rows))
	now := time.Now()
	aliases, _ := d.session.(interface{ ChatAliases(string) []string })
	phones, _ := d.session.(interface{ ChatPhone(string) string })
	for _, r := range rows {
		c, err := wa.WireChat(d.db, r, now)
		if err != nil {
			log.Print(err)
			return nil
		}
		if aliases != nil {
			c.Aliases = aliases.ChatAliases(c.JID)
		}
		if phones != nil {
			c.Phone = phones.ChatPhone(c.JID)
		}
		chats = append(chats, c)
	}
	return chats
}

// Messages lists up to limit stored messages of a chat, newest first, each incoming group message
// with its sender; none without a store or for a chat it has never seen.
func (d *daemon) Messages(jid string, limit int) ([]ipc.Message, error) {
	if d.db == nil {
		return nil, nil
	}
	if d.closed.Load() {
		return nil, errClosed
	}
	rows, err := store.ListMessages(d.db, jid, limit)
	if err != nil {
		return nil, err
	}
	return wa.WireMessages(jid, rows, d.senders()), nil
}

// DownloadMedia serves download_media: the session downloads the attachment into the media cache
// on this request, or serves it from the cache, and answers its path and mime type. After a wipe
// closed the store nothing is served.
func (d *daemon) DownloadMedia(jid, id string) (string, string, error) {
	if d.closed.Load() {
		return "", "", errClosed
	}
	return d.session.DownloadMedia(jid, id)
}

// Send sends one text message through the session's outbox: sent with WhatsApp's id and
// timestamp, queued while offline, or the session's error (ipc.ErrNotLinked while unlinked,
// ipc.ErrNotConfirmed without an ack, a short not-sent reason otherwise). A client_id already
// stored answers that send's state and sends nothing. After a wipe closed the store nothing is
// sent.
func (d *daemon) Send(jid, text, clientID string) (ipc.Sent, error) {
	if d.closed.Load() {
		return ipc.Sent{}, errClosed
	}
	return d.session.Send(jid, text, clientID)
}

// SendReply serves a send with reply_to: like Send, quoting that message of the chat.
func (d *daemon) SendReply(jid, text, clientID, replyTo string) (ipc.Sent, error) {
	if d.closed.Load() {
		return ipc.Sent{}, errClosed
	}
	return d.session.SendReply(jid, text, clientID, replyTo)
}

// React serves react: the session sends my reaction once, or answers why it did not.
func (d *daemon) React(jid, id, emoji string) error {
	if d.closed.Load() {
		return errClosed
	}
	return d.session.React(jid, id, emoji)
}

// MarkRead serves mark_read, which a UI sends when the user opens a chat: that is the explicit view
// after which the session sends WhatsApp the read receipts of the chat's unread messages (read-self
// when the user turned read receipts off), then clears them in the store and answers the new total.
// Nothing else sends a read receipt. After a wipe closed the store nothing is sent.
func (d *daemon) MarkRead(jid string) (int, error) {
	if d.closed.Load() {
		return 0, errClosed
	}
	return d.session.MarkRead(jid)
}

// Search serves search: up to limit stored messages of a chat that contain every word of query,
// newest first, with their senders as Messages; none without a store.
func (d *daemon) Search(jid, query string, limit int) ([]ipc.Message, error) {
	if d.db == nil {
		return nil, nil
	}
	if d.closed.Load() {
		return nil, errClosed
	}
	rows, err := store.SearchChat(d.db, jid, query, limit)
	if err != nil {
		return nil, err
	}
	return wa.WireMessages(jid, rows, d.senders()), nil
}

// senders names the authors of the incoming group messages of one reply (wire Message `sender`):
// the session's resolver, which reads each sender's name once and only locally, or none for a
// session that has no names.
func (d *daemon) senders() func(string) string {
	if names, ok := d.session.(interface{ Senders() func(string) string }); ok {
		return names.Senders()
	}
	return nil
}

// Retry serves retry: the session sends a not-sent message again, once, under its first WhatsApp
// id, and answers any other send's stored state.
func (d *daemon) Retry(clientID string) (ipc.Sent, error) {
	if d.closed.Load() {
		return ipc.Sent{}, errClosed
	}
	return d.session.Retry(clientID)
}

// CanonicalJID serves ipc.ChatResolver: the chat a request naming jid is served and answered as,
// which is the contact's LID chat for a phone-number JID whose LID the session knows (ADR 0001),
// and jid itself otherwise, after a wipe too.
func (d *daemon) CanonicalJID(jid string) string {
	r, ok := d.session.(interface{ CanonicalJID(string) string })
	if !ok || d.closed.Load() {
		return jid
	}
	return r.CanonicalJID(jid)
}

// logoutTimeout bounds the wait for WhatsApp to confirm an unlink; the CLI waits ipc.WipeTimeout.
const logoutTimeout = 20 * time.Second

// Wipe is the ordered teardown behind `nuncio logout` (unlink) and `nuncio purge`: (1) with unlink,
// tell WhatsApp to unlink the device through this daemon's session, and stop with nothing deleted
// if it does not confirm; (2) close the session and the store, delete the keyring items and (3)
// the database, media cache and exports; (4) remove the socket. ServeConn then answers wiped and
// serve returns, so the daemon exits. Without unlink WhatsApp is never contacted. When the
// deletion fails the daemon keeps serving with the session closed (offline, or unlinked after a
// confirmed logout) and no store, so a retried purge through it can finish.
func (d *daemon) Wipe(unlink bool) error {
	if d.dataDir == "" || d.socket == "" || d.keyring == nil {
		return errors.New("this daemon cannot wipe: it does not know its data dir, keyring or socket")
	}
	if unlink {
		ctx, cancel := context.WithTimeout(context.Background(), logoutTimeout)
		defer cancel()
		if err := d.session.Logout(ctx); err != nil {
			return err
		}
	}
	d.session.Close()
	d.closed.Store(true)
	// The store is closed, so the total is 0 from now on: the bar's badge clears even when the
	// deletion below fails and the daemon keeps serving.
	if d.unreadChanged != nil {
		d.unreadChanged()
	}
	if err := store.Wipe(d.dataDir, d.keyring); err != nil {
		log.Print(err)
		return errors.New("could not delete all of nuncio's local data; run nuncio purge --confirm to finish")
	}
	if err := os.Remove(d.socket); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("the data is deleted, but the socket stays: %w", err)
	}
	return nil
}
