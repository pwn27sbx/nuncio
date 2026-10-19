// Package wa is the whatsmeow integration.
//
// M0 contains the linking path and nothing else, and every entry into it is gated behind an
// explicit, per-run Consent. There is deliberately no code path that links a device as a side
// effect of starting the daemon or the clients.
package wa

import (
	"context"
	"errors"
	"fmt"
	"time"

	"go.mau.fi/whatsmeow"
)

// Consent is the owner's explicit permission to link a device, for one run only.
type Consent struct {
	Granted bool
	// Reason records who authorised the link and why, so a later reader can tell an agreed link
	// from an accidental one.
	Reason string
}

// ErrNoConsent is returned by every path that would link a device without Consent.
var ErrNoConsent = errors.New("nuncio: linking a WhatsApp device needs the owner's explicit consent")

// LinkWarning is shown before any linking is attempted. It is deliberately blunt: the ban risk is
// real, documented and borne by the person typing the command, and a client that soft-pedals it is
// not being honest with its own user.
const LinkWarning = `Linking WhatsApp with an unofficial client:

  * it is against WhatsApp's Terms of Service;
  * accounts have been warned and banned for it, including accounts that only ever
    replied to messages (tulir/whatsmeow#810, open 2025-05-09, closed as not planned);
  * voice and video calls are not supported and will not be;
  * use a spare test number, never the number that carries your 2FA.

Nothing is linked until --consent is passed.`

// Code is one step of a QR link.
type Code struct {
	// Text is the payload to render as a QR code. Empty on a terminal event.
	Text string
	// Event is "code", "success", "timeout", or an "err-*" reason.
	Event string
	// Timeout is how long Text stays valid, when the event carries one.
	Timeout time.Duration
}

// Done reports whether this code ends the link attempt.
func (c Code) Done() bool {
	return c.Event != "code" && c.Text == ""
}

// Linker is WhatsApp's two linking flows.
//
// It is an interface so the consent gate is testable with a fake: the real implementation opens a
// connection to WhatsApp the moment it is used, which tests must never do.
type Linker interface {
	// QR streams codes until one reports Done.
	QR(ctx context.Context) (<-chan Code, error)
	// Phone requests an 8-digit pairing code for an E.164 number.
	Phone(ctx context.Context, number string) (string, error)
}

// Gate is the single place consent is checked. Every linking path goes through it.
func Gate(consent Consent) error {
	if !consent.Granted {
		return ErrNoConsent
	}
	return nil
}

// Prepared is what a granted Consent yields. It is only ever built through Prepare.
type Prepared struct {
	Linker  Linker
	Consent Consent
}

// Prepare enforces the gate and, only then, wraps a client in a Linker.
//
// client may be nil when consent is not granted: the gate returns before it is touched, which is
// what lets the refusal be tested without a whatsmeow client at all.
func Prepare(consent Consent, client *whatsmeow.Client) (Prepared, error) {
	if err := Gate(consent); err != nil {
		return Prepared{}, err
	}
	if client == nil {
		return Prepared{}, errors.New("wa: no client to link with")
	}
	return Prepared{Linker: &whatsmeowLinker{client: client}, Consent: consent}, nil
}

type whatsmeowLinker struct {
	client *whatsmeow.Client
}

func (l *whatsmeowLinker) QR(ctx context.Context) (<-chan Code, error) {
	if err := l.client.Connect(); err != nil {
		return nil, fmt.Errorf("wa: connecting to link: %w", err)
	}
	raw, err := l.client.GetQRChannel(ctx)
	if err != nil {
		return nil, fmt.Errorf("wa: asking for a QR channel: %w", err)
	}
	out := make(chan Code)
	go func() {
		defer close(out)
		for item := range raw {
			code := Code{Event: item.Event, Text: item.Code, Timeout: item.Timeout}
			select {
			case out <- code:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out, nil
}

func (l *whatsmeowLinker) Phone(ctx context.Context, number string) (string, error) {
	// whatsmeow wants the login websocket fully up before PairPhone: subscribe to the QR channel
	// before connecting and wait for its first item (the QR itself is ignored in this flow).
	qr, err := l.client.GetQRChannel(ctx)
	if err != nil {
		return "", fmt.Errorf("wa: preparing to link: %w", err)
	}
	if err := l.client.Connect(); err != nil {
		return "", fmt.Errorf("wa: connecting to link: %w", err)
	}
	select {
	case <-qr:
	case <-time.After(30 * time.Second):
		return "", errors.New("wa: the login connection did not come up in 30 s")
	}
	go func() {
		for range qr { // keep draining so whatsmeow's QR rotation never blocks
		}
	}()
	// The server validates the display name: it must read "Browser (OS)" with a common browser/OS.
	code, err := l.client.PairPhone(ctx, number, true, whatsmeow.PairClientChrome, "Chrome (Linux)")
	if err != nil {
		return "", fmt.Errorf("wa: requesting a pairing code: %w", err)
	}
	return code, nil
}
