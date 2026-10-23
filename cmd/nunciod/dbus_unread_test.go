package main

// These tests need a real dbus-daemon (a private bus, see startPrivateBus): they skip when it is not
// on PATH, as on the Mac, and run at home on Linux.

import (
	"strings"
	"testing"
	"time"

	"github.com/godbus/dbus/v5"
	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

func TestDBusUnreadCountFollowsTheStoreWithOneSignalOfEachKind(t *testing.T) {
	bus := startPrivateBus(t, shortTempDir(t))
	daemonConn := busClient(t, bus.address)
	listener := busClient(t, bus.address)

	_, fake := unreadDaemonWith(t, func(d *daemon) unreadEmitter {
		emit, err := exportDBus(daemonConn, d)
		if err != nil {
			t.Fatalf("exportDBus: %v", err)
		}
		return emit
	}, unreadWindow)
	if reply, err := daemonConn.RequestName(ipc.DaemonName, dbus.NameFlagDoNotQueue); err != nil || reply != dbus.RequestNameReplyPrimaryOwner {
		t.Fatalf("RequestName = %v, %v", reply, err)
	}

	if err := listener.AddMatchSignal(dbus.WithMatchObjectPath(dbus.ObjectPath(ipc.ObjectPath))); err != nil {
		t.Fatalf("AddMatchSignal: %v", err)
	}
	signals := make(chan *dbus.Signal, 64)
	listener.Signal(signals)

	ada := types.NewJID("15551230001", types.DefaultUserServer)
	at := time.Date(2026, 10, 18, 17, 58, 0, 0, time.UTC)
	fake.emit(textMessage(ada, "3EB0D001", false, at, "hi"))
	fake.emit(textMessage(ada, "3EB0D002", false, at.Add(time.Second), "hi"))

	var changed, props int
	deadline := time.After(3 * time.Second)
	for changed+props < 2 {
		select {
		case sig := <-signals:
			switch sig.Name {
			case ipc.Interface + "." + ipc.UnreadChangedSignal:
				changed++
				if len(sig.Body) != 1 || sig.Body[0] != int32(2) {
					t.Errorf("%s body = %#v, want [int32(2)]", ipc.UnreadChangedSignal, sig.Body)
				}
			case "org.freedesktop.DBus.Properties.PropertiesChanged":
				props++
				got, _ := sig.Body[1].(map[string]dbus.Variant)
				if v := got[ipc.UnreadCountProperty]; v.Value() != int32(2) {
					t.Errorf("PropertiesChanged = %#v, want UnreadCount int32(2)", sig.Body)
				}
			}
		case <-deadline:
			t.Fatalf("got %d UnreadChanged and %d PropertiesChanged in 3s, want 1 of each", changed, props)
		}
	}
	if changed != 1 || props != 1 {
		t.Errorf("got %d UnreadChanged and %d PropertiesChanged, want 1 of each", changed, props)
	}
	// Both messages landed in one window: nothing more follows from the daemon's object (the bus
	// itself may still send this connection its NameAcquired, on another path).
	quiet := time.After(3 * unreadWindow)
	for done := false; !done; {
		select {
		case sig := <-signals:
			if sig.Path == dbus.ObjectPath(ipc.ObjectPath) {
				t.Errorf("an extra signal %s %#v after the burst", sig.Name, sig.Body)
			}
		case <-quiet:
			done = true
		}
	}

	var unread dbus.Variant
	if err := listener.Object(ipc.DaemonName, dbus.ObjectPath(ipc.ObjectPath)).
		Call("org.freedesktop.DBus.Properties.Get", 0, ipc.Interface, ipc.UnreadCountProperty).Store(&unread); err != nil {
		t.Fatalf("read %s: %v", ipc.UnreadCountProperty, err)
	}
	if unread.Value() != int32(2) {
		t.Errorf("%s = %#v, want int32(2)", ipc.UnreadCountProperty, unread.Value())
	}
}

func TestDBusIntrospectionListsUnreadCountAndUnreadChanged(t *testing.T) {
	bus := startPrivateBus(t, shortTempDir(t))
	daemonConn := busClient(t, bus.address)
	listener := busClient(t, bus.address)
	unreadDaemonWith(t, func(d *daemon) unreadEmitter {
		emit, err := exportDBus(daemonConn, d)
		if err != nil {
			t.Fatalf("exportDBus: %v", err)
		}
		return emit
	}, unreadWindow)
	if reply, err := daemonConn.RequestName(ipc.DaemonName, dbus.NameFlagDoNotQueue); err != nil || reply != dbus.RequestNameReplyPrimaryOwner {
		t.Fatalf("RequestName = %v, %v", reply, err)
	}
	var xml string
	if err := listener.Object(ipc.DaemonName, dbus.ObjectPath(ipc.ObjectPath)).
		Call("org.freedesktop.DBus.Introspectable.Introspect", 0).Store(&xml); err != nil {
		t.Fatalf("Introspect: %v", err)
	}
	for _, want := range []string{
		`<property name="` + ipc.UnreadCountProperty + `" type="i" access="read">`,
		`<signal name="` + ipc.UnreadChangedSignal + `">`,
		`<method name="Status">`,
	} {
		if !strings.Contains(xml, want) {
			t.Errorf("introspection over the bus lacks %s:\n%s", want, xml)
		}
	}
}
