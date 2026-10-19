package main

import (
	"crypto/rand"
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// sendCommand sends exactly one message to exactly one chat: `nuncio send <jid> <message>`, with
// the message quoted as one argument. Any other argument count is refused, so an unquoted message
// cannot smuggle in a second recipient, and there is no list, batch or stdin mode. It prints
// "sent" or "queued" and exits 0, or prints "not sent: <reason>" (or "not confirmed: …" when the
// outcome is unknown) and exits 1. Unless it was sent it also prints the send's client_id, which
// `nuncio retry` takes.
func sendCommand(args []string, stdout, stderr io.Writer) int {
	if len(args) != 2 {
		fmt.Fprintf(stderr, "nuncio send: takes exactly one recipient and one message: nuncio send <jid> \"<message>\"\n\n%s\n", usage)
		return 2
	}
	jid, text := args[0], args[1]
	if !ipc.ValidJID(jid) {
		fmt.Fprintln(stderr, "nuncio send: the recipient must be one WhatsApp chat jid, like 15551230001@s.whatsapp.net")
		return 2
	}
	if strings.TrimSpace(text) == "" {
		fmt.Fprintln(stderr, "nuncio send: the message is empty; nothing was sent")
		return 2
	}

	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio: not sent:", err)
		return 1
	}
	defer func() { _ = c.Close() }()
	clientID := rand.Text()
	sent, err := c.Send(jid, text, clientID)
	return reportSend(sent, err, clientID, stdout, stderr)
}

// reportSend prints the outcome of a send or a retry of clientID and returns the exit code: 0 when
// it was sent or queued, 1 otherwise. Every outcome but sent names the client_id, and every
// failure names the `nuncio retry` command that sends it again under the same WhatsApp id.
func reportSend(sent ipc.Sent, err error, clientID string, stdout, stderr io.Writer) int {
	retryHint := func(prefix string) {
		fmt.Fprintf(stderr, "nuncio: %s: nuncio retry %s\n", prefix, clientID)
	}
	var notSent *ipc.NotSentError
	var perr *ipc.ProtocolError
	switch {
	case errors.As(err, &notSent):
		fmt.Fprintln(stderr, "nuncio: not sent:", notSent.Reason)
		retryHint("to try again")
		return 1
	case errors.As(err, &perr) && perr.Code == ipc.CodeNotLinked:
		fmt.Fprintf(stderr, "nuncio: not sent: %s; link one with `nuncio link --qr --consent`\n", perr.Message)
		return 1
	case errors.As(err, &perr) && perr.Code == ipc.CodeInternal && perr.Message == ipc.ErrNotConfirmed.Error():
		// It may have been delivered: never say "not sent", which would invite a duplicate. A retry
		// reuses the same WhatsApp id, so it cannot become a second message.
		fmt.Fprintln(stderr, "nuncio:", perr.Message)
		retryHint("if it did not arrive, try again with the same message id")
		return 1
	case err != nil:
		// The daemon may or may not have sent it; say so rather than guess.
		fmt.Fprintln(stderr, "nuncio: not confirmed as sent:", err)
		retryHint("to see what happened to it, or try again")
		return 1
	case sent.State == ipc.SendQueued:
		fmt.Fprintf(stdout, "queued: it will be sent once the session reconnects (client_id %s)\n", clientID)
		return 0
	}
	fmt.Fprintln(stdout, "sent")
	return 0
}
