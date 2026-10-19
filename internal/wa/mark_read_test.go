package wa

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/types"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// storeIncomingFrom stores one incoming text of sender in chat as if it had arrived.
func storeIncomingFrom(t *testing.T, fake *fakeClient, chat, sender types.JID, id string, at time.Time) {
	t.Helper()
	evt := textEvent(chat, id, false, at, "hi")
	evt.Info.Sender = sender
	fake.emit(evt)
}

// readState is every stored message's chat, id and read flag, in insertion order: what mark_read
// may change.
func readState(t *testing.T, db *sql.DB) string {
	t.Helper()
	rows, err := db.Query(`SELECT chat_jid, id, read FROM messages ORDER BY seq`)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rows.Close() }()
	var b strings.Builder
	for rows.Next() {
		var chat, id string
		var read bool
		if err := rows.Scan(&chat, &id, &read); err != nil {
			t.Fatal(err)
		}
		b.WriteString(chat + " " + id + " " + map[bool]string{true: "read", false: "unread"}[read] + "\n")
	}
	return b.String()
}

func TestMarkReadSendsOneReceiptForTheUnreadIncomingMessagesClearsThemAndPublishes(t *testing.T) {
	s, fake, events := subscribedSession(t)
	storeIncomingFrom(t, fake, ada, ada, "A1", t0)
	storeIncomingFrom(t, fake, ada, ada, "A2", t0.Add(time.Second))
	fake.emit(textEvent(ada, "MINE", true, t0.Add(2*time.Second), "ok"))
	storeIncomingFrom(t, fake, grace, grace, "G1", t0)
	events()

	before := time.Now()
	total, err := s.MarkRead(ada.String())
	if err != nil || total != 1 {
		t.Fatalf("MarkRead = %d, %v; want 1 (Grace's message is still unread)", total, err)
	}
	want := []string{"15551230001@s.whatsapp.net 15551230001@s.whatsapp.net read A1,A2"}
	if got := fake.markReadCalls; !reflect.DeepEqual(got, want) {
		t.Errorf("receipts = %q, want %q", got, want)
	}
	if fake.markReadAt.Before(before) {
		t.Errorf("receipt time = %v, want the time of the view", fake.markReadAt)
	}
	if n, _ := store.UnreadCount(s.Store(), ada.String()); n != 0 {
		t.Errorf("unread of ada = %d, want 0", n)
	}
	assertEvents(t, events(), `{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":0}`)
}

func TestMarkReadInAGroupSendsOneReceiptPerSenderAndClearsM1RowsWithoutOne(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	// A group message stored before M2 recorded senders: a receipt for it would need a participant.
	if _, err := store.InsertMessage(s.Store(), store.Message{ID: "OLD1", ChatJID: group.String(),
		Kind: store.KindText, Text: "old", Timestamp: t0.Add(-time.Minute)}); err != nil {
		t.Fatal(err)
	}
	storeIncomingFrom(t, fake, group, ada, "Q1", t0)
	storeIncomingFrom(t, fake, group, grace, "Q2", t0.Add(time.Second))
	storeIncomingFrom(t, fake, group, ada, "Q3", t0.Add(2*time.Second))

	if _, err := s.MarkRead(group.String()); err != nil {
		t.Fatalf("MarkRead: %v", err)
	}
	want := []string{
		"120363000000000000@g.us 15551230001@s.whatsapp.net read Q1,Q3",
		"120363000000000000@g.us 15551230002@s.whatsapp.net read Q2",
	}
	if got := fake.markReadCalls; !reflect.DeepEqual(got, want) {
		t.Errorf("receipts = %q, want %q (none for OLD1)", got, want)
	}
	if n, _ := store.UnreadCount(s.Store(), group.String()); n != 0 {
		t.Errorf("unread of the group = %d, want 0 (OLD1 cleared too)", n)
	}
}

// unreadIDs stores n unread incoming messages of sender in chat, a second apart, and returns their
// ids oldest first.
func unreadIDs(t *testing.T, fake *fakeClient, chat, sender types.JID, n int) []string {
	t.Helper()
	ids := make([]string, n)
	for i := range n {
		ids[i] = fmt.Sprintf("M%03d", i)
		storeIncomingFrom(t, fake, chat, sender, ids[i], t0.Add(time.Duration(i)*time.Second))
	}
	return ids
}

func TestMarkReadSendsAtMostFiftyIdsPerReceiptOldestFirst(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	ids := unreadIDs(t, fake, ada, ada, 120)

	if _, err := s.MarkRead(ada.String()); err != nil {
		t.Fatalf("MarkRead: %v", err)
	}
	prefix := "15551230001@s.whatsapp.net 15551230001@s.whatsapp.net read "
	want := []string{
		prefix + strings.Join(ids[:50], ","),
		prefix + strings.Join(ids[50:100], ","),
		prefix + strings.Join(ids[100:], ","),
	}
	if got := fake.markReadCalls; !reflect.DeepEqual(got, want) {
		t.Errorf("receipts = %q,\nwant %q", got, want)
	}
	if n, _ := store.UnreadCount(s.Store(), ada.String()); n != 0 {
		t.Errorf("unread of ada = %d, want 0", n)
	}
}

func TestMarkReadStopsAtTheFirstFailedChunkAndKeepsTheChatUnread(t *testing.T) {
	// The 240 stored-message events overflow the test subscriber, so events are not checked here;
	// TestMarkReadThatCannotBeSentKeepsTheChatUnread covers "no event_unread after a failure".
	s, fake, _ := subscribedSession(t)
	unreadIDs(t, fake, ada, ada, 120)
	fake.markReadErr, fake.markReadFailAt = errors.New("socket closed"), 2
	before := readState(t, s.Store())

	if _, err := s.MarkRead(ada.String()); !errors.Is(err, errReceiptNotSent) {
		t.Errorf("MarkRead = %v, want %v", err, errReceiptNotSent)
	}
	if got := len(fake.markReadCalls); got != 2 {
		t.Errorf("%d receipts sent, want 2: nothing after the failed second chunk", got)
	}
	if got := readState(t, s.Store()); got != before {
		t.Errorf("read state changed after a failed chunk")
	}
}

func TestMarkReadWithReadReceiptsOffSendsReadSelf(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	fake.readReceipts = "none"
	storeIncomingFrom(t, fake, ada, ada, "A1", t0)

	if _, err := s.MarkRead(ada.String()); err != nil {
		t.Fatalf("MarkRead: %v", err)
	}
	want := []string{"15551230001@s.whatsapp.net 15551230001@s.whatsapp.net read-self A1"}
	if got := fake.markReadCalls; !reflect.DeepEqual(got, want) {
		t.Errorf("receipts = %q, want %q", got, want)
	}
}

// Every failure leaves the read state exactly as it was, answers why, sends nothing more and is
// never retried, not even after a reconnect.
func TestMarkReadThatCannotBeSentKeepsTheChatUnread(t *testing.T) {
	for _, tc := range []struct {
		name  string
		setup func(s *Session, fake *fakeClient)
		want  error
		calls int
	}{
		{"offline", func(s *Session, fake *fakeClient) { s.set(StateOffline) }, ErrMarkReadOffline, 0},
		{"privacy unknown", func(s *Session, fake *fakeClient) { fake.privacyErr = errors.New("iq timed out") }, errReadPrivacy, 0},
		{"not accepted", func(s *Session, fake *fakeClient) { fake.markReadErr = errors.New("socket closed") }, errReceiptNotSent, 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, fake, events := subscribedSession(t)
			storeIncomingFrom(t, fake, group, ada, "Q1", t0)
			storeIncomingFrom(t, fake, group, grace, "Q2", t0.Add(time.Second))
			tc.setup(s, fake)
			events()
			before := readState(t, s.Store())

			if _, err := s.MarkRead(group.String()); !errors.Is(err, tc.want) {
				t.Errorf("MarkRead = %v, want %v", err, tc.want)
			}
			if got := readState(t, s.Store()); got != before {
				t.Errorf("read state changed:\n%s\nwant\n%s", got, before)
			}
			if got := len(fake.markReadCalls); got != tc.calls {
				t.Errorf("receipts = %q, want %d attempt(s), never a second sender after a failure", fake.markReadCalls, tc.calls)
			}
			if ev := events(); len(ev) != 0 && strings.Contains(strings.Join(ev, ""), "event_unread") {
				t.Errorf("events = %q, want no unread change", ev)
			}
			fake.markReadErr, fake.privacyErr = nil, nil
			reconnect(s, fake)
			if got := len(fake.markReadCalls); got != tc.calls {
				t.Errorf("after a reconnect receipts = %q, want no retry", fake.markReadCalls)
			}
		})
	}
}

func TestMarkReadNeedsALinkedDevice(t *testing.T) {
	fake := &fakeClient{}
	unlinked := NewSession(fake, openTestStore(t), nil)
	if _, err := unlinked.MarkRead(ada.String()); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("MarkRead unlinked = %v, want ipc.ErrNotLinked", err)
	}
	if _, err := NewSession(nil, nil, nil).MarkRead(ada.String()); !errors.Is(err, ipc.ErrNotLinked) {
		t.Errorf("MarkRead with no client = %v, want ipc.ErrNotLinked", err)
	}
	if fake.markReads != 0 || fake.privacyAsks != 0 {
		t.Errorf("unlinked: %d receipts, %d privacy reads; want none", fake.markReads, fake.privacyAsks)
	}
}

func TestMarkReadOfAChatWithNothingUnreadSendsNothing(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.emit(textEvent(ada, "MINE", true, t0, "ok"))
	storeIncomingFrom(t, fake, grace, grace, "G1", t0)
	events()

	total, err := s.MarkRead(ada.String())
	if err != nil || total != 1 {
		t.Errorf("MarkRead = %d, %v; want the total 1 and no error", total, err)
	}
	if fake.markReads != 0 || fake.privacyAsks != 0 {
		t.Errorf("%d receipts, %d privacy reads; want none", fake.markReads, fake.privacyAsks)
	}
	if ev := events(); len(ev) != 0 {
		t.Errorf("events = %q, want none", ev)
	}
}

// whatsmeow's ParseJID error can quote part of the JID, and a MarkRead error is logged, so the
// adapter's error names no JID. Both checks fail before the whatsmeow client is used.
func TestMarkReadAdapterErrorsCarryNoJID(t *testing.T) {
	const bad = "15551230009:15551230008x@s.whatsapp.net"
	for _, tc := range []struct{ chat, sender string }{
		{bad, ada.String()},
		{group.String(), bad},
	} {
		err := whatsmeowClient{}.MarkRead(context.Background(), tc.chat, tc.sender, []string{"Q1"}, t0, "read")
		if err == nil {
			t.Fatalf("MarkRead(%q, %q) succeeded, want an error", tc.chat, tc.sender)
		}
		if strings.Contains(err.Error(), "1555123000") {
			t.Errorf("MarkRead(%q, %q) error %q carries the JID", tc.chat, tc.sender, err)
		}
	}
}
