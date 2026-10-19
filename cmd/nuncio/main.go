// Command nuncio is the thin CLI stub: it connects to the daemon, does the
// NDJSON handshake and prints the session status. When the socket is not there
// it asks the session bus for the daemon's name, which activates the daemon;
// it never starts a daemon itself.
package main

import (
	"fmt"
	"os"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

func main() {
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
