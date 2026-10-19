package wa

import (
	"io"

	"github.com/mdp/qrterminal/v3"
)

// RenderQR draws `payload` as a QR code in the terminal.
//
// This is the one piece of the linking path that is pure and offline, so it is the one piece M0
// can actually prove: it renders a payload shaped like the one WhatsApp sends, without ever
// obtaining a real one.
func RenderQR(w io.Writer, payload string) error {
	if payload == "" {
		return nil
	}
	qrterminal.GenerateHalfBlock(payload, qrterminal.L, w)
	return nil
}
