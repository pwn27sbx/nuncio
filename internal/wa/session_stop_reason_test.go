package wa

import (
	"context"
	"testing"
	"time"

	waevents "go.mau.fi/whatsmeow/types/events"
)

// lastFrame is the last frame in frames, "" when there is none.
func lastFrame(frames []string) string {
	if len(frames) == 0 {
		return ""
	}
	return frames[len(frames)-1]
}

// Q6: each way WhatsApp stops the session publishes, with the stop's state, one fixed client-safe
// stop_reason; WhatsApp's own text (ban codes, expiry, failure codes) never reaches the frame.
func TestEachStopPublishesItsFixedReason(t *testing.T) {
	cases := []struct {
		name  string
		event any
		want  string
	}{
		{"logged out", &waevents.LoggedOut{}, `{"v":1,"op":"event_connection","session":"unlinked","stop_reason":"logged_out"}`},
		{"temporary ban", &waevents.TemporaryBan{Code: waevents.TempBanSentToTooManyPeople, Expire: 3 * time.Hour}, `{"v":1,"op":"event_connection","session":"offline","stop_reason":"temporary_ban"}`},
		{"stream replaced", &waevents.StreamReplaced{}, `{"v":1,"op":"event_connection","session":"offline","stop_reason":"stream_replaced"}`},
		{"client outdated", &waevents.ClientOutdated{}, `{"v":1,"op":"event_connection","session":"offline","stop_reason":"client_outdated"}`},
		{"connect rejected", &waevents.ConnectFailure{Reason: waevents.ConnectFailureBadUserAgent}, `{"v":1,"op":"event_connection","session":"offline","stop_reason":"connect_rejected"}`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeClient{device: true}
			s := NewSession(fake, nil, nil)
			s.logf = t.Logf
			t.Cleanup(s.Close)
			events := eventsOf(t, s)
			if err := s.Start(); err != nil {
				t.Fatalf("Start: %v", err)
			}
			fake.emit(tc.event)
			if got := lastFrame(events()); got != tc.want {
				t.Errorf("last frame after %T = %s, want %s", tc.event, got, tc.want)
			}
		})
	}
}

// A logout asked for here (nuncio logout --confirm) is a logout too.
func TestALogoutOnRequestPublishesLoggedOut(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	s.logf = t.Logf
	t.Cleanup(s.Close)
	events := eventsOf(t, s)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	if err := s.Logout(context.Background()); err != nil {
		t.Fatalf("Logout: %v", err)
	}
	want := `{"v":1,"op":"event_connection","session":"unlinked","stop_reason":"logged_out"}`
	if got := lastFrame(events()); got != want {
		t.Errorf("last frame after Logout = %s, want %s", got, want)
	}
	if got := s.StopKind(); got != "logged_out" {
		t.Errorf("StopKind() after Logout = %q, want logged_out", got)
	}
}

// A ban that lands while the session is already offline (a connect after a drop) leaves the
// state as it was, but its reason is still published: the client learns why it stays offline.
func TestAStopWhileOfflineStillPublishesItsReason(t *testing.T) {
	fake := &fakeClient{device: true}
	s, _, _ := reconnectingSession(t, fake)
	events := eventsOf(t, s)
	fake.emit(&waevents.Disconnected{})
	fake.emit(&waevents.TemporaryBan{})
	fake.emit(&waevents.StreamReplaced{}) // a later stop keeps the first reason: nothing new
	assertEvents(t, events(),
		`{"v":1,"op":"event_connection","session":"offline"}`,
		`{"v":1,"op":"event_connection","session":"offline","stop_reason":"temporary_ban"}`,
	)
	if got := s.StopKind(); got != "temporary_ban" {
		t.Errorf("StopKind() = %q, want temporary_ban (the first stop's)", got)
	}
}

// StopKind is what status reports: the stop's fixed kind once stopped.
func TestStopKindNamesEachStop(t *testing.T) {
	cases := []struct {
		event any
		want  string
	}{
		{&waevents.LoggedOut{}, "logged_out"},
		{&waevents.TemporaryBan{Code: waevents.TempBanBlockedByUsers, Expire: time.Hour}, "temporary_ban"},
		{&waevents.StreamReplaced{}, "stream_replaced"},
		{&waevents.ClientOutdated{}, "client_outdated"},
		{&waevents.ConnectFailure{Reason: waevents.ConnectFailureGeneric}, "connect_rejected"},
	}
	for _, tc := range cases {
		fake := &fakeClient{device: true}
		s, _, _ := reconnectingSession(t, fake)
		if got := s.StopKind(); got != "" {
			t.Errorf("StopKind() before %T = %q, want empty", tc.event, got)
		}
		fake.emit(tc.event)
		fake.emit(&waevents.Disconnected{})
		if got := s.StopKind(); got != tc.want {
			t.Errorf("StopKind() after %T = %q, want %q", tc.event, got, tc.want)
		}
	}
}

// A drop and its reconnect are not a stop: no frame carries stop_reason and StopKind stays empty,
// so the frames are byte-identical to the ones before Q6.
func TestADropAndReconnectHaveNoStopReason(t *testing.T) {
	fake := &fakeClient{device: true}
	s, timers, _ := reconnectingSession(t, fake)
	events := eventsOf(t, s)
	fake.emit(&waevents.Disconnected{})
	timers.fire(t) // reconnects: Connected
	assertEvents(t, events(),
		`{"v":1,"op":"event_connection","session":"offline"}`,
		`{"v":1,"op":"event_connection","session":"linked"}`,
	)
	if got := s.StopKind(); got != "" {
		t.Errorf("StopKind() after a drop and reconnect = %q, want empty", got)
	}
}

// stopKind maps the session's own reasons only; anything else (a reason this build did not
// write) is not passed through but named by the generic kind.
func TestStopKindNeverPassesTextThrough(t *testing.T) {
	for reason, want := range map[string]string{
		"":                       "",
		"something new happened": "other",
		"logged out":             "logged_out",
		"logged out on request":  "logged_out",
	} {
		if got := stopKind(reason); got != want {
			t.Errorf("stopKind(%q) = %q, want %q", reason, got, want)
		}
	}
}
