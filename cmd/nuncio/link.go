package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/store/sqlstore"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// linkCommand is the S1–S3 entry point. It is prepared in full — the gate, the device store, the
// whatsmeow client, both flows and the QR renderer — and it does nothing at all until --consent is
// passed. Even then the owner is expected to have said yes out loud first.
func linkCommand(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("nuncio link", flag.ContinueOnError)
	fs.SetOutput(stderr)
	qr := fs.Bool("qr", false, "link by scanning a QR code in the terminal")
	phone := fs.String("phone", "", "link with an 8-digit pairing code for this E.164 number")
	consentFlag := fs.Bool("consent", false, "you have read the warning and accept the risk")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *qr == (*phone != "") {
		fmt.Fprintln(stderr, "nuncio link: pass exactly one of --qr or --phone <number>")
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

	dataDir, err := nuncioDataDir()
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	db, err := store.Open(filepath.Join(dataDir, "nuncio.db"))
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	defer func() { _ = db.Close() }()

	// M0 spike: whatsmeow's own sqlstore keeps the full Signal state (sessions, pre-keys, sender
	// keys, app state) that a linked device needs. The keyring-backed store proved in S4 replaces it
	// in M1; until then the keys live in nuncio.db inside the 0700 data directory.
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		fmt.Fprintln(stderr, "nuncio link: cannot prepare the device store:", err)
		return 1
	}
	device, err := container.GetFirstDevice(ctx)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	client := whatsmeow.NewClient(device, nil)
	events := watch(client)

	if device.ID != nil {
		fmt.Fprintln(stdout, "nuncio: this machine already holds a linked device:", device.ID.User)
		if err := client.Connect(); err != nil {
			fmt.Fprintln(stderr, "nuncio link:", err)
			return 1
		}
		defer client.Disconnect()
		return listen(ctx, client, events)
	}

	prepared, err := wa.Prepare(consent, client)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	defer client.Disconnect()

	if *qr {
		if code := runQRLink(ctx, prepared.Linker); code != 0 {
			return code
		}
	} else {
		code, err := prepared.Linker.Phone(ctx, *phone)
		if err != nil {
			fmt.Fprintln(stderr, "nuncio link:", err)
			return 1
		}
		fmt.Fprintf(stdout, "\nOn your phone: Settings → Linked devices → Link a device → Link with phone number instead, then enter:\n\n    %s\n\nWaiting up to 5 minutes...\n", code)
		select {
		case id := <-events.paired:
			fmt.Fprintln(stdout, "nuncio: linked as", id)
		case <-time.After(5 * time.Minute):
			fmt.Fprintln(stderr, "nuncio: the code was not entered in time; nothing was linked")
			return 1
		}
	}
	return listen(ctx, client, events)
}

type linkEvents struct {
	paired    chan string
	connected chan struct{}
	messages  chan *waevents.Message
}

// watch subscribes before Connect, so no pairing or message event can be missed.
func watch(client *whatsmeow.Client) linkEvents {
	ev := linkEvents{make(chan string, 1), make(chan struct{}, 1), make(chan *waevents.Message, 8)}
	client.AddEventHandler(func(e interface{}) {
		switch v := e.(type) {
		case *waevents.PairSuccess:
			select {
			case ev.paired <- v.ID.User:
			default:
			}
		case *waevents.Connected:
			select {
			case ev.connected <- struct{}{}:
			default:
			}
		case *waevents.Message:
			if !v.Info.IsFromMe {
				select {
				case ev.messages <- v:
				default:
				}
			}
		}
	})
	return ev
}

// listen is the S1 check: wait for the session to come up, then for one incoming message, store it
// (whatsmeow already persisted the session state) and send one short reply. Nothing is sent
// unless a message arrives first.
func listen(ctx context.Context, client *whatsmeow.Client, ev linkEvents) int {
	select {
	case <-ev.connected:
		fmt.Println("nuncio: connected; send one message to this number from another phone (waiting 10 minutes)")
	case <-time.After(60 * time.Second):
		fmt.Fprintln(os.Stderr, "nuncio: still not connected after 60 s")
		return 1
	}
	select {
	case m := <-ev.messages:
		text := m.Message.GetConversation()
		if text == "" {
			text = m.Message.GetExtendedTextMessage().GetText()
		}
		fmt.Printf("nuncio: received from %s: %q\n", m.Info.Sender.User, text)
		reply := &waE2E.Message{Conversation: proto.String("nuncio: mensaje recibido ✓")}
		if _, err := client.SendMessage(ctx, m.Info.Chat, reply); err != nil {
			fmt.Fprintln(os.Stderr, "nuncio: reply failed:", err)
			return 1
		}
		fmt.Println("nuncio: reply sent")
		time.Sleep(3 * time.Second)
		return 0
	case <-time.After(10 * time.Minute):
		fmt.Fprintln(os.Stderr, "nuncio: no message arrived in 10 minutes")
		return 1
	}
}

func runQRLink(ctx context.Context, linker wa.Linker) int {
	codes, err := linker.QR(ctx)
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}
	for code := range codes {
		if code.Text != "" {
			fmt.Printf("\nScan with WhatsApp → Settings → Linked devices (valid %s):\n\n",
				code.Timeout.Round(time.Second))
			if err := wa.RenderQR(os.Stdout, code.Text); err != nil {
				fmt.Fprintln(os.Stderr, "nuncio link: cannot draw the QR code:", err)
				return 1
			}
		}
		if code.Done() {
			fmt.Println("nuncio: link event:", code.Event)
			if code.Event == "success" {
				return 0
			}
			return 1
		}
	}
	fmt.Fprintln(os.Stderr, "nuncio: the QR stream ended without a result")
	return 1
}

// nuncioDataDir is `$XDG_DATA_HOME/nuncio`, created 0700 so the store never sits in a
// world-readable directory.
func nuncioDataDir() (string, error) {
	base := os.Getenv("XDG_DATA_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("cannot find a data directory: %w", err)
		}
		base = filepath.Join(home, ".local", "share")
	}
	dir := filepath.Join(base, "nuncio")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", fmt.Errorf("cannot create %s: %w", dir, err)
	}
	return dir, nil
}
