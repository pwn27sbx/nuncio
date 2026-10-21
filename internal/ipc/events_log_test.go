package ipc

import (
	"bytes"
	"errors"
	"log"
	"os"
	"strings"
	"testing"
)

// leakyFrame is an event whose encoding fails with an error that quotes a user value.
type leakyFrame struct{}

func (leakyFrame) MarshalJSON() ([]byte, error) {
	return nil, errors.New("cannot encode the text of Ada Lovelace")
}

func TestAnEventThatCannotBeEncodedIsLoggedByItsTypesOnly(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	if n := NewBroker().Publish(leakyFrame{}); n != 0 {
		t.Fatalf("Publish = %d, want 0", n)
	}
	got := buf.String()
	if !strings.Contains(got, "ipc: cannot encode a ") || !strings.Contains(got, " event (") {
		t.Errorf("log = %q, want the event and its error by their types", got)
	}
	if strings.Contains(got, "Ada") {
		t.Errorf("log = %q carries the error's text", got)
	}
}
