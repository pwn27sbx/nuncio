package main

import (
	"fmt"
	"io"
	"strings"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// retryCommand is the manual retry: `nuncio retry <client_id>` asks the daemon to send again, once,
// the one send it stored under that client_id when it ended not sent, under its first WhatsApp
// message id, so it can never become a second message. A send that is queued or already sent is
// only reported. It takes exactly one client_id, the one `nuncio send` printed, and never reads
// stdin; it prints what `nuncio send` prints.
func retryCommand(args []string, stdout, stderr io.Writer) int {
	if len(args) != 1 || strings.TrimSpace(args[0]) == "" {
		fmt.Fprintf(stderr, "nuncio retry: takes exactly one client_id, the one nuncio send printed: nuncio retry <client_id>\n\n%s\n", usage)
		return 2
	}
	clientID := args[0]
	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio: not sent:", err)
		return 1
	}
	defer func() { _ = c.Close() }()
	sent, err := c.Retry(clientID)
	return reportSend(sent, err, clientID, stdout, stderr)
}
