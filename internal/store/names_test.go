package store

import (
	"errors"
	"reflect"
	"testing"
	"time"
)

// SetChatName names a stored chat and reports whether the name changed; it never creates a chat,
// so a contact or group with nothing stored stays out of the chat list.
func TestSetChatNameNamesAStoredChatOnlyAndReportsAChange(t *testing.T) {
	db := openStore(t)
	if err := UpsertChat(db, Chat{JID: "15551230001@s.whatsapp.net", Pinned: true}); err != nil {
		t.Fatal(err)
	}
	changed, err := SetChatName(db, "15551230001@s.whatsapp.net", "Ada Lovelace")
	if err != nil || !changed {
		t.Fatalf("SetChatName = %v, %v; want a change", changed, err)
	}
	c, err := GetChat(db, "15551230001@s.whatsapp.net")
	if err != nil || c.Name != "Ada Lovelace" || !c.Pinned {
		t.Errorf("chat = %+v, %v; want the name set and the pin kept", c, err)
	}
	if changed, err := SetChatName(db, "15551230001@s.whatsapp.net", "Ada Lovelace"); err != nil || changed {
		t.Errorf("the same name again = %v, %v; want no change", changed, err)
	}
	if changed, err := SetChatName(db, "15551230009@s.whatsapp.net", "Nobody"); err != nil || changed {
		t.Errorf("a chat not stored = %v, %v; want no change", changed, err)
	}
	if _, err := GetChat(db, "15551230009@s.whatsapp.net"); !errors.Is(err, ErrNoChat) {
		t.Errorf("SetChatName created a chat: %v", err)
	}
}

func TestChatNameAndChatJIDs(t *testing.T) {
	db := openStore(t)
	if _, err := InsertMessage(db, Message{ID: "A1", ChatJID: "100000000000001@lid", Kind: KindText,
		Text: "hi", Timestamp: time.Unix(1, 0)}); err != nil {
		t.Fatal(err)
	}
	if err := UpsertChat(db, Chat{JID: "120363000000000000@g.us", Name: "Omarchy devs"}); err != nil {
		t.Fatal(err)
	}
	if name, err := ChatName(db, "120363000000000000@g.us"); err != nil || name != "Omarchy devs" {
		t.Errorf("ChatName = %q, %v", name, err)
	}
	if name, err := ChatName(db, "100000000000001@lid"); err != nil || name != "" {
		t.Errorf("ChatName of an unnamed chat = %q, %v", name, err)
	}
	if _, err := ChatName(db, "15551230009@s.whatsapp.net"); !errors.Is(err, ErrNoChat) {
		t.Errorf("ChatName of a chat not stored: %v, want ErrNoChat", err)
	}
	jids, err := ChatJIDs(db)
	if want := []string{"100000000000001@lid", "120363000000000000@g.us"}; err != nil || !reflect.DeepEqual(jids, want) {
		t.Errorf("ChatJIDs = %v, %v; want %v", jids, err, want)
	}
}
