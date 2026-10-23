package store

import (
	"reflect"
	"testing"
	"time"
)

func TestAReactionStoredBeforeItsTargetIsReadWithIt(t *testing.T) {
	db := openStore(t)
	if err := SetReaction(db, adaJID, "late", graceJID, false, "🎉"); err != nil {
		t.Fatalf("SetReaction: %v", err)
	}
	insert(t, db, Message{ID: "late", ChatJID: adaJID, Kind: KindText, Text: "arrived later", Timestamp: base})

	want := []Reaction{{Emoji: "🎉"}}
	got, err := MessageReactions(db, adaJID, "late")
	if err != nil {
		t.Fatalf("MessageReactions: %v", err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("MessageReactions = %+v, want %+v", got, want)
	}
	if m, _ := GetMessage(db, adaJID, "late"); !reflect.DeepEqual(m.Reactions, want) {
		t.Errorf("GetMessage reactions = %+v, want %+v", m.Reactions, want)
	}
	if msgs, _ := ListMessages(db, adaJID, 10); len(msgs) != 1 || !reflect.DeepEqual(msgs[0].Reactions, want) {
		t.Errorf("ListMessages = %+v, want late with %+v", msgs, want)
	}
}

func TestGetMessageCarriesExactlyItsOwnReactions(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "one", Timestamp: base})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "two", Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "a1", ChatJID: graceJID, Kind: KindText, Text: "same id, other chat", Timestamp: base})
	react := func(chat, id, sender string, fromMe bool, emoji string) {
		t.Helper()
		if err := SetReaction(db, chat, id, sender, fromMe, emoji); err != nil {
			t.Fatalf("SetReaction: %v", err)
		}
	}
	react(adaJID, "a1", graceJID, false, "👍")
	react(adaJID, "a1", adaJID, false, "😂")
	react(adaJID, "a1", "", true, "❤️")
	react(adaJID, "a2", adaJID, false, "😮")
	react(graceJID, "a1", graceJID, false, "🙏")

	got, err := GetMessage(db, adaJID, "a1")
	if err != nil {
		t.Fatalf("GetMessage: %v", err)
	}
	// Mine first, then the others by sender: the order ListMessages uses too.
	want := []Reaction{{Emoji: "❤️", FromMe: true}, {Emoji: "😂"}, {Emoji: "👍"}}
	if !reflect.DeepEqual(got.Reactions, want) {
		t.Errorf("reactions of a1 = %+v, want %+v", got.Reactions, want)
	}
	msgs, _ := ListMessages(db, adaJID, 10)
	if len(msgs) != 2 || !reflect.DeepEqual(msgs[1].Reactions, got.Reactions) {
		t.Errorf("ListMessages a1 reactions = %+v, want the same as GetMessage", msgs)
	}
	if got, _ := GetMessage(db, adaJID, "a2"); !reflect.DeepEqual(got.Reactions, []Reaction{{Emoji: "😮"}}) {
		t.Errorf("reactions of a2 = %+v, want only its own", got.Reactions)
	}
	if got, _ := GetMessage(db, graceJID, "a1"); !reflect.DeepEqual(got.Reactions, []Reaction{{Emoji: "🙏"}}) {
		t.Errorf("reactions of the other chat's a1 = %+v, want only its own", got.Reactions)
	}
}

func TestSearchChatFindsEmojiAndPunctuationTheIndexDoesNotTokenise(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "party 🎉", Timestamp: base})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "no emoji here", Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "a3", ChatJID: adaJID, Kind: KindText, Text: "🎉🎉 again?!", Timestamp: base.Add(2 * time.Minute)})
	insert(t, db, Message{ID: "a4", ChatJID: adaJID, Kind: KindImage, Text: "cake 🎉", Mime: "image/jpeg", Size: 9, Timestamp: base.Add(3 * time.Minute)})
	insert(t, db, Message{ID: "g1", ChatJID: graceJID, Kind: KindText, Text: "🎉", Timestamp: base.Add(4 * time.Minute)})
	insert(t, db, Message{ID: "a5", ChatJID: adaJID, Kind: KindText, Text: "100% sure_thing", Timestamp: base.Add(5 * time.Minute)})
	if err := SetReaction(db, adaJID, "a3", graceJID, false, "👍"); err != nil {
		t.Fatalf("SetReaction: %v", err)
	}

	ids := func(query string, limit int) []string {
		t.Helper()
		got, err := SearchChat(db, adaJID, query, limit)
		if err != nil {
			t.Fatalf("SearchChat(%q): %v", query, err)
		}
		var out []string
		for _, m := range got {
			out = append(out, m.ID)
		}
		return out
	}
	if got := ids("🎉", 10); !reflect.DeepEqual(got, []string{"a4", "a3", "a1"}) {
		t.Errorf("SearchChat 🎉 = %v, want [a4 a3 a1] (that chat only, newest first)", got)
	}
	if got := ids("🎉", 2); !reflect.DeepEqual(got, []string{"a4", "a3"}) {
		t.Errorf("SearchChat 🎉 limit 2 = %v, want [a4 a3]", got)
	}
	if got := ids("?!", 10); !reflect.DeepEqual(got, []string{"a3"}) {
		t.Errorf("SearchChat ?! = %v, want [a3]", got)
	}
	// LIKE wildcards in the query are literal characters, not patterns.
	if got := ids("%", 10); !reflect.DeepEqual(got, []string{"a5"}) {
		t.Errorf("SearchChat %% = %v, want [a5]", got)
	}
	if got := ids("_", 10); !reflect.DeepEqual(got, []string{"a5"}) {
		t.Errorf("SearchChat _ = %v, want [a5]", got)
	}
	// Every word must match, as with the index.
	if got := ids("🎉 ?!", 10); !reflect.DeepEqual(got, []string{"a3"}) {
		t.Errorf("SearchChat '🎉 ?!' = %v, want [a3]", got)
	}
	got, _ := SearchChat(db, adaJID, "🎉", 10)
	if len(got) != 3 || got[0].Kind != KindImage || got[0].Mime != "image/jpeg" ||
		!reflect.DeepEqual(got[1].Reactions, []Reaction{{Emoji: "👍"}}) {
		t.Errorf("SearchChat 🎉 hits = %+v, want each with its media and reactions", got)
	}
}

func TestSearchChatWithWordsStillUsesTheIndex(t *testing.T) {
	db := openStore(t)
	insert(t, db, Message{ID: "a1", ChatJID: adaJID, Kind: KindText, Text: "Lunch at noon", Timestamp: base})
	insert(t, db, Message{ID: "a2", ChatJID: adaJID, Kind: KindText, Text: "lunchbox", Timestamp: base.Add(time.Minute)})
	insert(t, db, Message{ID: "a3", ChatJID: adaJID, Kind: KindText, Text: "café LUNCH", Timestamp: base.Add(2 * time.Minute)})

	got, err := SearchChat(db, adaJID, "lunch", 10)
	if err != nil {
		t.Fatalf("SearchChat: %v", err)
	}
	var ids []string
	for _, m := range got {
		ids = append(ids, m.ID)
	}
	// Whole words, any case: a substring match would also find lunchbox.
	if !reflect.DeepEqual(ids, []string{"a3", "a1"}) {
		t.Errorf("SearchChat lunch = %v, want [a3 a1]", ids)
	}
}
