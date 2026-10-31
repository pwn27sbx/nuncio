package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
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

// linkCommand is the S1–S3 entry point. It is prepared in full — the gate, the device store, the
// whatsmeow client, both flows and the QR renderer — and it does nothing at all until --consent is
// passed. Even then the owner is expected to have said yes out loud first.
func linkCommand(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("nuncio link", flag.ContinueOnError)
	fs.SetOutput(stderr)
	qr := fs.Bool("qr", false, "link by scanning a QR code in the terminal")
	phone := fs.String("phone", "", "link with an 8-digit pairing code for this E.164 number")
	consentFlag := fs.Bool("consent", false, "you have read the warning and accept the risk")
	plain := fs.Bool("plain-qr", false, "print only the QR code, frame by frame, for a GUI to draw")
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

	// The daemon owns the session of a linked device, and at start it migrates a plain database
	// an older build left, as this command does. Checked before the store is opened, so a link
	// never migrates, creates a key or connects while the daemon may be doing the same: a second
	// connection of the same device would replace the daemon's stream, and each would take it
	// back from the other.
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
		fmt.Fprintln(stdout, "nuncio:", notice)
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
	events := watch(client)

	if device.ID != nil {
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
		return linked(stdout, device.ID.User)
	}

	prepared, err := prepareLink(consent, client)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	defer client.Disconnect()

	if *qr {
		if code := runQRLink(ctx, prepared.Linker, stdout, stderr, *plain); code != 0 {
			return code
		}
		user := ""
		if client.Store.ID != nil {
			user = client.Store.ID.User
		}
		if *plain {
			return 0
		}
		return linked(stdout, user)
	}
	code, err := prepared.Linker.Phone(ctx, *phone)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	fmt.Fprintf(stdout, "\nOn your phone: Settings → Linked devices → Link a device → Link with phone number instead, then enter:\n\n    %s\n\nWaiting up to 5 minutes...\n", code)
	select {
	case id := <-events.paired:
		return linked(stdout, id)
	case <-time.After(5 * time.Minute):
		fmt.Fprintln(stderr, "nuncio: the code was not entered in time; nothing was linked")
		return 1
	}
}

// linked reports a linked device and how to hand its session to the daemon, and ends the command.
// nuncio link never sends a message: sending is one explicit action of its own (nuncio send or
// an app), never a side effect of linking.
func linked(stdout io.Writer, user string) int {
	if user != "" {
		fmt.Fprintln(stdout, "nuncio: linked as", maskPhone(user))
	} else {
		fmt.Fprintln(stdout, "nuncio: linked")
	}
	fmt.Fprintln(stdout, "nuncio: start the daemon, which owns the session from now on: systemctl --user start nunciod")
	return 0
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

// runQRLink draws each QR WhatsApp offers until the phone says yes. With `plain`, it writes only
// the code's own art and nothing else: a GUI (nuncio-desktop's link screen) draws exactly these
// lines in a monospace label and learns the outcome from the exit status.
func runQRLink(ctx context.Context, linker wa.Linker, stdout, stderr io.Writer, plain bool) int {
	codes, err := linker.QR(ctx)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	for code := range codes {
		if code.Text != "" {
			if !plain {
				fmt.Fprintf(stdout, "\nScan with WhatsApp → Settings → Linked devices (valid %s):\n\n",
					code.Timeout.Round(time.Second))
			}
			if err := wa.RenderQR(stdout, code.Text); err != nil {
				fmt.Fprintln(stderr, "nuncio link: cannot draw the QR code:", err)
				return 1
			}
			if plain {
				// One frame per offer: the reader knows a new code starts after a blank line.
				fmt.Fprintln(stdout)
			}
		}
		if code.Done() {
			if code.Event == "success" {
				return 0
			}
			fmt.Fprintf(stderr, "nuncio: link event %s; nothing was linked\n", code.Event)
			return 1
		}
	}
	fmt.Fprintln(stderr, "nuncio: the QR stream ended without a result")
	return 1
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
