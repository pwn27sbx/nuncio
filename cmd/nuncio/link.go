package main

import (
	"context"
	"database/sql"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"go.mau.fi/whatsmeow"
	_ "modernc.org/sqlite"

	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// linkCommand is the S1–S3 entry point. It is prepared in full — the gate, the device store, the
// whatsmeow client, both flows and the QR renderer — and it does nothing at all until --consent is
// passed. Even then the owner is expected to have said yes out loud first.
func linkCommand(args []string) int {
	fs := flag.NewFlagSet("nuncio link", flag.ContinueOnError)
	qr := fs.Bool("qr", false, "link by scanning a QR code in the terminal")
	phone := fs.String("phone", "", "link with an 8-digit pairing code for this E.164 number")
	consentFlag := fs.Bool("consent", false, "you have read the warning and accept the risk")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *qr == (*phone != "") {
		fmt.Fprintln(os.Stderr, "nuncio link: pass exactly one of --qr or --phone <number>")
		return 2
	}

	consent := wa.Consent{Granted: *consentFlag, Reason: "nuncio link on the command line"}

	// The gate is checked first — before the store is opened, before a client exists, before any
	// socket is dialled. Nothing above this line has a side effect.
	if err := wa.Gate(consent); err != nil {
		fmt.Fprintln(os.Stderr, wa.LinkWarning)
		fmt.Fprintln(os.Stderr, "\nnuncio: nothing was linked.")
		return 2
	}

	ctx := context.Background()

	dataDir, err := nuncioDataDir()
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}
	db, err := openStoreDB(filepath.Join(dataDir, "nuncio.db"))
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}
	defer func() { _ = db.Close() }()

	devices, err := store.NewDeviceStore(db, store.SecretService{})
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}

	device, err := devices.LoadDevice(ctx)
	switch {
	case errors.Is(err, store.ErrNoDevice):
		// Unpaired: generate keys for a device that does not exist anywhere until a link succeeds.
		device = devices.NewDevice()
		fmt.Println("nuncio: no device linked yet; this run would create one.")
	case err != nil:
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	default:
		fmt.Println("nuncio: this machine already holds a linked device.")
	}

	prepared, err := wa.Prepare(consent, whatsmeow.NewClient(device, nil))
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}

	if *qr {
		return runQRLink(ctx, prepared.Linker)
	}
	return runPhoneLink(ctx, prepared.Linker, *phone)
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

func runPhoneLink(ctx context.Context, linker wa.Linker, number string) int {
	code, err := linker.Phone(ctx, number)
	if err != nil {
		fmt.Fprintln(os.Stderr, "nuncio link:", err)
		return 1
	}
	fmt.Printf("\nOn your phone: Settings → Linked devices → Link with phone number, then enter: %s\n", code)
	return 0
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

func openStoreDB(path string) (*sql.DB, error) {
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("cannot open %s: %w", path, err)
	}
	if _, err := db.Exec(`PRAGMA journal_mode=WAL`); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("cannot prepare %s: %w", path, err)
	}
	return db, nil
}
