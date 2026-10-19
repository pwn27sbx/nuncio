package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
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

	device, err := wa.OpenDevice(ctx, db)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio link:", err)
		return 1
	}
	client := whatsmeow.NewClient(device, nil)
	// whatsmeow's own retry has no cap, no jitter and no stop on a ban, so it stays off.
	client.EnableAutoReconnect = false
	events := watch(client)

	if device.ID != nil {
		fmt.Fprintln(stdout, "nuncio: this machine already holds a linked device:", maskPhone(device.ID.User))
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
			fmt.Fprintln(stdout, "nuncio: linked as", maskPhone(id))
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

// watch subscribes before Connect, so no pairing or message event can be missed. The event switch
// itself is wa.Events, shared with the daemon's session.
func watch(client *whatsmeow.Client) linkEvents {
	ev := linkEvents{make(chan string, 1), make(chan struct{}, 1), make(chan *waevents.Message, 8)}
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
		Message: func(m *waevents.Message) {
			if !m.Info.IsFromMe {
				select {
				case ev.messages <- m:
				default:
				}
			}
		},
	}.Handle)
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
		fmt.Printf("nuncio: received a message (%d bytes) from %s\n", len(text), maskPhone(m.Info.Sender.User))
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

// maskPhone keeps only the last four digits of a phone number, so nothing this command prints can
// put a full number into a log. A number of four digits or fewer is hidden entirely.
func maskPhone(number string) string {
	if len(number) <= 4 {
		return "…"
	}
	return "…" + number[len(number)-4:]
}
