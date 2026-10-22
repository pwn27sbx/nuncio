package main

import (
	"bufio"
	"context"
	"net"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// gateServed opens the daemon over path with kr as nunciod does, with fake timers and start, and
// serves it with a subscribed client.
func gateServed(t *testing.T, path string, kr store.Keyring) (*gate, *manualTimers, *atomic.Int64, *daemon, net.Conn, *bufio.Reader) {
	t.Helper()
	s, db, err := openSession(context.Background(), path, kr)
	if err != nil {
		t.Fatalf("openSession: %v", err)
	}
	g, timers, starts := asGate(t, s)
	t.Cleanup(g.Close)
	events := ipc.NewBroker()
	g.SetEvents(events)
	d, err := newDaemon(g, db)
	if err != nil {
		t.Fatal(err)
	}
	ui, uiR := subscriber(t, serveSocket(t, d, events))
	return g, timers, starts, d, ui, uiR
}

const (
	keyringLockedEvent    = `{"v":1,"op":"event_connection","session":"offline","stop_reason":"keyring_locked"}`
	deviceUnreadableEvent = `{"v":1,"op":"event_connection","session":"offline","stop_reason":"device_unreadable"}`
	clearedOfflineEvent   = `{"v":1,"op":"event_connection","session":"offline"}`
)

// D4: while the keyring is locked, status and event_connection say keyring_locked (linked, offline);
// a retry that fails the same way publishes nothing new; once the keyring opens and the device
// opens, an event_connection without stop_reason clears it, as a Q6 stop is cleared, and status
// has none.
func TestALockedKeyringIsReportedAsKeyringLockedUntilTheDeviceOpens(t *testing.T) {
	log := captureLog(t)
	path, kr := sealedStore(t)
	kr.locked.Store(true)
	g, timers, starts, d, ui, uiR := gateServed(t, path, kr)

	if got := d.Status(); got.StopReason != "" || !got.Linked || got.Session != ipc.SessionOffline {
		t.Errorf("Status() while opening = %+v, want offline, linked, no stop_reason", got)
	}
	_ = g.Start()
	if !timers.fire() {
		t.Fatal("Start scheduled no attempt")
	}
	if got := readFrame(t, ui, uiR, 5*time.Second); got != keyringLockedEvent {
		t.Errorf("event after the locked attempt = %s, want %s", got, keyringLockedEvent)
	}
	want := ipc.Status{Daemon: daemonVersion, Linked: true, Session: ipc.SessionOffline, StopReason: ipc.StopKeyringLocked}
	if got := d.Status(); got != want {
		t.Errorf("Status() = %+v, want %+v", got, want)
	}
	if !strings.Contains(log.String(), "the keyring (Secret Service) is locked or unavailable") {
		t.Errorf("log %q, want the keyring line kept", log.String())
	}

	if !timers.fire() {
		t.Fatal("no retry scheduled")
	}
	if got := readFrame(t, ui, uiR, 200*time.Millisecond); got != "" {
		t.Errorf("a retry that failed the same way published %s, want nothing", got)
	}

	kr.locked.Store(false)
	if !timers.fire() || starts.Load() != 1 {
		t.Fatalf("the device did not open once the keyring did (%d starts)", starts.Load())
	}
	if got := readFrame(t, ui, uiR, 5*time.Second); got != clearedOfflineEvent {
		t.Errorf("event once the device opened = %s, want %s", got, clearedOfflineEvent)
	}
	if got := d.Status(); got.StopReason != "" {
		t.Errorf("Status() once the device opened has stop_reason %q, want none", got.StopReason)
	}
}

// D4: with the data key gone over a sealed device, status and event_connection say
// device_unreadable (linked, offline); a keyring that was locked first says keyring_locked, then
// device_unreadable once it opens on the missing key.
func TestAnUnreadableDeviceIsReportedAsDeviceUnreadable(t *testing.T) {
	captureLog(t)
	path, kr := sealedStore(t)
	if err := kr.Delete(store.KeyringService, "store.data_key"); err != nil {
		t.Fatal(err)
	}
	kr.locked.Store(true)
	g, timers, starts, d, ui, uiR := gateServed(t, path, kr)
	_ = g.Start()
	timers.fire()
	if got := readFrame(t, ui, uiR, 5*time.Second); got != keyringLockedEvent {
		t.Errorf("event while locked = %s, want %s", got, keyringLockedEvent)
	}

	kr.locked.Store(false)
	if !timers.fire() {
		t.Fatal("no retry scheduled while locked")
	}
	if got := readFrame(t, ui, uiR, 5*time.Second); got != deviceUnreadableEvent {
		t.Errorf("event on the missing key = %s, want %s", got, deviceUnreadableEvent)
	}
	want := ipc.Status{Daemon: daemonVersion, Linked: true, Session: ipc.SessionOffline, StopReason: ipc.StopDeviceUnreadable}
	if got := d.Status(); got != want {
		t.Errorf("Status() = %+v, want %+v", got, want)
	}
	if timers.fire() || starts.Load() != 0 {
		t.Errorf("an unreadable device was retried or started (%d starts)", starts.Load())
	}
}

// The gate's other reasons (still opening, a pending cleanup, a store that fails) carry no
// stop_reason: status stays the plain offline it was.
func TestTheGatesOtherReasonsCarryNoStopReason(t *testing.T) {
	for _, reason := range []error{errOpening, errCleanupWait, errStoreFailed} {
		if got := (&gate{reason: reason}).StopKind(); got != "" {
			t.Errorf("StopKind with %q = %q, want none", reason, got)
		}
	}
}
