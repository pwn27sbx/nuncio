package wa

import (
	"context"
	"errors"
	"testing"

	waevents "go.mau.fi/whatsmeow/types/events"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

func TestLogoutUnlinksTheDeviceOnceAndStopsReconnecting(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if err := s.Logout(context.Background()); err != nil {
		t.Fatalf("Logout: %v", err)
	}
	if fake.logouts != 1 {
		t.Errorf("client Logout called %d times, want 1", fake.logouts)
	}
	if got := s.State(); got != StateUnlinked {
		t.Errorf("State() after Logout = %q, want %q", got, StateUnlinked)
	}
	if s.StopReason() == "" {
		t.Error("StopReason() after Logout is empty, want a stop: nothing may reconnect a logged-out device")
	}
}

func TestLogoutWithoutADeviceIsNotLinkedAndCallsNothing(t *testing.T) {
	for name, s := range map[string]*Session{
		"no device":       NewSession(&fakeClient{device: false}, nil, nil),
		"no device store": NewSession(nil, nil, nil),
	} {
		if err := s.Logout(context.Background()); !errors.Is(err, ipc.ErrNotLinked) {
			t.Errorf("%s: Logout = %v, want ErrNotLinked", name, err)
		}
	}
	fake := &fakeClient{device: false}
	_ = NewSession(fake, nil, nil).Logout(context.Background())
	if fake.logouts != 0 {
		t.Errorf("client Logout called %d times without a device, want 0", fake.logouts)
	}
}

// After WhatsApp logged the device out itself there is nothing to unlink: only purge deletes.
func TestLogoutAfterAnExternalLogoutIsNotLinkedAndCallsNothing(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	fake.emit(&waevents.LoggedOut{})
	if err := s.Logout(context.Background()); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("Logout after an external logout = %v, want ErrNotLinked", err)
	}
	if fake.logouts != 0 {
		t.Errorf("client Logout called %d times after an external logout, want 0", fake.logouts)
	}
}

func TestLogoutThatWhatsAppDoesNotConfirmKeepsTheSession(t *testing.T) {
	fake := &fakeClient{device: true, logoutErr: errors.New("error sending logout request: timed out")}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	err := s.Logout(context.Background())
	if err == nil || errors.Is(err, ipc.ErrNotLinked) {
		t.Fatalf("Logout = %v, want a not-confirmed error", err)
	}
	if got := s.State(); got != StateLinked {
		t.Errorf("State() after a refused Logout = %q, want still %q", got, StateLinked)
	}
	if s.StopReason() != "" {
		t.Errorf("StopReason() after a refused Logout = %q, want none", s.StopReason())
	}
}
