package wa

import (
	"time"

	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
)

// nonAD is the non-empty JIDs among jids, without device, as strings.
func nonAD(jids ...types.JID) []string {
	var out []string
	for _, j := range jids {
		if !j.IsEmpty() {
			out = append(out, j.ToNonAD().String())
		}
	}
	return out
}

// Events routes the whatsmeow events nuncio acts on to typed callbacks; a nil callback ignores its
// event. It is the one event switch, shared by `nuncio link` and the daemon's Session, so the two
// cannot drift apart. Register Handle before Connect, so no event is missed.
type Events struct {
	// Paired receives the user part of the new device's JID.
	Paired       func(user string)
	Connected    func()
	Disconnected func()
	// KeepAliveTimeout: one of whatsmeow's keepalive pings failed (it pings every 20-30 s);
	// lastSuccess is when one last succeeded. whatsmeow emits it for every failed ping and treats
	// the socket as dead only after KeepAliveMaxFailTime without a success (keepalive.go). Its
	// KeepAliveRestored, after a ping works again, needs no action and is ignored.
	KeepAliveTimeout func(lastSuccess time.Time)
	// LoggedOut: the device was unlinked (from the phone or by the server).
	LoggedOut func()
	// Stopped: the server ended the session in a way a reconnect must not fight: a temporary ban,
	// the stream replaced by another connection, a client too old to be accepted, or a rejected
	// connect. reason is a short description (codes and expiry only, no personal data).
	Stopped func(reason string)
	Message func(*waevents.Message)
	// The app-state changes made on another device (see appstate.go).
	Pin            func(*waevents.Pin)
	Archive        func(*waevents.Archive)
	Mute           func(*waevents.Mute)
	MarkChatAsRead func(*waevents.MarkChatAsRead)
	// ContactName: whatsmeow stored a new push name, business name or address-book name (app-state
	// contact action) for these user JIDs (without device; the alternate JID of a push name too).
	ContactName func(users ...string)
	// GroupName: a group's subject, from a GroupInfo subject change or a JoinedGroup.
	GroupName func(group, subject string)
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
	case *waevents.KeepAliveTimeout:
		if e.KeepAliveTimeout != nil {
			e.KeepAliveTimeout(v.LastSuccess)
		}
	case *waevents.LoggedOut:
		if e.LoggedOut != nil {
			e.LoggedOut()
		}
	case *waevents.TemporaryBan:
		if e.Stopped != nil {
			e.Stopped(v.String())
		}
	case *waevents.StreamReplaced:
		if e.Stopped != nil {
			e.Stopped("stream replaced by another connection of this device")
		}
	case *waevents.ClientOutdated:
		if e.Stopped != nil {
			e.Stopped("client is out of date")
		}
	case *waevents.ConnectFailure:
		// whatsmeow emits this only for codes it does not handle itself (e.g. 400, 409, 415, 418,
		// 501), after it already gave up on the connection; the retryable 500 and 503 arrive as
		// Disconnected instead. Retrying a server rejection would only repeat it.
		if e.Stopped != nil {
			e.Stopped("connect rejected: " + v.Reason.String())
		}
	case *waevents.Message:
		if e.Message != nil {
			e.Message(v)
		}
	case *waevents.Pin:
		if e.Pin != nil {
			e.Pin(v)
		}
	case *waevents.Archive:
		if e.Archive != nil {
			e.Archive(v)
		}
	case *waevents.Mute:
		if e.Mute != nil {
			e.Mute(v)
		}
	case *waevents.MarkChatAsRead:
		if e.MarkChatAsRead != nil {
			e.MarkChatAsRead(v)
		}
	case *waevents.PushName:
		if e.ContactName != nil {
			e.ContactName(nonAD(v.JID, v.JIDAlt)...)
		}
	case *waevents.BusinessName:
		if e.ContactName != nil {
			e.ContactName(nonAD(v.JID)...)
		}
	case *waevents.Contact:
		if e.ContactName != nil {
			e.ContactName(nonAD(v.JID)...)
		}
	case *waevents.GroupInfo:
		if e.GroupName != nil && v.Name != nil {
			e.GroupName(v.JID.String(), v.Name.Name)
		}
	case *waevents.JoinedGroup:
		if e.GroupName != nil {
			e.GroupName(v.JID.String(), v.GroupName.Name)
		}
	}
}
