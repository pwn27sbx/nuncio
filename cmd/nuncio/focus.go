package main

import (
	"fmt"
	"io"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// focusCommand asks the running UIs to open one chat and prints how many got the request. With no
// UI running nothing happens, and that is not an error.
func focusCommand(args []string, stdout, stderr io.Writer) int {
	if len(args) != 1 || !ipc.ValidJID(args[0]) {
		fmt.Fprintf(stderr, "nuncio focus: takes one WhatsApp chat jid: nuncio focus <jid>\n\n%s\n", usage)
		return 2
	}
	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	defer func() { _ = c.Close() }()
	focused, err := c.Focus(args[0])
	if err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	switch focused.Delivered {
	case 0:
		fmt.Fprintln(stdout, "focus delivered to 0 UIs: no UI is running")
	case 1:
		fmt.Fprintln(stdout, "focus delivered to 1 UI")
	default:
		fmt.Fprintf(stdout, "focus delivered to %d UIs\n", focused.Delivered)
	}
	return 0
}
