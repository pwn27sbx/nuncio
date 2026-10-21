package ipc

import (
	"encoding/json"
	"unicode/utf8"
)

// CutToFit cuts *field on a code point boundary (never inside a UTF-8 sequence) to the longest
// prefix that, followed by "…", makes fits report true; fits measures whatever holds field. It
// reports false, leaving field as it was, when not even an empty prefix fits.
func CutToFit(field *string, fits func() bool) bool {
	text := *field
	try := func(n int) bool {
		*field = text[:n] + "…"
		return fits()
	}
	// boundary is n moved back to the start of the code point it falls in.
	boundary := func(n int) int {
		for n > 0 && n < len(text) && !utf8.RuneStart(text[n]) {
			n--
		}
		return n
	}
	if !try(0) {
		*field = text
		return false
	}
	lo, hi := 0, len(text) // try(lo) holds; the answer is in [lo, hi]
	for lo < hi {
		mid := boundary(lo + (hi-lo+1)/2)
		if mid <= lo {
			// No code point starts between lo and the midpoint: try the next one after lo.
			_, size := utf8.DecodeRuneInString(text[lo:])
			mid = lo + size
			if mid > hi {
				break
			}
		}
		if try(mid) {
			lo = mid
		} else {
			hi = mid - 1
		}
	}
	*field = text[:lo] + "…"
	return true
}

// CutMessage cuts m so that fits reports true: its text (CutToFit) and, when not even an empty text
// fits, its text becomes "…" (when it had one) and its file name is cut. It reports false when that
// is not enough. m's Media is cut in place: the caller owns it.
func CutMessage(m *Message, fits func() bool) bool {
	if CutToFit(&m.Text, fits) {
		return true
	}
	if m.Text != "" {
		m.Text = "…"
	}
	return m.Media != nil && CutToFit(&m.Media.FileName, fits)
}

// fitChats is chats when their reply encodes in at most MaxFrameLen bytes, so a reply that fits is
// served as before. Otherwise it is the first chats (the newest) that fit, each whole, except the
// first when it alone does not fit, which is served cut (cutChat). chats itself is never changed.
func fitChats(chats []Chat) []Chat {
	whole, err := json.Marshal(ChatsResponse{V: Version, Op: OpChats, Chats: chats})
	if err != nil || len(whole) <= MaxFrameLen {
		return chats
	}
	empty, err := json.Marshal(ChatsResponse{V: Version, Op: OpChats, Chats: []Chat{}})
	if err != nil {
		return []Chat{}
	}
	room := MaxFrameLen - len(empty)
	out := []Chat{}
	for i, c := range chats {
		enc, err := json.Marshal(c)
		if err != nil {
			break
		}
		cost := len(enc)
		if i > 0 {
			cost++ // the comma before it
		} else if cost > room {
			var ok bool
			if c, ok = cutChat(c, room); !ok {
				break
			}
			if enc, err = json.Marshal(c); err != nil {
				break
			}
			cost = len(enc)
		}
		if cost > room {
			break
		}
		room -= cost
		out = append(out, c)
	}
	return out
}

// cutChat is a copy of c cut to encode in at most room bytes: its last message's text, then that
// message's file name (CutMessage), then its name. It reports false when even that does not fit.
func cutChat(c Chat, room int) (Chat, bool) {
	fits := func() bool {
		enc, err := json.Marshal(c)
		return err == nil && len(enc) <= room
	}
	if c.LastMessage != nil {
		m := *c.LastMessage
		if m.Media != nil {
			media := *m.Media
			m.Media = &media
		}
		c.LastMessage = &m
		if CutMessage(&m, fits) {
			return c, true
		}
	}
	return c, CutToFit(&c.Name, fits)
}

// fitEvent is the encoding of frame, an event whose encoding is longer than MaxFrameLen, cut to fit
// in one frame: an EventMessage's text, then its file name (CutMessage), on a copy. It reports false
// for any other event and for one that does not fit even cut.
func fitEvent(frame any) ([]byte, bool) {
	ev, ok := frame.(EventMessage)
	if !ok {
		return nil, false
	}
	if ev.Message.Media != nil {
		media := *ev.Message.Media
		ev.Message.Media = &media
	}
	fits := func() bool {
		enc, err := json.Marshal(ev)
		return err == nil && len(enc) <= MaxFrameLen
	}
	if !CutMessage(&ev.Message, fits) {
		return nil, false
	}
	data, err := json.Marshal(ev)
	return data, err == nil
}
