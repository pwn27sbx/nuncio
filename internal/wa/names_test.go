package wa

import (
	"context"
	"database/sql"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// omarchy is a group the account is in: fake.
var omarchy = types.NewJID("120363000000000000", types.GroupServer)

// nameClient is lidClient with whatsmeow's contact store (Store.Contacts) and its joined-groups
// query: contacts by user JID, the subjects GetJoinedGroups answers, and how often it was asked.
type nameClient struct {
	*lidClient
	nmu         sync.Mutex
	contacts    map[string]Contact
	contactErr  error
	groups      map[string]string
	groupsErr   error
	joinedCalls int
}

func newNameClient(pairs ...string) *nameClient {
	return &nameClient{lidClient: newLIDClient(pairs...), contacts: map[string]Contact{}, groups: map[string]string{}}
}

func (c *nameClient) Contact(_ context.Context, jid string) (Contact, error) {
	c.nmu.Lock()
	defer c.nmu.Unlock()
	if c.contactErr != nil {
		return Contact{}, c.contactErr
	}
	return c.contacts[jid], nil
}

func (c *nameClient) JoinedGroups(context.Context) (map[string]string, error) {
	c.nmu.Lock()
	defer c.nmu.Unlock()
	c.joinedCalls++
	if c.groupsErr != nil {
		return nil, c.groupsErr
	}
	out := map[string]string{}
	for k, v := range c.groups {
		out[k] = v
	}
	return out, nil
}

func (c *nameClient) setContact(jid string, ct Contact) {
	c.nmu.Lock()
	c.contacts[jid] = ct
	c.nmu.Unlock()
}

func (c *nameClient) joined() int {
	c.nmu.Lock()
	defer c.nmu.Unlock()
	return c.joinedCalls
}

func chatNameOf(t *testing.T, db *sql.DB, jid string) string {
	t.Helper()
	name, err := store.ChatName(db, jid)
	if err != nil {
		t.Fatalf("ChatName(%s): %v", jid, err)
	}
	return name
}

// storedMessage stores one received text message in chat before the session starts.
func storedMessage(t *testing.T, db *sql.DB, chat, id string) {
	t.Helper()
	if _, err := store.InsertMessage(db, store.Message{ID: id, ChatJID: chat, Kind: store.KindText, Text: "hi",
		Timestamp: t0}); err != nil {
		t.Fatal(err)
	}
}

// waitFor polls cond for up to 2 s: the joined-groups query runs off the event goroutine.
func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

// A direct chat is named, at Start, from its contact: the address-book name (full, else first),
// else the business name, else the push name; no name at all leaves it empty.
func TestADirectChatIsNamedFromItsContactInPriorityOrder(t *testing.T) {
	full := Contact{FullName: "Ada Lovelace", FirstName: "Ada", BusinessName: "Engines Ltd", PushName: "ada"}
	for _, tc := range []struct {
		name    string
		contact Contact
		want    string
	}{
		{"full name", full, "Ada Lovelace"},
		{"first name", Contact{FirstName: "Ada", BusinessName: "Engines Ltd", PushName: "ada"}, "Ada"},
		{"business name", Contact{BusinessName: "Engines Ltd", PushName: "ada"}, "Engines Ltd"},
		{"push name", Contact{PushName: "ada"}, "ada"},
		{"nothing", Contact{}, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			c := newNameClient()
			c.setContact(ada.String(), tc.contact)
			db := openTestStore(t)
			storedMessage(t, db, ada.String(), "A1")
			lidSession(t, c, db)
			if got := chatNameOf(t, db, ada.String()); got != tc.want {
				t.Errorf("name = %q, want %q", got, tc.want)
			}
		})
	}
}

// A LID chat also reads the contact stored under its phone number: the address book holds Ada under
// her number, so her full name there beats the push name WhatsApp gave her LID.
func TestALIDChatIsNamedThroughItsPhoneNumber(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	c.setContact(adaLID.String(), Contact{PushName: "ada"})
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	lidSession(t, c, db)
	if got := chatNameOf(t, db, adaLID.String()); got != "Ada Lovelace" {
		t.Errorf("name = %q, want the address-book name under her phone number", got)
	}
}

// A LID chat whose number whatsmeow does not know is named from the LID's own contact.
func TestALIDChatWithoutItsNumberUsesItsOwnContact(t *testing.T) {
	c := newNameClient()
	c.setContact(adaLID.String(), Contact{PushName: "ada"})
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	lidSession(t, c, db)
	if got := chatNameOf(t, db, adaLID.String()); got != "ada" {
		t.Errorf("name = %q, want the LID's push name", got)
	}
}

// A push name, business name or address-book change renames the stored chat and publishes it as
// event_chat; the same name again publishes nothing, and a contact with no chat creates none.
func TestAContactNameChangeRenamesTheChatAndPublishesIt(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	_, events, _ := lidSession(t, c, db)

	c.setContact(adaLID.String(), Contact{PushName: "ada"})
	c.emit(&waevents.PushName{JID: adaLID, JIDAlt: ada, NewPushName: "ada"})
	got := events()
	if len(got) != 1 || !strings.Contains(got[0], `"op":"event_chat"`) || !strings.Contains(got[0], `"name":"ada"`) {
		t.Fatalf("events after a push name = %q, want one event_chat named ada", got)
	}
	c.emit(&waevents.PushName{JID: adaLID, JIDAlt: ada, NewPushName: "ada"})
	if got := events(); len(got) != 0 {
		t.Errorf("events after the same push name = %q, want none", got)
	}

	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	c.emit(&waevents.Contact{JID: ada, Action: &waSyncAction.ContactAction{FullName: proto.String("Ada Lovelace")}})
	if got := events(); len(got) != 1 || !strings.Contains(got[0], `"name":"Ada Lovelace"`) {
		t.Errorf("events after an address-book change = %q, want one event_chat named Ada Lovelace", got)
	}

	c.setContact(adaLID.String(), Contact{PushName: "ada", BusinessName: "Engines Ltd"})
	c.emit(&waevents.BusinessName{JID: adaLID, NewBusinessName: "Engines Ltd"})
	if got := events(); len(got) != 0 {
		t.Errorf("events after a business name below the address-book name = %q, want none", got)
	}

	c.setContact(grace.String(), Contact{FullName: "Grace Hopper"})
	c.emit(&waevents.Contact{JID: grace, Action: &waSyncAction.ContactAction{FullName: proto.String("Grace Hopper")}})
	if got := events(); len(got) != 0 {
		t.Errorf("events for a contact with no chat = %q, want none", got)
	}
	if _, err := store.ChatName(db, grace.String()); !errors.Is(err, store.ErrNoChat) {
		t.Errorf("a contact with no chat created one: %v", err)
	}
}

// A chat created by its first message is named at once, after the message's own events.
func TestANewChatIsNamedWithItsFirstMessage(t *testing.T) {
	c := newNameClient()
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	db := openTestStore(t)
	_, events, _ := lidSession(t, c, db)
	c.emit(textEvent(ada, "A1", false, t0, "hello"))
	got := events()
	if len(got) != 3 || !strings.Contains(got[2], `"op":"event_chat"`) || !strings.Contains(got[2], `"name":"Ada Lovelace"`) {
		t.Fatalf("events = %q, want event_message, event_unread, then event_chat named Ada Lovelace", got)
	}
	c.emit(textEvent(ada, "A2", false, t0.Add(time.Second), "again"))
	if got := events(); len(got) != 2 {
		t.Errorf("events of a second message = %q, want no further event_chat", got)
	}
}

// Group subjects come from one joined-groups query per Start (never one per message or reconnect),
// and a subject change renames the group and publishes it as event_chat.
func TestAGroupIsNamedFromItsSubjectWithOneQueryPerStart(t *testing.T) {
	c := newNameClient()
	c.groups[omarchy.String()] = "Omarchy devs"
	db := openTestStore(t)
	storedMessage(t, db, omarchy.String(), "G1")
	_, events, _ := lidSession(t, c, db)
	var got []string
	waitFor(t, "the group's event_chat", func() bool { got = append(got, events()...); return len(got) > 0 })
	if len(got) != 1 || !strings.Contains(got[0], `"name":"Omarchy devs"`) {
		t.Errorf("events = %q, want one event_chat named Omarchy devs", got)
	}
	if n := chatNameOf(t, db, omarchy.String()); n != "Omarchy devs" {
		t.Errorf("stored name = %q, want the subject", n)
	}

	for i, id := range []string{"G2", "G3", "G4"} {
		evt := textEvent(omarchy, id, false, t0.Add(time.Duration(i)*time.Second), "hi")
		evt.Info.IsGroup = true
		evt.Info.Sender = grace
		c.emit(evt)
	}
	c.emit(&waevents.Disconnected{})
	c.emit(&waevents.Connected{})
	if n := c.joined(); n != 1 {
		t.Errorf("joined-groups queries = %d, want 1 per Start", n)
	}
	events()

	c.emit(&waevents.GroupInfo{JID: omarchy, Name: &types.GroupName{Name: "Omarchy core"}})
	got = events()
	if len(got) != 1 || !strings.Contains(got[0], `"op":"event_chat"`) || !strings.Contains(got[0], `"name":"Omarchy core"`) {
		t.Fatalf("events after a subject change = %q, want one event_chat named Omarchy core", got)
	}
	if n := chatNameOf(t, db, omarchy.String()); n != "Omarchy core" {
		t.Errorf("stored name = %q", n)
	}
	c.emit(&waevents.GroupInfo{JID: omarchy, Topic: &types.GroupTopic{Topic: "x"}})
	if got := events(); len(got) != 0 {
		t.Errorf("events after a change with no subject = %q, want none", got)
	}
	if n := chatNameOf(t, db, omarchy.String()); n != "Omarchy core" {
		t.Errorf("stored name after a topic change = %q", n)
	}
}

// A group joined later is named from JoinedGroup when its first message arrives.
func TestAGroupJoinedLaterIsNamedWithItsFirstMessage(t *testing.T) {
	c := newNameClient()
	db := openTestStore(t)
	_, events, _ := lidSession(t, c, db)
	waitFor(t, "the joined-groups query", func() bool { return c.joined() == 1 })
	joined := &waevents.JoinedGroup{}
	joined.JID = omarchy
	joined.Name = "Omarchy devs"
	c.emit(joined)
	if got := events(); len(got) != 0 {
		t.Errorf("events for a group with no chat = %q, want none", got)
	}
	evt := textEvent(omarchy, "G1", false, t0, "welcome")
	evt.Info.IsGroup = true
	evt.Info.Sender = grace
	c.emit(evt)
	if n := chatNameOf(t, db, omarchy.String()); n != "Omarchy devs" {
		t.Errorf("name = %q, want the subject", n)
	}
}

// A contact with no name falls back to her masked phone number, through the LID map for a LID chat;
// a LID chat whose number is unknown and a group have none. The LID digits are never the fallback.
func TestAChatPhoneIsTheMaskedNumberNeverTheLID(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	s, events, _ := lidSession(t, c, db)
	unknownLID := types.NewJID("100000000000009", types.HiddenUserServer).String()
	for _, tc := range []struct{ jid, want string }{
		{adaLID.String(), "…0001"},
		{grace.String(), "…0002"},
		{unknownLID, ""},
		{omarchy.String(), ""},
	} {
		if got := s.ChatPhone(tc.jid); got != tc.want {
			t.Errorf("ChatPhone(%s) = %q, want %q", tc.jid, got, tc.want)
		}
	}
	s.publishChat(adaLID.String())
	got := events()
	if len(got) != 1 || !strings.Contains(got[0], `"name":""`) || !strings.Contains(got[0], `"phone":"…0001"`) {
		t.Fatalf("event_chat = %q, want no name and the masked phone", got)
	}
	if strings.Count(got[0], "100000000000001") != 1 || strings.Contains(got[0], "15551230001") {
		t.Errorf("event_chat = %s: the LID digits only in jid, the number never in full", got[0])
	}
}

// A failed contact read or joined-groups query keeps the names stored, and the logs carry the
// error's type only: no name, number, LID or group id.
func TestNameFailuresKeepTheNamesAndLogNoPersonalData(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	c.groups[omarchy.String()] = "Omarchy devs"
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	storedMessage(t, db, omarchy.String(), "G1")
	s, _, logs := lidSession(t, c, db)
	waitFor(t, "the group subject", func() bool { n, _ := store.ChatName(db, omarchy.String()); return n != "" })

	c.nmu.Lock()
	c.contactErr = errors.New("contact store for 15551230001 Ada Lovelace failed")
	c.groupsErr = errors.New("query for 120363000000000000 failed")
	c.nmu.Unlock()
	c.emit(&waevents.PushName{JID: adaLID, JIDAlt: ada, NewPushName: "ada"})
	if n := chatNameOf(t, db, adaLID.String()); n != "Ada Lovelace" {
		t.Errorf("name after a failed read = %q, want it kept", n)
	}
	s.syncGroupNames()
	if n := chatNameOf(t, db, omarchy.String()); n != "Omarchy devs" {
		t.Errorf("group name after a failed query = %q, want it kept", n)
	}
	all := strings.Join(*logs, "\n")
	if all == "" {
		t.Fatal("the failures were not logged")
	}
	for _, secret := range []string{"Ada", "Omarchy", "15551230001", "100000000000001", "120363000000000000", "0001"} {
		if strings.Contains(all, secret) {
			t.Errorf("logs carry %q:\n%s", secret, all)
		}
	}
}

// A live merge of the number's chat into the LID chat renames the LID chat from the number's
// address-book entry, and publishes it once, with its aliases and masked phone.
func TestALiveMergeRenamesTheLIDChatFromItsNumbersAddressBookOnce(t *testing.T) {
	c := newNameClient()
	c.setContact(adaLID.String(), Contact{PushName: "ada"})
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	db := openTestStore(t)
	storedMessage(t, db, adaLID.String(), "L1")
	storedMessage(t, db, ada.String(), "P1")
	_, events, _ := lidSession(t, c, db)
	c.learn(ada.String(), adaLID.String())
	c.emit(textEvent(ada, "P2", false, t0.Add(time.Minute), "via number"))
	var chats []string
	for _, f := range events() {
		if strings.Contains(f, `"op":"event_chat"`) {
			chats = append(chats, f)
		}
	}
	if len(chats) != 1 || !strings.Contains(chats[0], `"name":"Ada Lovelace"`) ||
		!strings.Contains(chats[0], `"aliases":["15551230001@s.whatsapp.net"]`) || !strings.Contains(chats[0], `"phone":"…0001"`) {
		t.Fatalf("event_chat frames = %q, want one, named Ada Lovelace, with aliases and the masked phone", chats)
	}
	if n := chatNameOf(t, db, adaLID.String()); n != "Ada Lovelace" {
		t.Errorf("stored name = %q", n)
	}
}
