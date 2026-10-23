package store

import (
	"reflect"
	"testing"
)

// LastMessage is a chat's newest message as ListMessages lists it, without its media reference, and
// false for a chat without messages.
func TestLastMessageIsTheNewestWithoutItsMediaReference(t *testing.T) {
	db := openStore(t)
	if _, ok, err := LastMessage(db, chatAlice); err != nil || ok {
		t.Fatalf("LastMessage of an unknown chat = %v, %v; want none", ok, err)
	}
	for _, m := range []Message{
		{ID: "OLD", ChatJID: chatAlice, Kind: KindText, Text: "old", Timestamp: t0},
		{ID: "IMG", ChatJID: chatAlice, Kind: KindImage, Text: "caption", Mime: "image/jpeg", Size: 3, Timestamp: t0.Add(1),
			MediaRef: []byte{1, 2, 3}},
	} {
		if _, err := InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	if err := SetReaction(db, chatAlice, "IMG", "", true, "👍"); err != nil {
		t.Fatal(err)
	}
	listed, err := ListMessages(db, chatAlice, 1)
	if err != nil || len(listed) != 1 || len(listed[0].MediaRef) == 0 {
		t.Fatalf("ListMessages = %+v, %v; want the image with its reference", listed, err)
	}
	last, ok, err := LastMessage(db, chatAlice)
	if err != nil || !ok {
		t.Fatalf("LastMessage = %v, %v", ok, err)
	}
	if last.MediaRef != nil {
		t.Errorf("LastMessage read the media reference (%d bytes), want none", len(last.MediaRef))
	}
	listed[0].MediaRef = nil
	if !reflect.DeepEqual(last, listed[0]) {
		t.Errorf("LastMessage = %+v, want %+v", last, listed[0])
	}
}
