package wa

import (
	"sync/atomic"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// unreadHookSession is a started session whose unread hook counts its calls.
func unreadHookSession(t *testing.T) (*Session, *fakeClient, *atomic.Int32) {
	t.Helper()
	db := openTestStore(t)
	fake := &fakeClient{device: true}
	s := NewSession(fake, db, nil)
	var calls atomic.Int32
	s.SetUnreadChanged(func() { calls.Add(1) })
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	return s, fake, &calls
}

func TestUnreadHookRunsWhenAnIncomingMessageIsStored(t *testing.T) {
	_, fake, calls := unreadHookSession(t)
	msg := textEvent(ada, "U1", false, t0, "hi")
	fake.emit(msg)
	if n := calls.Load(); n != 1 {
		t.Fatalf("unread hook ran %d times after one incoming message, want 1", n)
	}
	// A duplicate stores nothing and my own message leaves the count alone: no call.
	fake.emit(msg)
	fake.emit(textEvent(ada, "U2", true, t0.Add(time.Second), "mine"))
	if n := calls.Load(); n != 1 {
		t.Errorf("unread hook ran %d times after a duplicate and an own message, want still 1", n)
	}
}

func TestUnreadHookRunsWhenAChatIsReadOnAnotherDevice(t *testing.T) {
	_, fake, calls := unreadHookSession(t)
	fake.emit(textEvent(ada, "U1", false, t0, "hi"))
	fake.emit(markReadEvent(ada, true))
	if n := calls.Load(); n != 2 {
		t.Errorf("unread hook ran %d times after a message and an app-state read, want 2", n)
	}
}

func TestUnreadHookRunsWhenAUIMarksAChatRead(t *testing.T) {
	s, fake, calls := unreadHookSession(t)
	fake.emit(textEvent(ada, "U1", false, t0, "hi"))
	if _, err := s.MarkRead(ada.String()); err != nil {
		t.Fatalf("MarkRead: %v", err)
	}
	if n := calls.Load(); n != 2 {
		t.Errorf("unread hook ran %d times after a message and mark_read, want 2", n)
	}
}

func TestUnreadHookRunsWhenAMergeAtStartChangesTheTotal(t *testing.T) {
	db := openTestStore(t)
	for _, m := range []store.Message{
		{ID: "D1", ChatJID: ada.String(), Kind: store.KindText, Text: "hi", Timestamp: t0, Sender: ada.String()},
		{ID: "D1", ChatJID: adaLID.String(), Kind: store.KindText, Text: "hi", Timestamp: t0, Sender: adaLID.String()},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	s := NewSession(newLIDClient(ada.String(), adaLID.String()), db, nil)
	var calls atomic.Int32
	s.SetUnreadChanged(func() { calls.Add(1) })
	if err := s.Start(); err != nil {
		t.Fatal(err)
	}
	if n, _ := store.TotalUnread(db); n != 1 || calls.Load() == 0 {
		t.Errorf("total %d after the merge, hook calls %d; want 1 and at least one call", n, calls.Load())
	}
}

func TestUnreadHookRunsWhenAnOwnMessageMergesAReadDuplicate(t *testing.T) {
	db := openTestStore(t)
	for _, m := range []store.Message{
		{ID: "D1", ChatJID: ada.String(), Kind: store.KindText, Text: "hi", Timestamp: t0, Read: true, Sender: ada.String()},
		{ID: "D1", ChatJID: adaLID.String(), Kind: store.KindText, Text: "hi", Timestamp: t0, Sender: adaLID.String()},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	c := newLIDClient()
	s := NewSession(c, db, nil)
	var calls atomic.Int32
	s.SetUnreadChanged(func() { calls.Add(1) })
	if err := s.Start(); err != nil {
		t.Fatal(err)
	}
	c.learn(ada.String(), adaLID.String())
	c.emit(textEvent(adaLID, "M1", true, t0.Add(time.Minute), "mine"))
	if n, _ := store.TotalUnread(db); n != 0 || calls.Load() == 0 {
		t.Errorf("total %d after the merge, hook calls %d; want 0 and at least one call", n, calls.Load())
	}
}
