package wa

import (
	"context"
	"errors"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// ErrLogoutNotConfirmed means WhatsApp did not confirm the unlink. The device may still be linked,
// so nothing local is deleted after it.
var ErrLogoutNotConfirmed = errors.New("WhatsApp did not confirm the logout; nothing was deleted (if this device is already gone from your phone's Linked devices, use nuncio purge --confirm)")

// Logout tells WhatsApp to unlink this device through the session's own connection, so it never
// opens a second session of the device. On success the session is unlinked and stops reconnecting
// for good; the caller then deletes the local data. Without a linked device, or after WhatsApp logged the device out
// itself, it returns ipc.ErrNotLinked and calls nothing. When WhatsApp does not confirm it returns
// ErrLogoutNotConfirmed and the session is left as it was.
func (s *Session) Logout(ctx context.Context) error {
	if s.client == nil || !s.client.HasDevice() || s.State() == StateUnlinked {
		return ipc.ErrNotLinked
	}
	// Held across the unlink and the stop, so a reconnect waiting on connMu runs only after the
	// stop is recorded, and then connects nothing.
	s.connMu.Lock()
	defer s.connMu.Unlock()
	err := s.client.Logout(ctx)
	if err != nil {
		// whatsmeow's error text is not logged: only its type, so no identifier reaches the log.
		s.mu.Lock()
		logf := s.logf
		s.mu.Unlock()
		logf("wa: logout not confirmed (%s)", ErrKind(err))
		return ErrLogoutNotConfirmed
	}
	s.stop("logged out on request", StateUnlinked)
	return nil
}
