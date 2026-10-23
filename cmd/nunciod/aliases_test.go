package main

import (
	"reflect"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// The daemon resolves a chat for ServeConn (ADR 0001).
var _ ipc.ChatResolver = (*daemon)(nil)

// aliasSession is fixedSession whose CanonicalJID maps one phone-number JID to its LID chat.
type aliasSession struct{ fixedSession }

func (aliasSession) CanonicalJID(jid string) string {
	if jid == "15551230001@s.whatsapp.net" {
		return "100000000000001@lid"
	}
	return jid
}

func TestDaemonServesAPhoneNumberAliasAsItsLIDChat(t *testing.T) {
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	if _, err := store.InsertMessage(db, store.Message{ID: "L1", ChatJID: "100000000000001@lid", Kind: store.KindText,
		Text: "the venue", Timestamp: t0}); err != nil {
		t.Fatal(err)
	}
	d, err := newDaemon(aliasSession{fixedSession(wa.StateLinked)}, db)
	if err != nil {
		t.Fatal(err)
	}
	if got := d.CanonicalJID("15551230001@s.whatsapp.net"); got != "100000000000001@lid" {
		t.Errorf("CanonicalJID = %s, want the LID chat", got)
	}
	plain, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	if got := plain.CanonicalJID("15551230001@s.whatsapp.net"); got != "15551230001@s.whatsapp.net" {
		t.Errorf("CanonicalJID without a resolving session = %s, want it unchanged", got)
	}
	if got := d.CanonicalJID("15551230002@s.whatsapp.net"); got != "15551230002@s.whatsapp.net" {
		t.Errorf("CanonicalJID of an unmapped number = %s", got)
	}
}

// ChatAliases is aliasSession's: the phone number merged into Ada's LID chat.
func (aliasSession) ChatAliases(jid string) []string {
	if jid == "100000000000001@lid" {
		return []string{"15551230001@s.whatsapp.net"}
	}
	return nil
}

// `chats` lists a chat with the phone numbers the session says were merged into it, and nothing
// for a session that has no aliases.
func TestDaemonListsTheAliasesOfAMergedChat(t *testing.T) {
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	for _, m := range []store.Message{
		{ID: "L1", ChatJID: "100000000000001@lid", Kind: store.KindText, Text: "the venue", Timestamp: t0},
		{ID: "G1", ChatJID: "15551230002@s.whatsapp.net", Kind: store.KindText, Text: "hi", Timestamp: t0},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	d, err := newDaemon(aliasSession{fixedSession(wa.StateLinked)}, db)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string][]string{}
	for _, c := range d.Chats() {
		got[c.JID] = c.Aliases
	}
	want := map[string][]string{"100000000000001@lid": {"15551230001@s.whatsapp.net"}, "15551230002@s.whatsapp.net": nil}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("chats = %v, want %v", got, want)
	}
	plain, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range plain.Chats() {
		if c.Aliases != nil {
			t.Errorf("chat %s lists aliases without a session that has them: %q", c.JID, c.Aliases)
		}
	}
}

// ChatPhone is aliasSession's: Ada's masked number, served on her LID chat.
func (aliasSession) ChatPhone(jid string) string {
	if jid == "100000000000001@lid" {
		return "…0001"
	}
	return ""
}

// `chats` serves each chat's masked phone number from the session (the fallback a client shows for
// a chat with no name), and none from a session that has none.
func TestDaemonListsTheMaskedPhoneOfAChat(t *testing.T) {
	db := openTestStore(t)
	t0 := time.Date(2026, 10, 18, 17, 0, 0, 0, time.UTC)
	for _, m := range []store.Message{
		{ID: "L1", ChatJID: "100000000000001@lid", Kind: store.KindText, Text: "the venue", Timestamp: t0},
		{ID: "G1", ChatJID: "120363000000000000@g.us", Kind: store.KindText, Text: "hi", Timestamp: t0},
	} {
		if _, err := store.InsertMessage(db, m); err != nil {
			t.Fatal(err)
		}
	}
	d, err := newDaemon(aliasSession{fixedSession(wa.StateLinked)}, db)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]string{}
	for _, c := range d.Chats() {
		got[c.JID] = c.Phone
	}
	want := map[string]string{"100000000000001@lid": "…0001", "120363000000000000@g.us": ""}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("phones = %v, want %v", got, want)
	}
	plain, err := newDaemon(fixedSession(wa.StateLinked), db)
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range plain.Chats() {
		if c.Phone != "" {
			t.Errorf("chat %s carries a phone without a session that has one: %q", c.JID, c.Phone)
		}
	}
}
