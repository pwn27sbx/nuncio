package wa

import (
	"context"
	"errors"
	"slices"
	"testing"

	"go.mau.fi/whatsmeow"
)

// The QR channel is asked for before connecting: whatsmeow refuses the other order
// ("GetQRChannel must be called before connecting") and refuses it immediately, so a flow that
// connected first published no code at all. T11 found this against the real server; every test of
// the daemon used a fake Linker, and every test of the CLI a fake daemon.
func TestStartQRAsksForTheChannelBeforeConnecting(t *testing.T) {
	var calls []string
	get := func(context.Context) (<-chan whatsmeow.QRChannelItem, error) {
		calls = append(calls, "channel")
		ch := make(chan whatsmeow.QRChannelItem)
		close(ch)
		return ch, nil
	}
	connect := func() error {
		calls = append(calls, "connect")
		return nil
	}

	if _, err := startQR(context.Background(), get, connect); err != nil {
		t.Fatalf("startQR: %v", err)
	}
	if want := []string{"channel", "connect"}; !slices.Equal(calls, want) {
		t.Errorf("calls %v, want %v: the channel comes first", calls, want)
	}
}

// A channel whatsmeow refuses is the answer: nothing connects behind its back.
func TestStartQRDoesNotConnectWhenTheChannelIsRefused(t *testing.T) {
	connected := false
	get := func(context.Context) (<-chan whatsmeow.QRChannelItem, error) {
		return nil, whatsmeow.ErrQRAlreadyConnected
	}
	connect := func() error {
		connected = true
		return nil
	}

	_, err := startQR(context.Background(), get, connect)
	if !errors.Is(err, whatsmeow.ErrQRAlreadyConnected) {
		t.Errorf("startQR = %v, want the channel's refusal", err)
	}
	if connected {
		t.Error("a refused channel still connected")
	}
}
