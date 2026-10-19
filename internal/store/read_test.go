package store

import (
	"reflect"
	"testing"
	"time"
)

const groupJID = "120363000000000000@g.us"

func TestUnreadIncomingListsOnlyThatChatsUnreadMessagesFromOthersOldestFirst(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "g2", ChatJID: groupJID, Kind: KindText, Text: "b", Sender: graceJID, Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "g1", ChatJID: groupJID, Kind: KindText, Text: "a", Sender: adaJID, Timestamp: base})
	insert(t, db, Message{ID: "old", ChatJID: groupJID, Kind: KindText, Text: "m1 row", Timestamp: base.Add(-time.Minute)})
	insert(t, db, Message{ID: "mine", ChatJID: groupJID, Kind: KindText, Text: "x", FromMe: true, Timestamp: base})
	insert(t, db, Message{ID: "seen", ChatJID: groupJID, Kind: KindText, Text: "y", Sender: adaJID, Read: true, Timestamp: base})
	insert(t, db, Message{ID: "other", ChatJID: adaJID, Kind: KindText, Text: "z", Sender: adaJID, Timestamp: base})

	got, err := UnreadIncoming(db, groupJID)
	if err != nil {
		t.Fatalf("UnreadIncoming: %v", err)
	}
	var ids, senders []string
	for _, m := range got {
		ids = append(ids, m.ID)
		senders = append(senders, m.Sender)
	}
	if want := []string{"old", "g1", "g2"}; !reflect.DeepEqual(ids, want) {
		t.Errorf("ids = %q, want %q", ids, want)
	}
	if want := []string{"", adaJID, graceJID}; !reflect.DeepEqual(senders, want) {
		t.Errorf("senders = %q, want %q", senders, want)
	}
	if !got[1].Timestamp.Equal(base) {
		t.Errorf("timestamp of g1 = %v, want %v", got[1].Timestamp, base)
	}
}

func TestMarkMessagesReadMarksExactlyThoseIDsOfThatChat(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "a", Timestamp: base})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "b", Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "a1", ChatJID: graceJID, Kind: KindText, Text: "c", Timestamp: base})

	if err := MarkMessagesRead(db, adaJID, []string{"a1"}); err != nil {
		t.Fatalf("MarkMessagesRead: %v", err)
	}
	if n, err := UnreadCount(db, adaJID); err != nil || n != 1 {
		t.Errorf("unread of ada = %d, %v; want 1 (a2 arrived unread and stays so)", n, err)
	}
	if n, err := UnreadCount(db, graceJID); err != nil || n != 1 {
		t.Errorf("unread of grace = %d, %v; want 1 (same id, other chat)", n, err)
	}
}
