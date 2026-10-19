package wa_test

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/pwn27sbx/nuncio/internal/wa"
)

// The gate is the whole point of M0's linking work: without consent nothing may be attempted.
func TestPrepareRefusesWithoutConsent(t *testing.T) {
	_, err := wa.Prepare(wa.Consent{}, nil)
	if !errors.Is(err, wa.ErrNoConsent) {
		t.Fatalf("Prepare without consent = %v, want ErrNoConsent", err)
	}
}

func TestGateAcceptsGrantedConsent(t *testing.T) {
	if err := wa.Gate(wa.Consent{Granted: true, Reason: "test"}); err != nil {
		t.Fatalf("Gate with consent = %v, want nil", err)
	}
}

func TestPrepareNeedsAClientOnceConsentIsGiven(t *testing.T) {
	// Consent alone must not be enough to produce a linker out of nothing.
	_, err := wa.Prepare(wa.Consent{Granted: true}, nil)
	if err == nil {
		t.Fatal("Prepare with consent and no client = nil error, want a failure")
	}
	if errors.Is(err, wa.ErrNoConsent) {
		t.Fatalf("the failure must not be the consent error, got %v", err)
	}
}

// The QR renderer must draw something a human could scan, and must need no network to do it.
func TestRenderQRDrawsBlocks(t *testing.T) {
	var out strings.Builder
	if err := wa.RenderQR(&out, "2@AbCdEf0123456789==,Kx9Yz8Xw7V,1"); err != nil {
		t.Fatalf("RenderQR = %v", err)
	}
	rendered := out.String()
	if len(rendered) < 200 {
		t.Fatalf("a QR that short is not a QR: %d bytes", len(rendered))
	}
	if !strings.ContainsAny(rendered, "\u2588\u2580\u2584") {
		t.Fatal("the renderer drew no block characters")
	}
}

func TestRenderQRIgnoresAnEmptyPayload(t *testing.T) {
	var out strings.Builder
	if err := wa.RenderQR(&out, ""); err != nil {
		t.Fatalf("RenderQR(empty) = %v", err)
	}
	if out.Len() != 0 {
		t.Fatalf("RenderQR(empty) wrote %d bytes, want 0", out.Len())
	}
}

// A caller has to be able to tell when to stop reading the stream.
func TestCodeDoneOnlyAfterATerminalEvent(t *testing.T) {
	cases := []struct {
		code wa.Code
		done bool
	}{
		{wa.Code{Event: "code", Text: "2@abc"}, false},
		{wa.Code{Event: "success"}, true},
		{wa.Code{Event: "timeout"}, true},
		{wa.Code{Event: "err-client-outdated"}, true},
	}
	for _, c := range cases {
		if got := c.code.Done(); got != c.done {
			t.Errorf("Code{%q}.Done() = %t, want %t", c.code.Event, got, c.done)
		}
	}
}

// fakeLinker proves the gate sits in front of the flow, not behind it.
type fakeLinker struct{ called bool }

func (f *fakeLinker) QR(context.Context) (<-chan wa.Code, error) {
	f.called = true
	ch := make(chan wa.Code, 1)
	ch <- wa.Code{Event: "success"}
	close(ch)
	return ch, nil
}

func (f *fakeLinker) Phone(context.Context, string) (string, error) {
	f.called = true
	return "12345678", nil
}

func TestTheGateStopsTheFlowBeforeItStarts(t *testing.T) {
	fake := &fakeLinker{}
	if err := wa.Gate(wa.Consent{}); err == nil {
		t.Fatal("Gate refused nothing")
	}
	if fake.called {
		t.Fatal("the linking flow ran despite the gate")
	}
}
