package wa

import (
	"database/sql"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/store"
)

func pinEvent(jid types.JID, pinned bool) *waevents.Pin {
	return &waevents.Pin{JID: jid, Timestamp: t0, Action: &waSyncAction.PinAction{Pinned: proto.Bool(pinned)}}
}

func archiveEvent(jid types.JID, archived bool) *waevents.Archive {
	return &waevents.Archive{JID: jid, Timestamp: t0,
		Action: &waSyncAction.ArchiveChatAction{Archived: proto.Bool(archived)}}
}

// muteEvent mutes until endMillis (Unix milliseconds, -1 for no end) or, with muted false, unmutes.
func muteEvent(jid types.JID, muted bool, endMillis int64) *waevents.Mute {
	act := &waSyncAction.MuteAction{Muted: proto.Bool(muted)}
	if muted {
		act.MuteEndTimestamp = proto.Int64(endMillis)
	}
	return &waevents.Mute{JID: jid, Timestamp: t0, Action: act}
}

func markReadEvent(jid types.JID, read bool) *waevents.MarkChatAsRead {
	return &waevents.MarkChatAsRead{JID: jid, Timestamp: t0,
		Action: &waSyncAction.MarkChatAsReadAction{Read: proto.Bool(read)}}
}

// storedChat is the stored chat jid, failing the test when it is not stored.
func storedChat(t *testing.T, db *sql.DB, jid types.JID) store.Chat {
	t.Helper()
	chats, err := store.ListChats(db, 200)
	if err != nil {
		t.Fatalf("ListChats: %v", err)
	}
	for _, c := range chats {
		if c.JID == jid.String() {
			return c
		}
	}
	t.Fatalf("chat %s is not stored: %+v", jid, chats)
	return store.Chat{}
}

func TestPinEventPinsAndUnpinsTheChat(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))

	fake.emit(pinEvent(ada, true))
	if c := storedChat(t, db, ada); !c.Pinned {
		t.Fatalf("after a pin = %+v, want pinned", c)
	}
	fake.emit(pinEvent(ada, false))
	if c := storedChat(t, db, ada); c.Pinned {
		t.Fatalf("after an unpin = %+v, want not pinned", c)
	}
}

func TestArchiveEventArchivesAndUnarchivesTheChat(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))

	fake.emit(archiveEvent(ada, true))
	if c := storedChat(t, db, ada); !c.Archived {
		t.Fatalf("after an archive = %+v, want archived", c)
	}
	fake.emit(archiveEvent(ada, false))
	if c := storedChat(t, db, ada); c.Archived {
		t.Fatalf("after an unarchive = %+v, want not archived", c)
	}
}

func TestMuteEventMutesForeverUntilAnEndTimeAndUnmutes(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))

	fake.emit(muteEvent(ada, true, -1))
	if c := storedChat(t, db, ada); !c.MutedUntil.Equal(store.MutedForever) {
		t.Fatalf("after a mute with no end = %+v, want MutedForever", c)
	}
	end := t0.Add(8 * time.Hour)
	fake.emit(muteEvent(ada, true, end.UnixMilli()))
	if c := storedChat(t, db, ada); !c.MutedUntil.Equal(end) {
		t.Fatalf("after a mute until %v = %+v", end, c)
	}
	fake.emit(muteEvent(ada, false, 0))
	if c := storedChat(t, db, ada); !c.MutedUntil.IsZero() {
		t.Fatalf("after an unmute = %+v, want not muted", c)
	}
}

func TestChatMarkedReadElsewhereClearsUnreadPublishesItAndSendsNoReceipt(t *testing.T) {
	_, fake, events := subscribedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))
	events()

	fake.emit(markReadEvent(ada, true))
	assertEvents(t, events(), `{"v":1,"op":"event_unread","unread":1,"jid":"15551230001@s.whatsapp.net","chat_unread":0}`)
	// Read receipts only on an explicit view: a read state that came from the phone sends nothing.
	if fake.markReads != 0 {
		t.Errorf("MarkRead was sent %d times, want 0", fake.markReads)
	}
}

func TestChatMarkedReadElsewhereClearsTheStoredUnread(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(textEvent(ada, "A2", false, t0.Add(time.Second), "two"))

	fake.emit(markReadEvent(ada, true))
	if n := unread(t, db, ada.String()); n != 0 {
		t.Errorf("unread after the phone marked the chat read = %d, want 0", n)
	}
}

// Marked unread on the phone: nuncio has no "marked unread" flag, so the counts stay as they are
// (unread messages stay unread, read ones stay read) and nothing is published.
func TestChatMarkedUnreadElsewhereLeavesTheCountsAlone(t *testing.T) {
	s, fake, events := subscribedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(textEvent(ada, "A2", false, t0.Add(time.Second), "two"))
	fake.emit(textEvent(grace, "G1", false, t0, "hello"))
	if err := s.MarkChatRead(grace.String()); err != nil {
		t.Fatalf("MarkChatRead: %v", err)
	}
	events()

	fake.emit(markReadEvent(ada, false))
	fake.emit(markReadEvent(grace, false))
	assertEvents(t, events())
	if n := unread(t, s.Store(), ada.String()); n != 2 {
		t.Errorf("unread of ada after the phone marked it unread = %d, want it unchanged at 2", n)
	}
	if n := unread(t, s.Store(), grace.String()); n != 0 {
		t.Errorf("unread of grace after the phone marked it unread = %d, want it unchanged at 0", n)
	}
}

func TestAppStateForAnUnknownChatCreatesItWithTheFlag(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(pinEvent(ada, true))
	fake.emit(archiveEvent(grace, true))
	group := types.NewJID("120363000000000000", types.GroupServer)
	fake.emit(muteEvent(group, true, -1))

	if c := storedChat(t, db, ada); !c.Pinned {
		t.Errorf("new pinned chat = %+v", c)
	}
	if c := storedChat(t, db, grace); !c.Archived {
		t.Errorf("new archived chat = %+v", c)
	}
	if c := storedChat(t, db, group); !c.MutedUntil.Equal(store.MutedForever) {
		t.Errorf("new muted chat = %+v", c)
	}
}

func TestAppStateForAFlagKeepsTheChatName(t *testing.T) {
	_, fake, db := startedSession(t)
	if err := store.UpsertChat(db, store.Chat{JID: ada.String(), Name: "Ada"}); err != nil {
		t.Fatalf("UpsertChat: %v", err)
	}
	fake.emit(pinEvent(ada, true))
	fake.emit(archiveEvent(ada, true))
	fake.emit(muteEvent(ada, true, -1))
	if c := storedChat(t, db, ada); c.Name != "Ada" || !c.Pinned || !c.Archived {
		t.Errorf("chat after its flags = %+v, want Ada pinned and archived", c)
	}
}

func TestAppStateForAnInvalidChatIsIgnored(t *testing.T) {
	_, fake, db := startedSession(t)
	for _, jid := range []types.JID{
		types.NewJID("status", types.BroadcastServer),
		types.NewJID("123", types.NewsletterServer),
		{},
	} {
		fake.emit(pinEvent(jid, true))
		fake.emit(archiveEvent(jid, true))
		fake.emit(muteEvent(jid, true, -1))
		fake.emit(markReadEvent(jid, true))
	}
	if chats, err := store.ListChats(db, 200); err != nil || len(chats) != 0 {
		t.Errorf("chats after app state for invalid jids = %+v, %v; want none", chats, err)
	}
}

func TestAppStateWithoutAnActionChangesNothing(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "A1", false, t0, "one"))
	fake.emit(pinEvent(ada, true))
	fake.emit(archiveEvent(ada, true))
	fake.emit(muteEvent(ada, true, -1))

	fake.emit(&waevents.Pin{JID: ada})
	fake.emit(&waevents.Archive{JID: ada})
	fake.emit(&waevents.Mute{JID: ada})
	fake.emit(&waevents.MarkChatAsRead{JID: ada})
	c := storedChat(t, db, ada)
	if !c.Pinned || !c.Archived || !c.MutedUntil.Equal(store.MutedForever) || c.Unread != 1 {
		t.Errorf("chat after app state with no action = %+v, want every flag and the unread kept", c)
	}
}

// The initial full sync is applied like any later change.
func TestAppStateFromTheFullSyncIsApplied(t *testing.T) {
	_, fake, db := startedSession(t)
	pin := pinEvent(ada, true)
	pin.FromFullSync = true
	fake.emit(pin)
	if c := storedChat(t, db, ada); !c.Pinned {
		t.Errorf("chat after a full-sync pin = %+v, want pinned", c)
	}
}

func TestSessionWithoutStoreIgnoresAppState(t *testing.T) {
	fake := &fakeClient{device: true}
	s := NewSession(fake, nil, nil)
	if err := s.Start(); err != nil {
		t.Fatalf("Start: %v", err)
	}
	// None of these may panic.
	fake.emit(pinEvent(ada, true))
	fake.emit(archiveEvent(ada, true))
	fake.emit(muteEvent(ada, true, -1))
	fake.emit(markReadEvent(ada, true))
}
