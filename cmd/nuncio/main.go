// Command nuncio is the CLI: `nuncio` alone connects to the daemon, does the NDJSON handshake and
// prints the session status; `nuncio link` is the linking entry point, and it refuses to do
// anything at all unless --consent is passed.
//
// When the socket is not there this asks the session bus for the daemon's name, which activates
// the daemon; it never starts a daemon itself.
package main

import (
	"fmt"
	"io"
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
	os.Exit(cli(os.Args[1:], os.Stdout, os.Stderr))
}

// cli runs one command line and returns the process exit code: 0 on success, 1 when the command
// failed, 2 on a usage error.
func cli(args []string, stdout, stderr io.Writer) int {
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		switch args[0] {
		case "link":
			return linkCommand(args[1:], stdout, stderr)
		case "status":
			// fall through to the default below
		case "help":
			fmt.Fprintln(stdout, usage)
			return 0
		default:
			fmt.Fprintf(stderr, "nuncio: unknown command %q\n\n%s\n", args[0], usage)
			return 2
		}
	}
	if len(args) > 0 && (args[0] == "-h" || args[0] == "--help") {
		fmt.Fprintln(stdout, usage)
		return 0
	}

	if err := run(stdout); err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	return 0
}

func run(stdout io.Writer) error {
	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		return err
	}
	defer func() { _ = c.Close() }()

	st, err := c.Status()
	if err != nil {
		return err
	}
	fmt.Fprintf(stdout, "linked=%t session=%s unread=%d\n", st.Linked, st.Session, st.Unread)
	return nil
}
