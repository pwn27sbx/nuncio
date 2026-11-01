package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"time"

	"go.mau.fi/whatsmeow"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// connectLinked connects the client of an already linked device; tests replace it so that no
// test can reach WhatsApp.
var connectLinked = func(c *whatsmeow.Client) error { return c.Connect() }

// prepareLink wraps a client in the linking flows behind the consent gate; tests replace it with a
// fake Linker so that no test can reach WhatsApp.
var prepareLink = wa.Prepare

// watchLink subscribes to the client's pairing and connection events; tests replace it so that a
// fake linker can report a pairing without a WhatsApp connection.
var watchLink = watch

// phoneWait bounds how long `--phone` waits for the code to be entered. Tests shorten it.
var phoneWait = 5 * time.Minute

// linkCommand is the S1–S3 entry point. It is prepared in full — the gate, the device store, the
// whatsmeow client, both flows and the QR renderer — and it does nothing at all until --consent is
// passed. Even then the owner is expected to have said yes out loud first.
func linkCommand(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("nuncio link", flag.ContinueOnError)
	fs.SetOutput(stderr)
	qr := fs.Bool("qr", false, "link by scanning a QR code in the terminal (nunciod draws it)")
	phone := fs.String("phone", "", "link with an 8-digit pairing code for this E.164 number")
	consentFlag := fs.Bool("consent", false, "you have read the warning and accept the risk")
	plain := fs.Bool("plain-qr", false, "print only the QR code, frame by frame, for a script or a GUI that draws the matrix itself")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *qr == (*phone != "") {
		fmt.Fprintln(stderr, "nuncio link: pass exactly one of --qr or --phone <number>")
		return 2
	}
	if *plain && !*qr {
		fmt.Fprintln(stderr, "nuncio link: --plain-qr needs --qr")
		return 2
	}

	consent := wa.Consent{Granted: *consentFlag, Reason: "nuncio link on the command line"}

	// The gate is checked first — before the store is opened, before a client exists, before any
	// socket is dialled. Nothing above this line has a side effect.
	if err := wa.Gate(consent); err != nil {
		fmt.Fprintln(stderr, wa.LinkWarning)
		fmt.Fprintln(stderr, "\nnuncio: nothing was linked.")
		return 2
	}

	ctx := context.Background()
	if *qr {
		// The daemon is the linker: it owns the store, the key and the codes, and it keeps the
		// session it pairs. Ctrl-C cancels the flow, and the daemon removes what it created.
		ctx, stop := signal.NotifyContext(ctx, os.Interrupt)
		defer stop()
		return linkByQR(ctx, consent, *plain, stdout, stderr)
	}

	// The daemon owns the session of a linked device, and at start it migrates a plain database
	// an older build left, as this command does. Checked before the store is opened, so a link
	// never migrates, creates a key or connects while the daemon may be doing the same: a second
	// connection of the same device would replace the daemon's stream, and each would take it
	// back from the other. `--phone` still pairs from here, because no frame carries a pairing
	// code yet; `--qr` never reaches this point.
	if daemonAnswers() {
		fmt.Fprintln(stderr, "nuncio link: nunciod is running and owns this device's session; stop it first. Nothing was linked.")
		return 1
	}

	path, err := wa.DBPath()
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	// store.Open creates the data directory 0700 and the database 0600.
	db, err := store.Open(path)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	defer func() { _ = db.Close() }()

	// The device's keys are sealed under the data key in the keyring (ADR 0003). A plain database
	// from an older build is backed up and sealed first. A new device's data key is created only
	// when pairing saves it, so a link that never pairs leaves no key and no device behind.
	device, migration, err := wa.OpenDevice(ctx, db, path, localKeyring, true)
	if notice := migration.Notice(); notice != "" {
		// In plain mode stdout belongs to the code: a GUI draws every line as art.
		out := stdout
		if *plain {
			out = stderr
		}
		fmt.Fprintln(out, "nuncio:", notice)
	}
	switch {
	case errors.Is(err, store.ErrKeyringUnavailable):
		fmt.Fprintln(stderr, "nuncio link: the keyring (Secret Service) is locked or unavailable; unlock it and try again. Nothing was linked.")
		return 1
	case errors.Is(err, wa.ErrDeviceUnreadable):
		fmt.Fprintf(stderr, "nuncio link: %v. Nothing was linked.\n", wa.ErrDeviceUnreadable)
		return 1
	case err != nil:
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	client := whatsmeow.NewClient(device, nil)
	// whatsmeow's own retry has no cap, no jitter and no stop on a ban, so it stays off.
	client.EnableAutoReconnect = false
	events := watchLink(client)

	if device.ID != nil {
		if *plain {
			// A GUI draws stdout as the code: nothing else may go there.
			return 0
		}
		fmt.Fprintln(stdout, "nuncio: this machine already holds a linked device:", maskPhone(device.ID.User))
		if err := connectLinked(client); err != nil {
			fmt.Fprintln(stderr, "nuncio link:", err)
			return 1
		}
		defer client.Disconnect()
		select {
		case <-events.connected:
		case <-time.After(60 * time.Second):
			fmt.Fprintln(stderr, "nuncio: still not connected after 60 s")
			return 1
		}
		return linked(stdout, device.ID.User, false)
	}

	prepared, err := prepareLink(consent, client)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	defer client.Disconnect()

	code, err := prepared.Linker.Phone(ctx, *phone)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	fmt.Fprintf(stdout, "\nOn your phone: Settings → Linked devices → Link a device → Link with phone number instead, then enter:\n\n    %s\n\nWaiting up to %d minutes...\n", code, int(phoneWait.Minutes()))
	select {
	case id := <-events.paired:
		return linked(stdout, id, false)
	case <-time.After(phoneWait):
		fmt.Fprintln(stderr, "nuncio: the code was not entered in time; nothing was linked")
		return 1
	}
}

// linked reports a linked device and how the session continues, and ends the command. With
// daemonUp the daemon is already running and owns the session (`--qr` pairs it there); otherwise
// this command held the session and the daemon must be started for the user.
// nuncio link never sends a message: sending is one explicit action of its own (nuncio send or
// an app), never a side effect of linking.
func linked(stdout io.Writer, user string, daemonUp bool) int {
	if user != "" {
		fmt.Fprintln(stdout, "nuncio: linked as", maskPhone(user))
	} else {
		fmt.Fprintln(stdout, "nuncio: linked")
	}
	if daemonUp {
		fmt.Fprintln(stdout, "nuncio: nunciod owns the session from now on; open nuncio to see your chats.")
	} else {
		fmt.Fprintln(stdout, "nuncio: start the daemon, which owns the session from now on: systemctl --user start nunciod")
	}
	return 0
}

// linkEventTimeout bounds every read of the flow. The daemon publishes a new code before the one
// on screen expires (every 20 s or so), so a minute of quiet means the flow is gone.
const linkEventTimeout = time.Minute

// linkByQR asks the running daemon for the device-linking flow and draws each code it publishes
// until the phone answers. The daemon owns the store, the data key and the codes, so a link from
// here and one from the app leave exactly the same data behind; nothing here opens the database or
// the keyring. `plain` keeps stdout to the art alone: a script or a GUI draws the matrix itself and
// learns the outcome from the exit status.
func linkByQR(ctx context.Context, consent wa.Consent, plain bool, stdout, stderr io.Writer) int {
	socket, err := ipc.DefaultSocketPath()
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	requests, err := ipc.DialTimeout(socket, ipc.ClientName, linkEventTimeout)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link: nunciod is not running; open nuncio, or start nunciod, and try again. Nothing was linked.")
		return 1
	}
	defer func() { _ = requests.Close() }()
	events, err := ipc.DialTimeout(socket, ipc.ClientName, linkEventTimeout)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link: nunciod stopped answering; open nuncio and try again. Nothing was linked.")
		return 1
	}
	defer func() { _ = events.Close() }()

	// Subscribe before asking: the flow's first code can be published before link_started arrives.
	if err := events.Subscribe(); err != nil {
		fmt.Fprintln(stderr, "nuncio link: cannot follow the flow:", err)
		return 1
	}
	if _, err := requests.LinkStart(consent.Granted); err != nil {
		var refused *ipc.ProtocolError
		if errors.As(err, &refused) {
			fmt.Fprintf(stderr, "nuncio link: %s. Nothing was linked.\n", refused.Message)
			return 1
		}
		// The daemon may have started the flow even though its answer never arrived (a keyring
		// waiting on an unlock is the slow case). Stop it before leaving: a flow left running
		// refuses the next `link_start` as busy, and this command showed no code to scan.
		if _, cancelErr := requests.LinkCancel(); cancelErr == nil {
			fmt.Fprintln(stderr, "nuncio link: the daemon did not answer and the flow was stopped; nothing was linked.")
			return 1
		}
		fmt.Fprintln(stderr, "nuncio link: the daemon did not answer; nothing was linked:", err)
		return 1
	}

	type event struct {
		op    string
		frame []byte
		err   error
	}
	incoming := make(chan event, 8)
	go func() {
		for {
			op, frame, err := events.NextEvent()
			select {
			case incoming <- event{op: op, frame: frame, err: err}:
			case <-ctx.Done():
				return
			}
			if err != nil {
				return
			}
		}
	}()

	if !plain {
		fmt.Fprintln(stdout, "nuncio: scan this code with WhatsApp → Settings → Linked devices → Link a device; it refreshes until your phone answers.")
	}
	for {
		select {
		case <-ctx.Done():
			// Interrupted. The daemon removes the store and the data key this flow created, and
			// answers only once it has, so nothing is left behind.
			if _, err := requests.LinkCancel(); err != nil {
				fmt.Fprintln(stderr, "nuncio link: cannot stop the flow, which removes what it made when it ends:", err)
				return 1
			}
			fmt.Fprintln(stderr, "nuncio: linking cancelled; nothing was linked")
			return 1
		case ev := <-incoming:
			if ev.err != nil {
				fmt.Fprintln(stderr, "nuncio link: lost the flow:", ev.err)
				return 1
			}
			switch ev.op {
			case ipc.OpLinkQR:
				var qr ipc.EventLinkQR
				if err := json.Unmarshal(ev.frame, &qr); err != nil {
					fmt.Fprintln(stderr, "nuncio link: a malformed link_qr frame:", err)
					return 1
				}
				if err := wa.RenderMatrix(stdout, qr.Size, qr.Rows); err != nil {
					fmt.Fprintln(stderr, "nuncio link: cannot draw the QR code:", err)
					return 1
				}
				if plain {
					// One frame per code: a reader knows a new code starts after a blank line.
					fmt.Fprintln(stdout)
				}
			case ipc.OpLinkState:
				var state ipc.EventLinkState
				if err := json.Unmarshal(ev.frame, &state); err != nil {
					fmt.Fprintln(stderr, "nuncio link: a malformed link_state frame:", err)
					return 1
				}
				switch state.State {
				case ipc.LinkLinked:
					if plain {
						return 0
					}
					return linked(stdout, "", true)
				case ipc.LinkFailed:
					reason := state.Reason
					if reason == "" {
						reason = ipc.LinkFailureReason
					}
					fmt.Fprintf(stderr, "nuncio: %s; nothing was linked\n", reason)
					return 1
				}
			}
		}
	}
}

type linkEvents struct {
	paired    chan string
	connected chan struct{}
}

// watch subscribes before Connect, so no pairing or connection event can be missed. The event
// switch itself is wa.Events, shared with the daemon's session.
func watch(client *whatsmeow.Client) linkEvents {
	ev := linkEvents{make(chan string, 1), make(chan struct{}, 1)}
	client.AddEventHandler(wa.Events{
		Paired: func(user string) {
			select {
			case ev.paired <- user:
			default:
			}
		},
		Connected: func() {
			select {
			case ev.connected <- struct{}{}:
			default:
			}
		},
	}.Handle)
	return ev
}

// daemonAnswers reports whether a daemon completes the handshake on core.sock. It dials the
// socket only: it never asks the session bus, so it never activates a daemon.
func daemonAnswers() bool {
	path, err := ipc.DefaultSocketPath()
	if err != nil {
		return false
	}
	c, err := ipc.DialTimeout(path, ipc.ClientName, 2*time.Second)
	if err != nil {
		return false
	}
	_ = c.Close()
	return true
}

// maskPhone keeps only the last four digits of a phone number, so nothing this command prints can
// put a full number into a log. A number of four digits or fewer is hidden entirely.
func maskPhone(number string) string { return ipc.MaskPhone(number) }
