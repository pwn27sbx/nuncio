package main

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"sync"

	"go.mau.fi/whatsmeow"
	"rsc.io/qr"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// A link flow is the only thing that pairs a device, and it runs here, inside the daemon: a client
// asks link_start with the user's explicit consent, the daemon generates the QR codes and
// publishes them, and the client only draws them (proto/wire.md, "The link flow"). At most one
// flow runs at a time, and the flow owns the store and the data key it opened, so a cancel removes
// only what this flow created and never a device that was already linked.

// errLinkBusy refuses a second link_start while a flow runs: two whatsmeow clients pairing the same
// account would take the stream from each other, and a cancel could not tell which one it owns.
var errLinkBusy = errors.New("a link flow is already running")

// prepareLink is wa.Prepare; tests replace it with a fake Linker so that no test reaches WhatsApp.
var prepareLink = wa.Prepare

// linkFlow is the daemon's one device-linking flow, reused by every link_start.
type linkFlow struct {
	// dbPath and kr are what the flow opens and writes; events is where it publishes link_qr and
	// link_state; paired hands the new device to the daemon, which opens and starts the session it
	// serves. A flow without any of them refuses to start.
	dbPath string
	kr     store.Keyring
	events *ipc.Broker
	paired func() error

	mu        sync.Mutex
	starting  bool          // a start is opening the store and the keyring
	startDone chan struct{} // closed when that start finishes
	canceled  bool          // a cancel arrived while starting
	run       *linkRun      // the flow in progress, nil when none
}

// linkRun is one flow in progress: what it opened and how to stop it. cleanupErr is what removing
// that turned out to be impossible, written before done closes and read by the cancel that waited.
type linkRun struct {
	cancel     context.CancelFunc
	done       chan struct{}
	boot       *linkBootstrap
	client     *whatsmeow.Client
	cleanupErr error
}

// Start begins a flow and returns once it runs. It refuses without the owner's consent before it
// opens anything, while a flow already runs, and while a device is already linked or the keyring
// cannot be read (openLinkBootstrap's refusals).
func (f *linkFlow) Start(consent bool) error {
	// The gate is the same one `nuncio link` passes: nothing below it has a side effect.
	if err := wa.Gate(wa.Consent{Granted: consent}); err != nil {
		return err
	}
	if f.dbPath == "" || f.kr == nil || f.events == nil || f.paired == nil {
		return errors.New("this daemon cannot link a device: it does not know its data dir, keyring or session")
	}
	f.mu.Lock()
	if f.starting || f.run != nil {
		f.mu.Unlock()
		return errLinkBusy
	}
	f.starting = true
	f.startDone = make(chan struct{})
	done := f.startDone
	f.mu.Unlock()

	// Outside the lock: the keyring may wait on an unlock prompt for as long as it likes, and a
	// cancel must not hold the flow's lock while it does.
	boot, err := openLinkBootstrap(context.Background(), f.dbPath, f.kr)
	f.mu.Lock()
	f.starting = false
	f.startDone = nil
	canceled := f.canceled
	f.canceled = false
	if err != nil {
		f.mu.Unlock()
		close(done)
		return err
	}
	if canceled {
		// A link_cancel arrived while the keyring blocked: the flow never ran, so it removes what
		// this start created before the cancel is answered, and reports what it could not remove.
		f.mu.Unlock()
		err := f.discard(boot)
		close(done)
		return err
	}
	ctx, cancel := context.WithCancel(context.Background())
	r := &linkRun{cancel: cancel, done: make(chan struct{}), boot: boot}
	f.run = r
	f.mu.Unlock()
	close(done)
	go f.serve(ctx, r, consent)
	return nil
}

// Cancel stops the flow in progress and returns once it has removed everything it created, so the
// link_cancelled reply the client reads is the truth. With no flow it does nothing.
func (f *linkFlow) Cancel() error {
	f.mu.Lock()
	if f.starting {
		// The start is still opening the store and the keyring: it removes what it made before it
		// answers, and this cancel waits for that.
		f.canceled = true
		done := f.startDone
		f.mu.Unlock()
		<-done
		return nil
	}
	r := f.run
	f.mu.Unlock()
	if r == nil {
		return nil
	}
	r.cancel()
	<-r.done
	return r.cleanupErr
}

// serve runs one flow until it pairs, fails or is cancelled, and removes what it created when it
// does not pair.
func (f *linkFlow) serve(ctx context.Context, r *linkRun, consent bool) {
	// Whatever the linker started ends with the flow: its QR producer waits on this context.
	defer r.cancel()
	defer func() {
		if r.client != nil {
			r.client.Disconnect()
		}
		close(r.done)
	}()

	client := whatsmeow.NewClient(r.boot.device, nil)
	// whatsmeow's own retry has no cap, no jitter and no stop on a ban, so it stays off.
	client.EnableAutoReconnect = false
	r.client = client
	// The consent Start gated on, threaded to the call that touches WhatsApp.
	prepared, err := prepareLink(wa.Consent{Granted: consent}, client)
	if err != nil {
		f.failed(r, err)
		return
	}
	codes, err := prepared.Linker.QR(ctx)
	if err != nil {
		f.failed(r, err)
		return
	}
	f.publishState(ipc.LinkWaiting, "")
	for code := range codes {
		if code.Text != "" {
			size, rows, err := linkQR(code.Text)
			if err != nil {
				// The payload never goes into a log or an error: it is the secret that pairs the
				// account (proto/wire.md, "The link flow").
				f.failed(r, err)
				return
			}
			f.events.Publish(ipc.EventLinkQR{V: ipc.Version, Op: ipc.OpLinkQR, Size: size, Rows: rows})
		}
		if !code.Done() {
			continue
		}
		if ctx.Err() != nil {
			// A cancel stopped the flow: the client that asked for it gets link_cancelled and no
			// failure sentence, which would say WhatsApp stopped something the user stopped.
			f.finish(r, true)
			return
		}
		if code.Event == "success" {
			f.linked(r)
			return
		}
		// WhatsApp stopped the pairing (a ban, a logout, an expired code): one fixed sentence, and
		// the client offers a new QR.
		f.failed(r, fmt.Errorf("link event %s", code.Event))
		return
	}
	if ctx.Err() != nil {
		f.finish(r, true)
		return
	}
	f.failed(r, errors.New("the QR stream ended without a result"))
}

// linked hands the paired device to the daemon, which opens and starts the session it serves, and
// publishes link_state linked only once that session is the daemon's: the frames that follow are
// the session's own.
func (f *linkFlow) linked(r *linkRun) {
	// The flow's own client and handle go first: the daemon opens the store and the device again,
	// and two whatsmeow clients over one device would take the stream from each other.
	if r.client != nil {
		r.client.Disconnect()
	}
	_ = r.boot.db.Close()
	r.boot.db = nil
	if err := f.paired(); err != nil {
		// The device is paired but this daemon cannot serve it; status is the truth about the
		// device, and the flow reports its own outcome. Nothing of it is removed: the device is
		// WhatsApp's now.
		f.finish(r, false)
		f.publishState(ipc.LinkFailed, ipc.LinkFailureReason)
		return
	}
	f.finish(r, false)
	f.publishState(ipc.LinkLinked, "")
}

// failed publishes the one fixed sentence a failed flow carries. cause is logged by its type only:
// it can quote a server address, and the flow's failure is never a secret.
func (f *linkFlow) failed(r *linkRun, cause error) {
	log.Printf("the link flow stopped without linking (%T)", cause)
	f.finish(r, true)
	f.publishState(ipc.LinkFailed, ipc.LinkFailureReason)
}

// finish takes the flow out of the running slot, removing what it created when discard is set,
// before the state a client reacts to is published: a link_start that answers that state starts a
// new flow instead of hearing that one is already running.
func (f *linkFlow) finish(r *linkRun, discard bool) {
	if discard {
		r.cleanupErr = f.discard(r.boot)
		if r.cleanupErr != nil {
			log.Printf("the link flow could not remove everything it created: %v", r.cleanupErr)
		}
	}
	f.mu.Lock()
	if f.run == r {
		f.run = nil
	}
	f.mu.Unlock()
}

func (f *linkFlow) publishState(state, reason string) {
	f.events.Publish(ipc.EventLinkState{V: ipc.Version, Op: ipc.OpLinkState, State: state, Reason: reason})
}

// discard removes what a flow that did not pair created: the store it opened and, only when this
// flow made it, the data key. A device that was already linked is refused before any of this, so
// nothing of a previous device is ever removed here. It reports everything it could not remove, so
// a cancel is never answered as clean when something was left behind.
func (f *linkFlow) discard(boot *linkBootstrap) error {
	var left []error
	if boot.db != nil {
		_ = boot.db.Close()
	}
	for _, p := range []string{f.dbPath, f.dbPath + "-wal", f.dbPath + "-shm"} {
		if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
			// The base name only: the error's own text names the data dir's paths.
			left = append(left, fmt.Errorf("remove %s: %w", filepath.Base(p), err))
		}
	}
	if boot.createdKey {
		key, err := store.LoadDataKey(f.kr, false)
		if err != nil {
			left = append(left, fmt.Errorf("read back the data key to remove it: %w", err))
		} else {
			defer clear(key)
			if err := store.ForgetDataKey(f.kr, key); err != nil {
				left = append(left, fmt.Errorf("remove the data key: %w", err))
			}
		}
	}
	return errors.Join(left...)
}

// linkQR encodes payload as the matrix a link_qr frame carries: size modules per side and rows of
// "0"/"1", row 0 first, no quiet zone (the client draws the margin). It refuses a matrix larger
// than the wire allows instead of publishing one a client cannot draw, and its errors never carry
// the payload.
func linkQR(payload string) (int, []string, error) {
	code, err := qr.Encode(payload, qr.L)
	if err != nil {
		return 0, nil, fmt.Errorf("nunciod: cannot encode the QR code: %w", err)
	}
	if code.Size > ipc.MaxLinkQRSize {
		return 0, nil, fmt.Errorf("nunciod: the QR code is %d modules per side, over the %d the wire carries", code.Size, ipc.MaxLinkQRSize)
	}
	rows := make([]string, code.Size)
	for y := range rows {
		row := make([]byte, code.Size)
		for x := range row {
			row[x] = '0'
			if code.Black(x, y) {
				row[x] = '1'
			}
		}
		rows[y] = string(row)
	}
	return code.Size, rows, nil
}
