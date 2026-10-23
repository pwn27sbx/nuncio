package wa

import (
	"bytes"
	"database/sql"
	"errors"
	"log"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"google.golang.org/protobuf/proto"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// lockedLog is a log writer safe for -race: the session may log from more than one goroutine.
type lockedLog struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (l *lockedLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.buf.Write(p)
}

func (l *lockedLog) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.buf.String()
}

// captureLog sends the standard logger to a fresh lockedLog until the test ends.
func captureLog(t *testing.T) *lockedLog {
	t.Helper()
	l := &lockedLog{}
	log.SetOutput(l)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	return l
}

const secretText = "the secret words of Ada"

func TestAMessageTheStoreCannotSaveIsLoggedByItsTypeOnly(t *testing.T) {
	logs := captureLog(t)
	db := openTestStore(t)
	s := NewSession(&fakeClient{device: true}, db, nil)
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	s.storeMessage(textEvent(ada, "M1", false, t0, secretText))
	got := logs.String()
	if !strings.Contains(got, "wa: cannot store a message (") {
		t.Errorf("log = %q, want the failed store by its error type", got)
	}
	if strings.Contains(got, "database is closed") {
		t.Errorf("log = %q carries the error's text", got)
	}
	if strings.Contains(got, secretText) {
		t.Errorf("log = %q carries the message text", got)
	}
}

func TestReactionsTheStoreCannotReadAreLoggedByTheirTypeOnly(t *testing.T) {
	logs := captureLog(t)
	db := openTestStore(t)
	s := NewSession(&fakeClient{device: true}, db, nil)
	if _, err := db.Exec(`DROP TABLE reactions`); err != nil {
		t.Fatal(err)
	}
	s.storeMessage(textEvent(ada, "M1", false, t0, secretText))
	got := logs.String()
	if !strings.Contains(got, "wa: cannot read a message's reactions (") {
		t.Errorf("log = %q, want the failed read by its error type", got)
	}
	if strings.Contains(got, "no such table") {
		t.Errorf("log = %q carries the error's text", got)
	}
	if strings.Contains(got, secretText) {
		t.Errorf("log = %q carries the message text", got)
	}
}

// refusal is the text a test trigger fails a write with: it quotes a JID and an id, as SQLite's own
// text can quote a value.
const refusal = "store refused 15551230001 client-secret"

// refuse makes every write of kind (INSERT, UPDATE) on table fail with refusal until undone.
func refuse(t *testing.T, db *sql.DB, kind, table string) (undo func()) {
	t.Helper()
	name := "refuse_" + strings.ToLower(kind) + "_" + table
	if _, err := db.Exec(`CREATE TRIGGER ` + name + ` BEFORE ` + kind + ` ON ` + table +
		` BEGIN SELECT RAISE(ABORT, '` + refusal + `'); END`); err != nil {
		t.Fatal(err)
	}
	return func() {
		if _, err := db.Exec(`DROP TRIGGER ` + name); err != nil {
			t.Fatal(err)
		}
	}
}

// assertTypeOnly checks that the log holds every fixed description in wants and none of the
// store's error text, nor a JID, id, client id or message text.
func assertTypeOnly(t *testing.T, got string, wants ...string) {
	t.Helper()
	for _, w := range wants {
		if !strings.Contains(got, w+" (") {
			t.Errorf("log = %q, want %q by its error type", got, w)
		}
	}
	for _, leak := range []string{"store refused", "no such table", "database is closed",
		"15551230001", "120363", "client-secret", "3EB0", "QSECRET", secretText} {
		if strings.Contains(got, leak) {
			t.Errorf("log = %q carries %q", got, leak)
		}
	}
}

func TestSendStoreFailuresAreLoggedByTheirTypeOnly(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	db := s.Store()
	storeIncoming(t, fake, ada, "QSECRET1", secretText)
	fake.sendErr = errors.New("refused")
	if _, err := s.Send(ada.String(), secretText, "client-secret-0"); !errors.Is(err, ErrNotSent) {
		t.Fatalf("Send = %v, want ErrNotSent", err)
	}
	logs := captureLog(t)

	undo := refuse(t, db, "UPDATE", "outbox")
	_, _ = s.Send(ada.String(), secretText, "client-secret-2") // not sent, and that cannot be stored
	fake.sendErr = nil
	_, _ = s.Send(ada.String(), secretText, "client-secret-1") // sent, and that cannot be stored
	_, _ = s.Retry("client-secret-0")
	undo()

	undo = refuse(t, db, "INSERT", "messages")
	_, _ = s.Send(ada.String(), secretText, "client-secret-3")
	undo()

	if _, _, err := store.Enqueue(db, store.OutboxEntry{ClientID: "client-secret-4", ChatJID: ada.String(),
		Text: secretText, MessageID: "3EB0SECRET4", Created: t0, ReplyTo: "QSECRET1"}); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`DROP TABLE messages`); err != nil {
		t.Fatal(err)
	}
	s.drain()
	_, _ = s.SendReply(ada.String(), secretText, "client-secret-5", "QSECRET1")

	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	_, _ = s.SendReply(ada.String(), secretText, "client-secret-6", "QSECRET1")
	_, _ = s.Send(ada.String(), secretText, "client-secret-6")
	_, _ = s.deliver("client-secret-6", true)
	s.drain()

	assertTypeOnly(t, logs.String(),
		"wa: cannot record a failed send",
		"wa: cannot record a sent message",
		"wa: cannot queue a retry",
		"wa: cannot store a sent message",
		"wa: a reply's quoted message cannot be read, so it is sent unquoted",
		"wa: cannot read a quoted message",
		"wa: cannot read a send",
		"wa: cannot queue a send",
		"wa: cannot read the outbox")
}

func TestReactionStoreFailuresAreLoggedByTheirTypeOnly(t *testing.T) {
	s, fake, _ := subscribedSession(t)
	storeIncoming(t, fake, group, "QSECRET1", secretText)
	logs := captureLog(t)
	undo := refuse(t, s.Store(), "INSERT", "reactions")
	defer undo()

	if err := s.React(group.String(), "QSECRET1", "👍"); err != nil {
		t.Fatalf("React = %v, want nil: the reaction was sent", err)
	}
	evt := messageEvent(group, "RSECRET", false, t0.Add(time.Minute), &waE2E.Message{ReactionMessage: &waE2E.ReactionMessage{
		Key:  &waCommon.MessageKey{ID: proto.String("QSECRET1"), RemoteJID: proto.String(group.String()), FromMe: proto.Bool(false)},
		Text: proto.String("😂"),
	}})
	evt.Info.Sender = ada
	fake.emit(evt)

	assertTypeOnly(t, logs.String(), "wa: cannot store a sent reaction", "wa: cannot store a reaction")
}

func TestAppStateStoreFailuresAreLoggedByTheirTypeOnly(t *testing.T) {
	_, fake, db := startedSession(t)
	fake.emit(textEvent(ada, "QSECRET1", false, t0, secretText))
	logs := captureLog(t)

	undoInsert := refuse(t, db, "INSERT", "chats")
	undoUpdate := refuse(t, db, "UPDATE", "chats")
	undoMessages := refuse(t, db, "UPDATE", "messages")
	fake.emit(pinEvent(ada, true))
	fake.emit(archiveEvent(ada, true))
	fake.emit(muteEvent(ada, true, -1))
	fake.emit(markReadEvent(ada, true))
	undoInsert()
	undoUpdate()
	undoMessages()

	if _, err := db.Exec(`DROP TABLE messages`); err != nil {
		t.Fatal(err)
	}
	fake.emit(pinEvent(ada, true))

	assertTypeOnly(t, logs.String(),
		"wa: cannot apply a pin from app state",
		"wa: cannot apply an archive from app state",
		"wa: cannot apply a mute from app state",
		"wa: cannot apply a read state from app state",
		"wa: cannot publish a chat changed by app state")
}
