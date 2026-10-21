package main

import (
	"bytes"
	"context"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/godbus/dbus/v5/introspect"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// testWindow is the coalescing window of the pure tests: short, so they run fast.
const testWindow = 30 * time.Millisecond

// fakeEmitter records every emitted count; while block is open (not closed) EmitUnread waits on it.
type fakeEmitter struct {
	mu    sync.Mutex
	got   []int32
	block chan struct{}
}

func (f *fakeEmitter) EmitUnread(n int32) {
	if f.block != nil {
		<-f.block
	}
	f.mu.Lock()
	f.got = append(f.got, n)
	f.mu.Unlock()
}

func (f *fakeEmitter) emitted() []int32 {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]int32(nil), f.got...)
}

// waitLast waits until the last emitted count is want, then a few more windows so a stray extra
// emit would show, and returns everything emitted.
func (f *fakeEmitter) waitLast(t *testing.T, want int32) []int32 {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if got := f.emitted(); len(got) > 0 && got[len(got)-1] == want {
			time.Sleep(5 * testWindow)
			return f.emitted()
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("emitted %v, never ending with %d", f.emitted(), want)
	return nil
}

func runNotifier(t *testing.T, n *unreadNotifier) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { n.run(ctx); close(done) }()
	t.Cleanup(func() { cancel(); <-done })
}

func TestUnreadNotifierEmitsOnceWhenTheCountChanges(t *testing.T) {
	var count atomic.Int64
	emit := &fakeEmitter{}
	n := newUnreadNotifier(func() (int, error) { return int(count.Load()), nil }, emit, 0, testWindow)
	runNotifier(t, n)
	count.Store(3)
	n.Changed()
	if got := emit.waitLast(t, 3); len(got) != 1 {
		t.Errorf("emitted %v after one change, want exactly [3]", got)
	}
}

func TestUnreadNotifierEmitsNothingWhenTheCountStaysTheSame(t *testing.T) {
	emit := &fakeEmitter{}
	n := newUnreadNotifier(func() (int, error) { return 2, nil }, emit, 2, testWindow)
	runNotifier(t, n)
	for range 3 {
		n.Changed()
		time.Sleep(2 * testWindow)
	}
	if got := emit.emitted(); len(got) != 0 {
		t.Errorf("emitted %v with the count unchanged at 2, want nothing", got)
	}
}

func TestUnreadNotifierCoalescesABurstIntoOneOrTwoEmitsWithTheFinalCount(t *testing.T) {
	var count atomic.Int64
	emit := &fakeEmitter{}
	n := newUnreadNotifier(func() (int, error) { return int(count.Load()), nil }, emit, 0, testWindow)
	runNotifier(t, n)
	for range 500 {
		count.Add(1)
		n.Changed()
	}
	got := emit.waitLast(t, 500)
	if len(got) < 1 || len(got) > 2 {
		t.Errorf("a burst of 500 changes emitted %d times (%v), want 1 or 2", len(got), got)
	}
}

func TestUnreadNotifierNeverBlocksTheCallerWhileTheEmitterIsStuck(t *testing.T) {
	var count atomic.Int64
	emit := &fakeEmitter{block: make(chan struct{})}
	n := newUnreadNotifier(func() (int, error) { return int(count.Load()), nil }, emit, 0, time.Millisecond)
	runNotifier(t, n)
	defer close(emit.block)
	done := make(chan struct{})
	go func() {
		for range 10000 {
			count.Add(1)
			n.Changed()
			time.Sleep(time.Microsecond)
		}
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("Changed blocked while the emitter was stuck")
	}
}

// unreadDaemon is a daemon over a real store and a started session, wired to a fake emitter.
func unreadDaemon(t *testing.T) (*daemon, *fakeWAClient, *fakeEmitter) {
	t.Helper()
	emit := &fakeEmitter{}
	d, fake := unreadDaemonWith(t, func(*daemon) unreadEmitter { return emit }, testWindow)
	return d, fake, emit
}

// unreadDaemonWith builds the daemon, wires the emitter newEmit makes for it, runs the notifier
// with that window and starts the session.
func unreadDaemonWith(t *testing.T, newEmit func(*daemon) unreadEmitter, window time.Duration) (*daemon, *fakeWAClient) {
	t.Helper()
	root := t.TempDir()
	dataDir := filepath.Join(root, "data", "nuncio")
	db, err := store.Open(filepath.Join(dataDir, store.DBFile))
	if err != nil {
		t.Fatalf("store.Open: %v", err)
	}
	if err := store.EnsureSchema(db); err != nil {
		t.Fatalf("EnsureSchema: %v", err)
	}
	fake := &fakeWAClient{}
	session := wa.NewSession(fake, db, db.Close)
	t.Cleanup(session.Close)
	session.SetEvents(ipc.NewBroker())
	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	d.dataDir, d.keyring, d.socket = dataDir, &fakeKeyring{items: map[string]string{}}, filepath.Join(root, "core.sock")
	runNotifier(t, d.notifyUnread(newEmit(d), window))
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	return d, fake
}

func TestStoredMessagesKeepTheDBusCountCurrentAndCoalesce(t *testing.T) {
	_, fake, emit := unreadDaemon(t)
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	at := time.Date(2026, 10, 18, 17, 58, 0, 0, time.UTC)
	fake.emit(textMessage(ada, "3EB0U000", false, at, "hi"))
	if got := emit.waitLast(t, 1); len(got) != 1 {
		t.Fatalf("one stored message emitted %v, want [1]", got)
	}
	// History arriving on reconnect: hundreds of messages back to back.
	start := time.Now()
	for i := 1; i < 500; i++ {
		fake.emit(textMessage(ada, fmt.Sprintf("3EB0U%03d", i), false, at.Add(time.Duration(i)*time.Second), "hi"))
	}
	elapsed := time.Since(start)
	got := emit.waitLast(t, 500)[1:]
	// One emit per window at most: the burst cannot emit more often than that.
	limit := int(elapsed/testWindow) + 2
	if len(got) < 1 || len(got) > limit {
		t.Errorf("499 more stored messages in %v emitted %d times (%v), want 1..%d", elapsed, len(got), got, limit)
	}
}

func TestWipeBringsTheDBusCountToZero(t *testing.T) {
	d, fake, emit := unreadDaemon(t)
	ada := types.NewJID("15551230001", types.DefaultUserServer)
	at := time.Date(2026, 10, 18, 17, 58, 0, 0, time.UTC)
	fake.emit(textMessage(ada, "3EB0W001", false, at, "hi"))
	fake.emit(textMessage(ada, "3EB0W002", false, at.Add(time.Second), "hi"))
	emit.waitLast(t, 2)
	if err := d.Wipe(false); err != nil {
		t.Fatalf("Wipe(false): %v", err)
	}
	got := emit.waitLast(t, 0)
	if got[len(got)-1] != 0 {
		t.Errorf("after a purge emitted %v, want it to end with 0", got)
	}
}

func TestIntrospectionListsTheUnreadPropertyAndSignal(t *testing.T) {
	xml, derr := introspect.NewIntrospectable(introspectNode()).Introspect()
	if derr != nil {
		t.Fatalf("Introspect: %v", derr)
	}
	for _, want := range []string{
		`<interface name="` + ipc.Interface + `">`,
		`<method name="Status">`,
		`<property name="` + ipc.UnreadCountProperty + `" type="i" access="read">`,
		`<annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true">`,
		`<signal name="` + ipc.UnreadChangedSignal + `">`,
		`<arg name="count" type="i" direction="out">`,
		`<interface name="org.freedesktop.DBus.Introspectable">`,
		`<interface name="org.freedesktop.DBus.Properties">`,
	} {
		if !strings.Contains(xml, want) {
			t.Errorf("introspection lacks %s:\n%s", want, xml)
		}
	}
}

// lidWAClient is fakeWAClient that knows one phone-number -> LID pair, as whatsmeow's LID map does.
type lidWAClient struct {
	*fakeWAClient
	pn, lid string
}

func (c *lidWAClient) LIDForPN(_ context.Context, pn string) (string, error) {
	if pn == c.pn {
		return c.lid, nil
	}
	return "", nil
}

func (c *lidWAClient) PNForLID(_ context.Context, lid string) (string, error) {
	if lid == c.lid {
		return c.pn, nil
	}
	return "", nil
}

func TestAMergeAtStartReachesTheDBusCount(t *testing.T) {
	db := openTestStore(t)
	pn, lid := "15551230001@s.whatsapp.net", "100000000000001@lid"
	at := time.Date(2026, 10, 18, 17, 58, 0, 0, time.UTC)
	// The same message stored under both chats of one contact: two unread until they are merged.
	for _, chat := range []string{pn, lid} {
		if _, err := store.InsertMessage(db, store.Message{ID: "D1", ChatJID: chat, Kind: store.KindText, Text: "hi", Timestamp: at, Sender: chat}); err != nil {
			t.Fatal(err)
		}
	}
	session := wa.NewSession(&lidWAClient{fakeWAClient: &fakeWAClient{}, pn: pn, lid: lid}, db, nil)
	t.Cleanup(session.Close)
	d, err := newDaemon(session, db)
	if err != nil {
		t.Fatalf("newDaemon: %v", err)
	}
	emit := &fakeEmitter{}
	runNotifier(t, d.notifyUnread(emit, testWindow))
	if err := session.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if got := emit.waitLast(t, 1); len(got) != 1 {
		t.Errorf("the merge at start emitted %v, want [1]", got)
	}
}

func TestUnreadNotifierSkipsTheEmitWhenTheCountFails(t *testing.T) {
	var fail atomic.Bool
	var count atomic.Int64
	count.Store(4)
	emit := &fakeEmitter{}
	n := newUnreadNotifier(func() (int, error) {
		if fail.Load() {
			return 0, fmt.Errorf("database is locked")
		}
		return int(count.Load()), nil
	}, emit, 3, testWindow)
	runNotifier(t, n)
	n.Changed()
	emit.waitLast(t, 4)
	fail.Store(true)
	n.Changed()
	time.Sleep(5 * testWindow)
	if got := emit.emitted(); len(got) != 1 {
		t.Errorf("a failed count emitted %v, want only the earlier [4]", got)
	}
	// Once the store answers again, an unchanged total still emits nothing.
	fail.Store(false)
	n.Changed()
	time.Sleep(5 * testWindow)
	if got := emit.emitted(); len(got) != 1 {
		t.Errorf("after the store recovered with the same total emitted %v, want [4]", got)
	}
}

// lockedBuffer is a log output that the notifier's goroutine and the test share.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

// A failed recount is logged by its error type only: SQLite's text could quote a value.
func TestAFailedRecountIsLoggedByItsTypeOnly(t *testing.T) {
	var buf lockedBuffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	n := newUnreadNotifier(func() (int, error) {
		return 0, fmt.Errorf("near \"Ada Lovelace\": syntax error")
	}, &fakeEmitter{}, 3, testWindow)
	runNotifier(t, n)
	n.Changed()
	deadline := time.Now().Add(5 * time.Second)
	for !strings.Contains(buf.String(), "cannot recount") && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	got := buf.String()
	if !strings.Contains(got, "cannot recount the unread total for D-Bus (*errors.errorString)") {
		t.Errorf("log = %q, want the failed recount by its error type", got)
	}
	if strings.Contains(got, "Ada") {
		t.Errorf("log = %q carries the error's text", got)
	}
}
