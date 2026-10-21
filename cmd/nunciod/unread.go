package main

import (
	"context"
	"fmt"
	"log"
	"time"

	"github.com/godbus/dbus/v5"
	"github.com/godbus/dbus/v5/introspect"
	"github.com/godbus/dbus/v5/prop"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// unreadWindow is how long the D-Bus face waits after the first change of a burst before it
// recounts: everything stored or read meanwhile lands in that one count, so the bus sees at most
// one emit per window (five a second), however many messages a reconnect brings in.
const unreadWindow = 200 * time.Millisecond

// unreadEmitter publishes a new total unread count: on the bus, the UnreadCount property (with
// PropertiesChanged) and the UnreadChanged signal.
type unreadEmitter interface {
	EmitUnread(count int32)
}

// unreadNotifier keeps the D-Bus UnreadCount current. Changed only flags that the total may have
// moved and never waits; run, on its own goroutine, waits one window after a flag, recounts from
// the store and emits only when the total differs from the last one emitted; a failed recount
// emits nothing. A slow or absent bus
// stalls run alone: flags raised meanwhile collapse into one recount.
type unreadNotifier struct {
	count  func() (int, error)
	emit   unreadEmitter
	window time.Duration
	kick   chan struct{}
	last   int // only run touches it after construction
}

func newUnreadNotifier(count func() (int, error), emit unreadEmitter, initial int, window time.Duration) *unreadNotifier {
	return &unreadNotifier{count: count, emit: emit, window: window, kick: make(chan struct{}, 1), last: initial}
}

// Changed says the total may have moved. It returns at once.
func (n *unreadNotifier) Changed() {
	select {
	case n.kick <- struct{}{}:
	default: // a recount is already pending; it will see this change too
	}
}

// run recounts and emits until ctx is done.
func (n *unreadNotifier) run(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case <-n.kick:
		}
		wait := time.NewTimer(n.window)
		select {
		case <-ctx.Done():
			wait.Stop()
			return
		case <-wait.C:
		}
		// A flag raised during the window is covered by the count taken now.
		select {
		case <-n.kick:
		default:
		}
		v, err := n.count()
		if err != nil {
			// A store that cannot count says nothing about the total: keep the last one, so a
			// passing SQLite error never clears the badge. The error is logged by its type only:
			// SQLite's text could quote a value.
			log.Printf("cannot recount the unread total for D-Bus (%T)", err)
			continue
		}
		if v != n.last {
			n.last = v
			n.emit.EmitUnread(int32(v))
		}
	}
}

// notifyUnread keeps emit current with the daemon's total unread (Status().Unread): the session
// flags every change that may move it, and so does a wipe (the total is 0 once the store is
// closed). Call it before the session starts; the caller runs the returned notifier.
func (d *daemon) notifyUnread(emit unreadEmitter, window time.Duration) *unreadNotifier {
	n := newUnreadNotifier(d.unreadCount, emit, d.unread(), window)
	d.unreadChanged = n.Changed
	if s, ok := d.session.(interface{ SetUnreadChanged(func()) }); ok {
		s.SetUnreadChanged(n.Changed)
	}
	return n
}

// unreadProp is the UnreadCount property: read-only, and announced with PropertiesChanged.
func unreadProp(initial int) *prop.Prop {
	return &prop.Prop{Value: int32(initial), Writable: false, Emit: prop.EmitTrue}
}

// introspectNode is the introspection data of the daemon's object: its interface (the Status
// method, the UnreadCount property, the UnreadChanged signal) and org.freedesktop.DBus.Properties;
// introspect.NewIntrospectable adds Introspectable and Peer.
func introspectNode() *introspect.Node {
	return &introspect.Node{
		Name: ipc.ObjectPath,
		Interfaces: []introspect.Interface{
			prop.IntrospectData,
			{
				Name:       ipc.Interface,
				Methods:    introspect.Methods(&dbusFace{}),
				Properties: []introspect.Property{unreadProp(0).Introspection(ipc.UnreadCountProperty)},
				Signals: []introspect.Signal{{
					Name: ipc.UnreadChangedSignal,
					Args: []introspect.Arg{{Name: "count", Type: "i", Direction: "out"}},
				}},
			},
		},
	}
}

// busUnread emits a new total on the session bus.
type busUnread struct {
	conn  *dbus.Conn
	props *prop.Properties
}

// EmitUnread sets UnreadCount, which sends PropertiesChanged, then sends UnreadChanged. A failure
// (the bus went away) is logged by its error type, without the count, and the daemon goes on.
func (b busUnread) EmitUnread(count int32) {
	if err := b.setProp(count); err != nil {
		log.Printf("cannot update %s (%T)", ipc.UnreadCountProperty, err)
	}
	if err := b.conn.Emit(dbus.ObjectPath(ipc.ObjectPath), ipc.Interface+"."+ipc.UnreadChangedSignal, count); err != nil {
		log.Printf("cannot emit %s (%T)", ipc.UnreadChangedSignal, err)
	}
}

// setProp is prop.SetMust without its panic: SetMust panics when PropertiesChanged cannot be sent.
func (b busUnread) setProp(count int32) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("%v", r)
		}
	}()
	b.props.SetMust(ipc.Interface, ipc.UnreadCountProperty, count)
	return nil
}
