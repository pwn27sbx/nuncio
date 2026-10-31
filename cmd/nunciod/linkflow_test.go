package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// fakeLinker is WhatsApp's linking flow without WhatsApp: it answers the codes the test wrote, in
// order, and waits for the flow's context when it runs out of them.
type fakeLinker struct {
	codes []wa.Code
	// blocked makes the stream wait forever, so a test can cancel a flow in progress.
	blocked bool
	// err is what QR answers instead of a stream.
	err error
	// sent records the codes the flow consumed, to prove it saw them.
	mu   sync.Mutex
	seen []string
}

func (l *fakeLinker) QR(ctx context.Context) (<-chan wa.Code, error) {
	if l.err != nil {
		return nil, l.err
	}
	out := make(chan wa.Code)
	go func() {
		defer close(out)
		for _, c := range l.codes {
			l.mu.Lock()
			l.seen = append(l.seen, c.Text)
			l.mu.Unlock()
			select {
			case out <- c:
			case <-ctx.Done():
				return
			}
		}
		if !l.blocked {
			return
		}
		<-ctx.Done()
	}()
	return out, nil
}

func (l *fakeLinker) Phone(context.Context, string) (string, error) {
	return "", errors.New("fakeLinker: no pairing code")
}

// withLinker replaces wa.Prepare for one test, so no flow can reach WhatsApp.
func withLinker(t *testing.T, l *fakeLinker) {
	t.Helper()
	prev := prepareLink
	prepareLink = func(wa.Consent, *whatsmeow.Client) (wa.Prepared, error) { return wa.Prepared{Linker: l}, nil }
	t.Cleanup(func() { prepareLink = prev })
}

// frames collects what a flow publishes, in order, so a test can wait for the one it needs.
type frames struct {
	mu     sync.Mutex
	got    []json.RawMessage
	cancel func()
}

func subscribe(t *testing.T, b *ipc.Broker) *frames {
	t.Helper()
	ch, cancel := b.Subscribe(nil)
	f := &frames{cancel: cancel}
	t.Cleanup(cancel)
	go func() {
		for raw := range ch {
			f.mu.Lock()
			f.got = append(f.got, json.RawMessage(append([]byte(nil), raw...)))
			f.mu.Unlock()
		}
	}()
	return f
}

// until waits for a frame with the op want and returns it decoded into out.
func (f *frames) until(t *testing.T, want string, out any) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		f.mu.Lock()
		for i, raw := range f.got {
			var head struct {
				Op string `json:"op"`
			}
			if err := json.Unmarshal(raw, &head); err != nil {
				f.mu.Unlock()
				t.Fatalf("a published frame is not JSON: %v", err)
			}
			if head.Op == want {
				f.got = append(f.got[:i], f.got[i+1:]...)
				f.mu.Unlock()
				if out != nil {
					if err := json.Unmarshal(raw, out); err != nil {
						t.Fatalf("decode %s: %v", want, err)
					}
				}
				return
			}
		}
		f.mu.Unlock()
		time.Sleep(5 * time.Millisecond)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	t.Fatalf("no %s frame was published; got %d frames", want, len(f.got))
}

// all returns every frame published so far.
func (f *frames) all() []json.RawMessage {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]json.RawMessage(nil), f.got...)
}

// linkFlowTest is a flow over an empty data directory, with a broker to watch and a paired stub
// that records the handover.
type linkFlowTest struct {
	flow   *linkFlow
	events *ipc.Broker
	frames *frames
	dir    string
	path   string
	kr     *lockableKeyring
	paired int
	mu     sync.Mutex
}

func newLinkFlowTest(t *testing.T) *linkFlowTest {
	t.Helper()
	dir, path := tmpfilesDataDir(t)
	events := ipc.NewBroker()
	lt := &linkFlowTest{events: events, frames: subscribe(t, events), dir: dir, path: path, kr: newLockableKeyring()}
	lt.flow = &linkFlow{dbPath: path, kr: lt.kr, events: events, paired: func() error {
		lt.mu.Lock()
		lt.paired++
		lt.mu.Unlock()
		return nil
	}}
	t.Cleanup(func() { _ = lt.flow.Cancel() })
	return lt
}

func (lt *linkFlowTest) handovers() int {
	lt.mu.Lock()
	defer lt.mu.Unlock()
	return lt.paired
}

// A link flow without the owner's consent does nothing at all: no store, no key, no frame.
func TestLinkFlowRefusesWithoutConsent(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{})
	if err := lt.flow.Start(false); !errors.Is(err, wa.ErrNoConsent) {
		t.Fatalf("Start(false) = %v, want wa.ErrNoConsent", err)
	}
	if _, err := os.Stat(lt.path); !errors.Is(err, os.ErrNotExist) {
		t.Error("a refused start created the store")
	}
	if lt.kr.count() != 0 {
		t.Error("a refused start wrote to the keyring")
	}
	if got := lt.frames.all(); len(got) != 0 {
		t.Errorf("a refused start published %d frames", len(got))
	}
}

// A flow publishes waiting, one link_qr per code WhatsApp offers and, once paired, linked — and the
// daemon gets the device.
func TestLinkFlowRotatesTheQRAndHandsTheDeviceOver(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{codes: []wa.Code{
		{Text: "2@first-payload", Event: "code", Timeout: time.Minute},
		{Text: "2@second-payload", Event: "code", Timeout: time.Minute},
		{Event: "success"},
	}})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	var state ipc.EventLinkState
	lt.frames.until(t, ipc.OpLinkState, &state)
	if state.State != ipc.LinkWaiting {
		t.Errorf("first state = %q, want %q", state.State, ipc.LinkWaiting)
	}
	for i := 0; i < 2; i++ {
		var qr ipc.EventLinkQR
		lt.frames.until(t, ipc.OpLinkQR, &qr)
		if qr.Size < 21 || qr.Size > ipc.MaxLinkQRSize || len(qr.Rows) != qr.Size {
			t.Fatalf("qr frame %d: size %d with %d rows", i, qr.Size, len(qr.Rows))
		}
		for _, row := range qr.Rows {
			if len(row) != qr.Size || strings.Trim(row, "01") != "" {
				t.Fatalf("qr frame %d: row %q is not %d modules", i, row, qr.Size)
			}
		}
	}
	lt.frames.until(t, ipc.OpLinkState, &state)
	if state.State != ipc.LinkLinked || state.Reason != "" {
		t.Errorf("last state = %+v, want linked with no reason", state)
	}
	if got := lt.handovers(); got != 1 {
		t.Errorf("handovers = %d, want 1", got)
	}
}

// A flow WhatsApp stops carries the one fixed sentence, and leaves nothing of its own behind.
func TestLinkFlowFailureCarriesTheFixedSentenceAndLeavesNothing(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{codes: []wa.Code{
		{Text: "2@payload", Event: "code", Timeout: time.Minute},
		{Event: "error"},
	}})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	var state ipc.EventLinkState
	lt.frames.until(t, ipc.OpLinkState, &state) // waiting
	lt.frames.until(t, ipc.OpLinkQR, nil)
	lt.frames.until(t, ipc.OpLinkState, &state)
	if state.State != ipc.LinkFailed || state.Reason != ipc.LinkFailureReason {
		t.Fatalf("last state = %+v, want failed with %q", state, ipc.LinkFailureReason)
	}
	if lt.handovers() != 0 {
		t.Error("a failed flow handed a device over")
	}
	// Cancel waits for a flow that already ended: what it created is already gone.
	if err := lt.flow.Cancel(); err != nil {
		t.Fatalf("Cancel: %v", err)
	}
	if _, err := os.Stat(lt.path); !errors.Is(err, os.ErrNotExist) {
		t.Error("a failed flow left its store behind")
	}
	if lt.kr.count() != 0 {
		t.Errorf("a failed flow left %d keyring items behind", lt.kr.count())
	}
}

// Cancel stops a flow in progress and removes the store and the data key the flow made, so the
// client that reads link_cancelled finds nothing behind.
func TestLinkFlowCancelRemovesOnlyWhatItCreated(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{blocked: true})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if !lt.kr.hasDataKey() {
		t.Fatal("the flow did not create the data key")
	}
	if err := lt.flow.Cancel(); err != nil {
		t.Fatalf("Cancel: %v", err)
	}
	// Cancel is the outcome: no failure sentence says WhatsApp stopped what the user stopped.
	for _, raw := range lt.frames.all() {
		if strings.Contains(string(raw), ipc.LinkFailureReason) {
			t.Errorf("a cancelled flow published a failure: %s", raw)
		}
	}
	for _, p := range []string{lt.path, lt.path + "-wal", lt.path + "-shm"} {
		if _, err := os.Stat(p); !errors.Is(err, os.ErrNotExist) {
			t.Errorf("%s survived the cancel", filepath.Base(p))
		}
	}
	if lt.kr.count() != 0 {
		t.Errorf("the cancel left %d keyring items behind", lt.kr.count())
	}
	// A second cancel has nothing to do and is not an error.
	if err := lt.flow.Cancel(); err != nil {
		t.Errorf("second Cancel: %v", err)
	}
}

// A store that already holds a linked device refuses the flow before anything is written, and
// cancel cannot touch it: the device still opens afterwards.
func TestLinkFlowRefusesALinkedDeviceAndCancelLeavesIt(t *testing.T) {
	path, kr := sealedStore(t)
	events := ipc.NewBroker()
	f := subscribe(t, events)
	flow := &linkFlow{dbPath: path, kr: kr, events: events, paired: func() error { return nil }}
	if err := flow.Start(true); !errors.Is(err, errLinkRefused) {
		t.Fatalf("Start over a linked store = %v, want errLinkRefused", err)
	}
	if err := flow.Cancel(); err != nil {
		t.Fatalf("Cancel: %v", err)
	}
	if got := f.all(); len(got) != 0 {
		t.Errorf("a refused flow published %d frames", len(got))
	}
	db, err := store.Open(path)
	if err != nil {
		t.Fatalf("the linked store is gone: %v", err)
	}
	defer func() { _ = db.Close() }()
	device, _, err := wa.OpenDevice(context.Background(), db, path, kr, false)
	if err != nil {
		t.Fatalf("the linked device no longer opens: %v", err)
	}
	if device.ID == nil || device.ID.User != sealedJID.User {
		t.Errorf("linked device is %v, want %s", device.ID, sealedJID.User)
	}
}

// A flow that ended lets the next one start: a failed pairing does not leave the daemon stuck.
func TestLinkFlowStartsAgainAfterAFailedOne(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{codes: []wa.Code{{Event: "timeout"}}})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	var state ipc.EventLinkState
	lt.frames.until(t, ipc.OpLinkState, &state) // waiting
	lt.frames.until(t, ipc.OpLinkState, &state)
	if state.State != ipc.LinkFailed {
		t.Fatalf("state = %q, want failed", state.State)
	}
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start after a failed flow: %v", err)
	}
	lt.frames.until(t, ipc.OpLinkState, &state)
	if state.State != ipc.LinkWaiting {
		t.Errorf("the new flow's first state = %q, want waiting", state.State)
	}
}

// A second link_start while a flow runs is refused instead of pairing two clients over one account.
func TestLinkFlowRefusesASecondStart(t *testing.T) {
	lt := newLinkFlowTest(t)
	withLinker(t, &fakeLinker{blocked: true})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if err := lt.flow.Start(true); !errors.Is(err, errLinkBusy) {
		t.Errorf("second Start = %v, want errLinkBusy", err)
	}
	if err := lt.flow.Cancel(); err != nil {
		t.Fatalf("Cancel: %v", err)
	}
}

// The QR payload is ephemeral pairing material: it never reaches a frame, a log line or an error.
func TestLinkFlowNeverCarriesThePayload(t *testing.T) {
	log := captureLog(t)
	lt := newLinkFlowTest(t)
	const payload = "2@the-payload-that-pairs-the-account"
	withLinker(t, &fakeLinker{codes: []wa.Code{
		{Text: payload, Event: "code", Timeout: time.Minute},
		{Event: "error"},
	}})
	if err := lt.flow.Start(true); err != nil {
		t.Fatalf("Start: %v", err)
	}
	var state ipc.EventLinkState
	lt.frames.until(t, ipc.OpLinkState, &state) // waiting
	lt.frames.until(t, ipc.OpLinkQR, nil)
	lt.frames.until(t, ipc.OpLinkState, &state)
	for _, raw := range lt.frames.all() {
		if strings.Contains(string(raw), "the-payload") {
			t.Errorf("a published frame carries the payload: %s", raw)
		}
	}
	if strings.Contains(log.String(), "the-payload") {
		t.Errorf("the log carries the payload: %q", log.String())
	}
	if strings.Contains(state.Reason, "the-payload") {
		t.Errorf("the failure reason carries the payload: %q", state.Reason)
	}
}

// A matrix is what the wire allows: at most 177 modules per side, every row that long, and a
// payload that does not fit is refused without being echoed.
func TestLinkQRStaysInsideTheWireLimits(t *testing.T) {
	size, rows, err := linkQR("2@synthetic-payload")
	if err != nil {
		t.Fatalf("linkQR: %v", err)
	}
	if size < 21 || size > ipc.MaxLinkQRSize {
		t.Errorf("size = %d, want 21..%d", size, ipc.MaxLinkQRSize)
	}
	if len(rows) != size {
		t.Errorf("%d rows for size %d", len(rows), size)
	}
	for _, row := range rows {
		if len(row) != size || strings.Trim(row, "01") != "" {
			t.Fatalf("row %q is not %d modules", row, size)
		}
	}
	if _, _, err := linkQR(strings.Repeat("x", 10000)); err == nil {
		t.Fatal("an oversized payload was encoded")
	} else if strings.Contains(err.Error(), "xxxx") {
		t.Errorf("the error carries the payload: %v", err)
	}
}

// Once a flow pairs, the daemon serves the new device: status says linked and the chats come from
// the store, without a restart.
func TestADaemonServesTheDeviceALinkFlowPaired(t *testing.T) {
	// A store with a device and its key, as pairing leaves it.
	path, kr := sealedStore(t)
	fake := &fakeWAClient{}
	db, err := store.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })
	prev := openDaemonSession
	openDaemonSession = func(context.Context, string, store.Keyring) (daemonSession, *sql.DB, error) {
		return wa.NewSession(fake, nil, nil), db, nil
	}
	t.Cleanup(func() { openDaemonSession = prev })

	events := ipc.NewBroker()
	session := wa.NewSession(nil, nil, nil)
	d, err := newDaemon(session, nil)
	if err != nil {
		t.Fatal(err)
	}
	d.dbPath, d.keyring, d.events = path, kr, events
	if got := d.Status(); got.Session != ipc.SessionUnlinked {
		t.Fatalf("before pairing: session %q, want unlinked", got.Session)
	}
	if err := d.linkedDevice(); err != nil {
		t.Fatalf("linkedDevice: %v", err)
	}
	defer d.sess().Close()
	if got := d.Status(); !got.Linked || got.Session == ipc.SessionUnlinked {
		t.Errorf("after pairing: %+v, want linked", got)
	}
	if d.store() == nil {
		t.Error("the daemon still has no store after pairing")
	}
}
