package main

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// testQR is the matrix `nuncio link --qr` draws in these tests: 3 modules per side, so a test can
// compare the art exactly.
var testQR = ipc.EventLinkQR{V: 1, Op: ipc.OpLinkQR, Size: 3, Rows: []string{"101", "010", "111"}}

// The art RenderMatrix draws for testQR, as `nuncio link --qr` prints it.
const testQRArt = "    ▀▄▀    \n    ▀▀▀    "

// linkDaemon is a fake daemon whose LinkStart publishes `script` to the subscribers, so the CLI
// sees the flow's frames after it subscribed and before it waits.
func linkDaemon(t *testing.T, script ...any) *fakeDaemon {
	t.Helper()
	d := &fakeDaemon{session: ipc.SessionUnlinked}
	d.onLinkStart = func(d *fakeDaemon) {
		for _, frame := range script {
			d.events.Publish(frame)
		}
	}
	serveFakeDaemon(t, d)
	return d
}

func TestLinkQRAsksTheDaemonWithConsentAndDrawsItsCode(t *testing.T) {
	root := isolateData(t)
	d := linkDaemon(t,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkWaiting},
		testQR,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkLinked},
	)

	code, out, errOut := runCLI("link", "--qr", "--consent")
	if code != 0 {
		t.Fatalf("exit %d, want 0; stderr %q", code, errOut)
	}
	if !strings.Contains(out, testQRArt) {
		t.Errorf("stdout %q, want the code the daemon published", out)
	}
	if !strings.Contains(out, "nuncio: linked") {
		t.Errorf("stdout %q, want the linked line", out)
	}
	// The daemon is up and owns the session: the CLI never tells the user to start it.
	if strings.Contains(out, "systemctl") || strings.Contains(errOut, "systemctl") {
		t.Errorf("stdout %q / stderr %q, want no systemctl line", out, errOut)
	}
	if got := d.linkStarts(); len(got) != 1 || !got[0] {
		t.Errorf("the daemon saw link_start %v, want exactly one with consent", got)
	}
	if errOut != "" {
		t.Errorf("stderr %q, want nothing", errOut)
	}
	// S9: the daemon owns the store, so a link from the CLI leaves exactly what one from the app
	// leaves, and this command creates nothing of its own.
	assertEmptyDir(t, root)
}

// TestLinkQRNoLongerRefusesWhileTheDaemonAnswers is one of T5's two intended output changes: the
// flow belongs to the daemon now, so a running daemon is required, not refused.
func TestLinkQRNoLongerRefusesWhileTheDaemonAnswers(t *testing.T) {
	isolateData(t)
	d := linkDaemon(t, ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkLinked})

	code, out, errOut := runCLI("link", "--qr", "--consent")
	if code != 0 {
		t.Fatalf("exit %d, want 0; stderr %q", code, errOut)
	}
	if strings.Contains(errOut, "nunciod is running") {
		t.Errorf("stderr %q, want no daemon-owns-the-session refusal", errOut)
	}
	if !strings.Contains(out, "nuncio: linked") {
		t.Errorf("stdout %q, want the linked line", out)
	}
	if got := d.linkStarts(); len(got) != 1 {
		t.Errorf("the daemon saw %d link_starts, want one", len(got))
	}
}

func TestLinkQRWithoutARunningDaemonSaysSoAndLinksNothing(t *testing.T) {
	root := isolateData(t)
	t.Setenv("XDG_RUNTIME_DIR", shortTempDir(t)) // no core.sock there

	code, out, errOut := runCLI("link", "--qr", "--consent")
	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	if !strings.Contains(errOut, "nunciod is not running") || !strings.Contains(errOut, "Nothing was linked") {
		t.Errorf("stderr %q, want the daemon-is-not-running line", errOut)
	}
	if out != "" {
		t.Errorf("stdout %q, want nothing", out)
	}
	// The daemon owns the store: this path opens nothing of its own.
	assertEmptyDir(t, root)
}

func TestLinkQRPlainWritesArtAndNothingElse(t *testing.T) {
	isolateData(t)
	linkDaemon(t,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkWaiting},
		testQR,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkLinked},
	)

	code, out, errOut := runCLI("link", "--qr", "--plain-qr", "--consent")
	if code != 0 {
		t.Fatalf("exit %d, want 0; stderr %q", code, errOut)
	}
	// Every non-blank line is art: a script or a GUI reads the code and nothing else, and the
	// outcome comes from the exit status.
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) != "" && strings.Trim(line, "▀▄█ ") != "" {
			t.Errorf("stdout line %q is not art", line)
		}
	}
	if !strings.Contains(out, testQRArt) {
		t.Errorf("stdout %q, want the code", out)
	}
}

func TestLinkQRFailureShowsTheDaemonsSentenceAndFails(t *testing.T) {
	isolateData(t)
	linkDaemon(t,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkWaiting},
		testQR,
		ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkFailed, Reason: ipc.LinkFailureReason},
	)

	code, out, errOut := runCLI("link", "--qr", "--consent")
	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	if !strings.Contains(errOut, ipc.LinkFailureReason) {
		t.Errorf("stderr %q, want the daemon's sentence", errOut)
	}
	if strings.Contains(out, "nuncio: linked") {
		t.Errorf("stdout %q, want no linked line", out)
	}
}

// A daemon that ends the flow without a reason still gets the one fixed sentence shown.
func TestLinkQRFailedWithoutAReasonUsesTheFixedSentence(t *testing.T) {
	isolateData(t)
	linkDaemon(t, ipc.EventLinkState{V: 1, Op: ipc.OpLinkState, State: ipc.LinkFailed})

	code, out, errOut := runCLI("link", "--qr", "--consent")
	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	if !strings.Contains(errOut, ipc.LinkFailureReason) {
		t.Errorf("stderr %q, want %q", errOut, ipc.LinkFailureReason)
	}
	if strings.Contains(out, "nuncio: linked") {
		t.Errorf("stdout %q, want no linked line", out)
	}
}

// TestLinkQRCancelAsksTheDaemonToRemoveWhatItMade is S7 for the command line: an interrupted flow
// leaves no device, no store and no keyring entry, because the daemon removes what it created.
func TestLinkQRCancelAsksTheDaemonToRemoveWhatItMade(t *testing.T) {
	isolateData(t)
	started := make(chan struct{})
	d := &fakeDaemon{session: ipc.SessionUnlinked}
	d.onLinkStart = func(*fakeDaemon) { close(started) }
	serveFakeDaemon(t, d)

	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		<-started
		cancel()
	}()
	var out, errOut strings.Builder
	code := linkByQR(ctx, wa.Consent{Granted: true}, false, &out, &errOut)

	if code != 1 {
		t.Errorf("exit %d, want 1", code)
	}
	if !strings.Contains(errOut.String(), "nothing was linked") {
		t.Errorf("stderr %q, want the cancellation line", errOut.String())
	}
	deadline := time.Now().Add(2 * time.Second)
	for d.cancelCalls() == 0 && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	if got := d.cancelCalls(); got != 1 {
		t.Errorf("the daemon saw %d link_cancels, want exactly 1", got)
	}
}
