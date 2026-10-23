package wa

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// errKindSecret is in every error's text below: ErrKind must never return it.
const errKindSecret = "15551230001"

func TestErrKindNamesTheInnermostCauseNeverNilNorTheText(t *testing.T) {
	_, statErr := os.Stat(filepath.Join(t.TempDir(), errKindSecret))
	var pathErr *fs.PathError
	if !errors.As(statErr, &pathErr) {
		t.Fatalf("os.Stat error %T is not a *fs.PathError", statErr)
	}
	// A *fs.PathError wraps the syscall.Errno that names the failure, its innermost cause.
	sentinel := errors.New("media: the cache could not be written")
	cases := []struct {
		name string
		err  error
		want string
	}{
		{"an error that wraps nothing", errors.New("dial " + errKindSecret), "*errors.errorString"},
		{"a wrapped *fs.PathError", fmt.Errorf("wa: %w", fmt.Errorf("media: %w", statErr)), "syscall.Errno"},
		{"a sentinel and its cause", fmt.Errorf("%w: %w", sentinel, statErr), "syscall.Errno"},
		{"a join", errors.Join(errors.New(errKindSecret), statErr), "syscall.Errno"},
		{"a cache error", cacheLike{fmt.Errorf("media: %w", statErr)}, "syscall.Errno"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := ErrKind(tc.err)
			if got != tc.want {
				t.Errorf("ErrKind = %q, want %q", got, tc.want)
			}
			if strings.Contains(got, "nil") || strings.Contains(got, errKindSecret) {
				t.Errorf("ErrKind = %q names nil or the error's text", got)
			}
		})
	}
}

// cacheLike is shaped like media's cacheErr: a struct that wraps its cause.
type cacheLike struct{ error }

func (e cacheLike) Unwrap() error { return e.error }

// failingLIDMap is a client whose LID map cannot be read, its error wrapped as the whatsmeow
// adapter wraps it.
type failingLIDMap struct{ *fakeClient }

func (failingLIDMap) LIDForPN(context.Context, string) (string, error) {
	return "", fmt.Errorf("wa: reading the LID of a phone number: %w", lidStoreError{})
}

func (failingLIDMap) PNForLID(context.Context, string) (string, error) {
	return "", fmt.Errorf("wa: reading the phone number of a LID: %w", lidStoreError{})
}

// lidStoreError is the LID map's own failure, quoting a JID as a SQL error can.
type lidStoreError struct{}

func (lidStoreError) Error() string { return "no such row for " + errKindSecret }

func TestAnUnreadableLIDMapIsLoggedByItsCausesType(t *testing.T) {
	db := openTestStore(t)
	s, _, logs := lidSession(t, failingLIDMap{&fakeClient{device: true}}, db)
	if got := s.CanonicalJID(ada.String()); got != ada.String() {
		t.Errorf("CanonicalJID = %q, want the phone-number chat itself", got)
	}
	got := strings.Join(*logs, "\n")
	if !strings.Contains(got, "wa: cannot read the LID map (wa.lidStoreError)") {
		t.Errorf("logs = %q, want the LID map failure by its cause's type", got)
	}
	if strings.Contains(got, errKindSecret) {
		t.Errorf("logs = %q carry the error's text", got)
	}
}
