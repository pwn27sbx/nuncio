// Command nuncio is the CLI: `nuncio` alone connects to the daemon, does the NDJSON handshake and
// prints the session status; `nuncio link` is the linking entry point, and it refuses to do
// anything at all unless --consent is passed.
//
// When the socket is not there this asks the session bus for the daemon's name, which activates
// the daemon; it never starts a daemon itself.
package main

import (
	"fmt"
	"os"
	"strings"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

const usage = `usage:
  nuncio                 print the daemon's session status
  nuncio link --qr       link by scanning a QR code (needs --consent)
  nuncio link --phone N  link with an 8-digit pairing code (needs --consent)
  nuncio --help

Nothing links a device without --consent.`

func main() {
	if len(os.Args) > 1 && !strings.HasPrefix(os.Args[1], "-") {
		switch os.Args[1] {
		case "link":
			os.Exit(linkCommand(os.Args[2:]))
		case "status":
			// fall through to the default below
		case "help":
			fmt.Println(usage)
			return
		default:
			fmt.Fprintf(os.Stderr, "nuncio: unknown command %q\n\n%s\n", os.Args[1], usage)
			os.Exit(2)
		}
	}
	if len(os.Args) > 1 && (os.Args[1] == "-h" || os.Args[1] == "--help") {
		fmt.Println(usage)
		return
	}

	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "nuncio:", err)
		os.Exit(1)
	}
}

func run() error {
	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		return err
	}
	defer func() { _ = c.Close() }()

	st, err := c.Status()
	if err != nil {
		return err
	}
	fmt.Printf("linked=%t session=%s unread=%d\n", st.Linked, st.Session, st.Unread)
	return nil
}
