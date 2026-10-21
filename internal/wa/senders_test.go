package wa

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	waevents "go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// Fake people of the omarchy group: one known by her number only, one by her LID only.
var (
	strangerPN  = types.NewJID("15551230009", types.DefaultUserServer)
	strangerLID = types.NewJID("100000000000009", types.HiddenUserServer)
)

// groupEvent is a text message in a group from sender.
func groupEvent(group, sender types.JID, id string, fromMe bool, ts time.Time, msg *waE2E.Message) *waevents.Message {
	return &waevents.Message{
		Info: types.MessageInfo{
			MessageSource: types.MessageSource{Chat: group, Sender: sender, IsFromMe: fromMe, IsGroup: true},
			ID:            id,
			Timestamp:     ts,
		},
		Message: msg,
	}
}

func groupText(sender types.JID, id string, fromMe bool, text string) *waevents.Message {
	return groupEvent(omarchy, sender, id, fromMe, t0, &waE2E.Message{Conversation: proto.String(text)})
}

// publishedMessage is the message of the event_message published for id.
func publishedMessage(t *testing.T, frames []string, id string) (ipc.Message, string) {
	t.Helper()
	var found *ipc.Message
	raw := ""
	for _, f := range frames {
		var ev ipc.EventMessage
		if json.Unmarshal([]byte(f), &ev) == nil && ev.Op == ipc.OpEventMessage && ev.Message.ID == id {
			found, raw = &ev.Message, f
		}
	}
	if found == nil {
		t.Fatalf("no event_message for %s in %q", id, frames)
	}
	return *found, raw
}

// An incoming group message names its author in `sender`: her name by the chat-name priority
// (address book, business, push name; a LID read through her number), else her masked number, else
// "unknown". Mine carries none. A LID's digits and a full number are never served.
func TestAnIncomingGroupMessageNamesItsSender(t *testing.T) {
	c := newNameClient(ada.String(), adaLID.String())
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	c.setContact(adaLID.String(), Contact{PushName: "ada"})
	c.setContact(grace.String(), Contact{BusinessName: "Hopper Ltd", PushName: "grace"})
	db := openTestStore(t)
	_, events, _ := lidSession(t, c, db)

	c.emit(groupText(adaLID, "G1", false, "rope?"))
	c.emit(groupText(grace, "G2", false, "mine"))
	c.emit(groupText(strangerPN, "G3", false, "hi all"))
	c.emit(groupText(strangerLID, "G4", false, "hello"))
	c.emit(groupText(ada, "G5", true, "on my way"))
	got := events()
	for _, tc := range []struct{ id, want string }{
		{"G1", "Ada Lovelace"},
		{"G2", "Hopper Ltd"},
		{"G3", "…0009"},
		{"G4", "unknown"},
		{"G5", ""},
	} {
		m, raw := publishedMessage(t, got, tc.id)
		if m.Sender != tc.want {
			t.Errorf("%s sender = %q, want %q", tc.id, m.Sender, tc.want)
		}
		if tc.want == "" && strings.Contains(raw, `"sender"`) {
			t.Errorf("%s = %s, want no sender key", tc.id, raw)
		}
		if strings.Contains(m.Sender, "100000000000009") || strings.Contains(m.Sender, "15551230009") {
			t.Errorf("%s sender = %q: a LID's digits or a full number", tc.id, m.Sender)
		}
	}
}

// A direct chat's messages and a group message stored before senders were recorded carry no
// sender, and are encoded exactly as before.
func TestADirectOrUnrecordedMessageHasNoSender(t *testing.T) {
	c := newNameClient()
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	db := openTestStore(t)
	s, events, _ := lidSession(t, c, db)
	c.emit(textEvent(ada, "D1", false, t0, "hi"))
	if m, raw := publishedMessage(t, events(), "D1"); m.Sender != "" {
		t.Errorf("direct message = %s, want no sender", raw)
	}
	storedMessage(t, db, omarchy.String(), "OLD1")
	rows, err := store.ListMessages(db, omarchy.String(), 10)
	if err != nil {
		t.Fatal(err)
	}
	if got := jsonString(t, WireMessages(omarchy.String(), rows, s.Senders())[0]); got != `{"id":"OLD1","kind":"text","text":"hi","from_me":false,"timestamp":"2026-10-18T17:55:04Z"}` {
		t.Errorf("unrecorded group message = %s, want it as before", got)
	}
}

// countingClient counts the contact reads of a nameClient.
type countingClient struct {
	*nameClient
	cmu   sync.Mutex
	reads int
}

func (c *countingClient) Contact(ctx context.Context, jid string) (Contact, error) {
	c.cmu.Lock()
	c.reads++
	c.cmu.Unlock()
	return c.nameClient.Contact(ctx, jid)
}

func (c *countingClient) count() int {
	c.cmu.Lock()
	defer c.cmu.Unlock()
	return c.reads
}

// One reply reads each sender's contact once, however many of her messages it serves.
func TestServingAGroupReadsEachSenderOnce(t *testing.T) {
	c := &countingClient{nameClient: newNameClient()}
	c.setContact(ada.String(), Contact{PushName: "ada"})
	c.setContact(grace.String(), Contact{PushName: "grace"})
	db := openTestStore(t)
	s, _, _ := lidSession(t, c, db)
	for i, from := range []types.JID{ada, ada, grace, ada, grace} {
		if _, err := store.InsertMessage(db, store.Message{ID: fmt.Sprintf("G%d", i), ChatJID: omarchy.String(),
			Kind: store.KindText, Text: "hi", Timestamp: t0.Add(time.Duration(i) * time.Second), Sender: from.String()}); err != nil {
			t.Fatal(err)
		}
	}
	rows, err := store.ListMessages(db, omarchy.String(), 10)
	if err != nil {
		t.Fatal(err)
	}
	before := c.count()
	msgs := WireMessages(omarchy.String(), rows, s.Senders())
	if reads := c.count() - before; reads != 2 {
		t.Errorf("%d contact reads for 2 senders, want 2", reads)
	}
	for _, m := range msgs {
		if m.Sender != "ada" && m.Sender != "grace" {
			t.Errorf("%s sender = %q", m.ID, m.Sender)
		}
	}
}

// Senders count inside the frame budget: a reply whose senders take room keeps fewer thumbnails, and
// the frame still fits in ipc.MaxFrameLen.
func TestSendersCountInsideTheFrameBudget(t *testing.T) {
	c := newNameClient()
	db := openTestStore(t)
	s, _, _ := lidSession(t, c, db)
	ref, err := proto.Marshal(thumbImage(jpegOf(MaxThumb), 640, 480).GetImageMessage())
	if err != nil {
		t.Fatal(err)
	}
	var rows []store.Message
	for i := 0; i < ipc.MaxChatLimit; i++ {
		sender := fmt.Sprintf("1555%07d@s.whatsapp.net", i)
		c.setContact(sender, Contact{PushName: fmt.Sprintf("%04d", i) + strings.Repeat("n", 3000)})
		m := store.Message{ID: fmt.Sprintf("G%d", i), ChatJID: omarchy.String(), Kind: store.KindText, Text: "hi",
			Timestamp: t0, Sender: sender}
		if i < ThumbBudget/MaxThumb {
			m.Kind, m.Mime, m.MediaRef = store.KindImage, "image/jpeg", ref
		}
		rows = append(rows, m)
	}
	msgs := WireMessages(omarchy.String(), rows, s.Senders())
	if !strings.HasPrefix(msgs[len(msgs)-1].Sender, fmt.Sprintf("%04d", len(rows)-1)) {
		t.Fatalf("the last message's sender = %.10q, want its name", msgs[len(msgs)-1].Sender)
	}
	if err := ipc.WriteFrame(io.Discard, ipc.MessagesResponse{V: 1, Op: ipc.OpMessages, JID: omarchy.String(), Messages: msgs}); err != nil {
		t.Errorf("the messages frame cannot be written: %v", err)
	}
}

// A reaction republishes its group message with the sender; a failed contact read falls back to the
// masked number, and the logs carry neither names nor numbers.
func TestAFailedContactReadFallsBackToTheMaskedNumberAndLogsNoPersonalData(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	c := newNameClient(ada.String(), adaLID.String())
	c.setContact(ada.String(), Contact{FullName: "Ada Lovelace"})
	db := openTestStore(t)
	_, events, logs := lidSession(t, c, db)
	c.emit(groupText(adaLID, "G1", false, "rope?"))
	if m, _ := publishedMessage(t, events(), "G1"); m.Sender != "Ada Lovelace" {
		t.Fatalf("sender = %q, want her name", m.Sender)
	}
	c.nmu.Lock()
	c.contactErr = errors.New("contact store for 15551230001 Ada Lovelace failed")
	c.nmu.Unlock()
	c.emit(groupEvent(omarchy, grace, "R1", false, t0, &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{
		Key: &waCommon.MessageKey{ID: proto.String("G1")}, Text: proto.String("👍")}}))
	m, _ := publishedMessage(t, events(), "G1")
	if m.Sender != "…0001" || len(m.Reactions) != 1 {
		t.Errorf("republished = %+v, want the masked number and the reaction", m)
	}
	all := strings.Join(*logs, "\n") + buf.String()
	if !strings.Contains(all, "cannot read a contact") {
		t.Errorf("the failed read was not logged: %q", all)
	}
	for _, secret := range []string{"Ada", "15551230001", "100000000000001", "0001", "120363000000000000"} {
		if strings.Contains(all, secret) {
			t.Errorf("logs carry %q:\n%s", secret, all)
		}
	}
}
