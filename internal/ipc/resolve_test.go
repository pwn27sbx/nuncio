package ipc

import (
	"slices"
	"testing"
)

// resolvingHandler is fakeHandler with a ChatResolver: aliases maps a phone-number JID to the LID
// chat it is an alias of (ADR 0001).
type resolvingHandler struct {
	*fakeHandler
	aliases map[string]string
}

func (r resolvingHandler) CanonicalJID(jid string) string {
	if c, ok := r.aliases[jid]; ok {
		return c
	}
	return jid
}

const (
	resolvePN  = "15551230001@s.whatsapp.net"
	resolveLID = "100000000000001@lid"
)

// Every request naming the phone-number JID reaches the handler, and is answered, as the LID chat.
func TestRequestsNamingAnAliasAreServedAndAnsweredAsTheCanonicalChat(t *testing.T) {
	f := newFakeHandler(t)
	f.sent = Sent{State: SendQueued}
	f.mediaPath, f.mediaMime = "/tmp/x.jpg", "image/jpeg"
	h := resolvingHandler{fakeHandler: f, aliases: map[string]string{resolvePN: resolveLID}}
	b := NewBroker()
	sock := startEventServer(t, h, b)
	conn, br := subscribed(t, sock)

	for _, c := range []struct {
		frame   string
		replyOp string
		hasJID  bool
	}{
		{`{"v":1,"op":"messages","jid":"` + resolvePN + `","limit":10}`, OpMessages, true},
		{`{"v":1,"op":"search","jid":"` + resolvePN + `","query":"x","limit":10}`, OpMessages, true},
		{`{"v":1,"op":"send","jid":"` + resolvePN + `","text":"hi","client_id":"c-1"}`, OpSent, false},
		{`{"v":1,"op":"send","jid":"` + resolvePN + `","text":"hi","client_id":"c-2","reply_to":"Q1"}`, OpSent, false},
		{`{"v":1,"op":"react","jid":"` + resolvePN + `","id":"Q1","emoji":"👍"}`, OpReacted, true},
		{`{"v":1,"op":"mark_read","jid":"` + resolvePN + `"}`, OpMarkedRead, true},
		{`{"v":1,"op":"download_media","jid":"` + resolvePN + `","id":"Q1"}`, OpMediaReady, true},
	} {
		sendRaw(t, conn, c.frame)
		reply := recvRaw(t, br)
		if op := fieldOf(t, reply, "op"); op != c.replyOp {
			t.Errorf("%s -> %s, want %s", c.frame, reply, c.replyOp)
			continue
		}
		if c.hasJID && fieldOf(t, reply, "jid") != resolveLID {
			t.Errorf("%s -> %s, want jid %s", c.frame, reply, resolveLID)
		}
	}

	sendRaw(t, conn, `{"v":1,"op":"focus","jid":"`+resolvePN+`"}`)
	for range 2 { // event_focus to this subscriber, then focused, in either order
		frame := recvRaw(t, br)
		if fieldOf(t, frame, "jid") != resolveLID {
			t.Errorf("focus -> %s, want jid %s", frame, resolveLID)
		}
	}

	for name, got := range map[string][]string{
		"messages":  f.calls(&f.asked),
		"searches":  f.calls(&f.searches),
		"sends":     f.calls(&f.sends),
		"replies":   f.calls(&f.replies),
		"reacts":    f.calls(&f.reacts),
		"markReads": f.calls(&f.markReads),
		"downloads": f.calls(&f.downloads),
	} {
		if len(got) != 1 || len(got[0]) < len(resolveLID) || got[0][:len(resolveLID)] != resolveLID {
			t.Errorf("handler %s = %q, want one call naming %s", name, got, resolveLID)
		}
	}
	if got := f.calls(&f.markReads); !slices.Equal(got, []string{resolveLID}) {
		t.Errorf("mark_read reached the handler as %q", got)
	}
}
