// Command nuncio is the CLI: `nuncio` alone (or `nuncio status`) connects to the daemon, does the
// NDJSON handshake and prints the session status; `chats` lists the chats, `send` sends one message
// to one chat, `focus` asks a running UI to open a chat, and `link` is the linking entry point,
// which refuses to do anything at all unless --consent is passed. `logout` and `purge` delete
// everything nuncio keeps for the device and refuse to do anything unless --confirm is passed.
//
// There is no automation surface: `send` takes exactly one recipient and one message per
// invocation and never reads stdin.
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
  nuncio status                 print the daemon's session status (also: nuncio alone)
  nuncio chats                  list the chats: name, unread count, pinned/archived, last message time
  nuncio send <jid> <message>   send one message to one chat; quote the message, it is one argument
  nuncio focus <jid>            ask the running UI to open that chat
  nuncio link --qr              link by scanning a QR code (needs --consent)
  nuncio link --phone N         link with an 8-digit pairing code (needs --consent)
  nuncio logout --confirm       unlink this device from WhatsApp, delete all local data and keys, stop nunciod
  nuncio purge --confirm        the same without contacting WhatsApp, for a device already removed on the phone
  nuncio --help

A <jid> is a WhatsApp chat: 15551230001@s.whatsapp.net (a person) or 120363000000000000@g.us (a group).
send prints "sent", "queued", "not sent: <reason>" or "not confirmed: …; it may have been delivered"
(no acknowledgement in time), and exits non-zero unless it was sent or queued.
Nothing links a device without --consent. logout and purge without --confirm only list what they
would delete. logout needs a running nunciod (it unlinks through the daemon's session); purge works
with or without one.`

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
		case "chats":
			return chatsCommand(args[1:], stdout, stderr)
		case "send":
			return sendCommand(args[1:], stdout, stderr)
		case "focus":
			return focusCommand(args[1:], stdout, stderr)
		case "logout":
			return logoutCommand(args[1:], stdout, stderr)
		case "purge":
			return purgeCommand(args[1:], stdout, stderr)
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
