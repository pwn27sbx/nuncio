package wa

import (
	waevents "go.mau.fi/whatsmeow/types/events"
)

// Events routes the whatsmeow events nuncio acts on to typed callbacks; a nil callback ignores its
// event. It is the one event switch, shared by `nuncio link` and the daemon's Session, so the two
// cannot drift apart. Register Handle before Connect, so no event is missed.
type Events struct {
	// Paired receives the user part of the new device's JID.
	Paired       func(user string)
	Connected    func()
	Disconnected func()
	// LoggedOut: the device was unlinked (from the phone or by the server).
	LoggedOut func()
	// Dropped: the session ended without a Disconnected event and without a logout: a temporary
	// ban, the stream replaced by another connection, or a failed connect.
	Dropped func()
	Message func(*waevents.Message)
}

// Handle dispatches one raw whatsmeow event.
func (e Events) Handle(raw any) {
	switch v := raw.(type) {
	case *waevents.PairSuccess:
		if e.Paired != nil {
			e.Paired(v.ID.User)
		}
	case *waevents.Connected:
		if e.Connected != nil {
			e.Connected()
		}
	case *waevents.Disconnected:
		if e.Disconnected != nil {
			e.Disconnected()
		}
	case *waevents.LoggedOut:
		if e.LoggedOut != nil {
			e.LoggedOut()
		}
	case *waevents.TemporaryBan, *waevents.StreamReplaced, *waevents.ConnectFailure:
		if e.Dropped != nil {
			e.Dropped()
		}
	case *waevents.Message:
		if e.Message != nil {
			e.Message(v)
		}
	}
}
