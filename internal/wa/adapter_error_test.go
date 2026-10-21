package wa

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow"
)

// adapterCause stands for the typed cause whatsmeow answers under the adapter's "wa: <what it was
// doing>: %w" wrapping; its text plays the JID whatsmeow's text can carry.
type adapterCause struct{}

func (adapterCause) Error() string { return "iq for 15551230001@s.whatsapp.net failed" }

const (
	causeKind   = "(wa.adapterCause)"
	wrapperKind = "*fmt.wrapError"
	causeSecret = "15551230001"
)

// wrapped is the error the adapter answers for a whatsmeow failure.
func wrapped(doing string, cause error) error { return fmt.Errorf("wa: %s: %w", doing, cause) }

// checkKindLogged reports a log that does not name the wrapped cause's type, names the wrapper's,
// or carries the cause's text.
func checkKindLogged(t *testing.T, logs, line string) {
	t.Helper()
	if !strings.Contains(logs, line+" "+causeKind) {
		t.Errorf("log = %q, want %q naming the cause's type", logs, line+" "+causeKind)
	}
	if strings.Contains(logs, wrapperKind) {
		t.Errorf("log = %q names the wrapper, not the cause", logs)
	}
	if strings.Contains(logs, causeSecret) {
		t.Errorf("log = %q carries the error's text", logs)
	}
}

// A send the adapter answers wrapped is still classified by its cause: an ack timeout or a dropped
// socket is an unknown outcome, anything else is not.
func TestAWrappedSendErrorKeepsItsOutcome(t *testing.T) {
	for _, tc := range []struct {
		cause error
		want  bool
	}{
		{context.DeadlineExceeded, true},
		{whatsmeow.ErrMessageTimedOut, true},
		{&whatsmeow.DisconnectedError{Action: "message send (retry)"}, true},
		{whatsmeow.ErrNotConnected, false},
		{adapterCause{}, false},
	} {
		if got := unknownOutcome(wrapped("sending a message", tc.cause)); got != tc.want {
			t.Errorf("unknownOutcome(wrapped %T) = %v, want %v", tc.cause, got, tc.want)
		}
	}
}

func TestAWrappedSendFailureIsLoggedByItsCause(t *testing.T) {
	logs := captureLog(t)
	s, fake, _ := subscribedSession(t)
	fake.sendErr = wrapped("sending a message", adapterCause{})
	if _, err := s.Send(ada.String(), "On my way", "c-1"); !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send = %v, want ErrNotSent", err)
	}
	checkKindLogged(t, logs.String(), "wa: a send failed")
}

func TestAWrappedReactionFailureIsLoggedByItsCause(t *testing.T) {
	logs := captureLog(t)
	s, fake, events := subscribedSession(t)
	storeIncoming(t, fake, group, "Q1", "Which venue?")
	events()
	fake.sendErr = wrapped("sending a message", adapterCause{})
	if err := s.React(group.String(), "Q1", "👍"); !errors.Is(err, errReactNotSent) {
		t.Fatalf("React = %v, want errReactNotSent", err)
	}
	checkKindLogged(t, logs.String(), "wa: a reaction failed")
}

func TestWrappedReadReceiptFailuresAreLoggedByTheirCause(t *testing.T) {
	for _, tc := range []struct {
		name, line string
		setup      func(fake *fakeClient)
		want       error
	}{
		{"privacy", "wa: cannot read the read-receipt privacy setting",
			func(fake *fakeClient) { fake.privacyErr = wrapped("reading the privacy settings", adapterCause{}) }, errReadPrivacy},
		{"receipt", "wa: a read receipt failed",
			func(fake *fakeClient) { fake.markReadErr = wrapped("sending a read receipt", adapterCause{}) }, errReceiptNotSent},
	} {
		t.Run(tc.name, func(t *testing.T) {
			logs := captureLog(t)
			s, fake, events := subscribedSession(t)
			storeIncomingFrom(t, fake, group, ada, "Q1", t0)
			events()
			tc.setup(fake)
			if _, err := s.MarkRead(group.String()); !errors.Is(err, tc.want) {
				t.Fatalf("MarkRead = %v, want %v", err, tc.want)
			}
			checkKindLogged(t, logs.String(), tc.line)
		})
	}
}

func TestWrappedNameReadFailuresAreLoggedByTheirCause(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	s, _, logs := lidSession(t, c, db)
	// The first connection's own group query runs off the event goroutine; it must be over before
	// the failures are set, so every logged line comes from the calls below.
	waitFor(t, "the first group query", func() bool { return c.joined() > 0 })

	c.nmu.Lock()
	c.contactErr = wrapped("reading a contact", adapterCause{})
	c.groupsErr = wrapped("querying the joined groups", adapterCause{})
	c.nmu.Unlock()
	if _, ok := s.contactName(c, adaLID.String()); ok {
		t.Fatal("contactName ok after a failed read")
	}
	s.syncGroupNames()
	all := strings.Join(*logs, "\n")
	checkKindLogged(t, all, "wa: cannot read a contact's name")
	checkKindLogged(t, all, "wa: cannot read the group subjects")
}

func TestAWrappedLogoutFailureIsLoggedByItsCause(t *testing.T) {
	fake := &fakeClient{device: true, logoutErr: wrapped("logging out", adapterCause{})}
	s := NewSession(fake, nil, nil)
	t.Cleanup(s.Close)
	var logs []string
	s.logf = func(format string, args ...any) { logs = append(logs, fmt.Sprintf(format, args...)) }
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := s.Logout(ctx); !errors.Is(err, ErrLogoutNotConfirmed) {
		t.Fatalf("Logout = %v, want ErrLogoutNotConfirmed", err)
	}
	checkKindLogged(t, strings.Join(logs, "\n"), "wa: logout not confirmed")
}
