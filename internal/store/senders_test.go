package store

import (
	"testing"
	"time"
)

// messages and search read each message's recorded sender, so the wire can name the author of a
// group message; a message stored before senders were recorded (M1, no message_meta row) and mine
// have none. Both search paths (the index and the substring match) carry it.
func TestListedAndFoundMessagesCarryTheirRecordedSender(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "g1", ChatJID: groupJID, Kind: KindText, Text: "rope at the crag 🧗", Timestamp: base, Sender: adaJID})
	insert(t, db, Message{ID: "g2", ChatJID: groupJID, Kind: KindText, Text: "rope is mine", Timestamp: base.Add(time.Minute), FromMe: true})
	insert(t, db, Message{ID: "g0", ChatJID: groupJID, Kind: KindText, Text: "old rope", Timestamp: base.Add(-time.Minute)})
	want := map[string]string{"g1": adaJID, "g2": "", "g0": ""}

	listed, err := ListMessages(db, groupJID, 10)
	if err != nil {
		t.Fatal(err)
	}
	found, err := SearchChat(db, groupJID, "rope", 10)
	if err != nil {
		t.Fatal(err)
	}
	climber, err := SearchChat(db, groupJID, "🧗", 10)
	if err != nil {
		t.Fatal(err)
	}
	for name, tc := range map[string]struct {
		got []Message
		n   int
	}{"ListMessages": {listed, 3}, "SearchChat": {found, 3}, "SearchChat by substring": {climber, 1}} {
		if len(tc.got) != tc.n {
			t.Fatalf("%s: %d messages, want %d", name, len(tc.got), tc.n)
		}
		for _, m := range tc.got {
			if m.Sender != want[m.ID] {
				t.Errorf("%s: %s sender = %q, want %q", name, m.ID, m.Sender, want[m.ID])
			}
		}
	}
}
