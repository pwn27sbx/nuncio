package wa

import (
	"bytes"
	"log"
	"os"
	"strings"
	"sync"
	"testing"
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
