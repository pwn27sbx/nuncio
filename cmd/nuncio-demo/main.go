// Command nuncio-demo is a demo daemon for trying the TUI by hand. It serves wire v1
// (proto/wire.md) on $XDG_RUNTIME_DIR/nuncio/core.sock with invented chats held in memory, and has
// no WhatsApp code path at all: no network, no database, no keyring. Sends are answered at once (a
// text starting with "fail" ends not sent, and retry then sends it; until then it cannot be replied
// or reacted to), reactions, search, media downloads (a photo and stickers with thumbnails, a document) and mark_read work (opening a chat clears its
// unread), and a short while after each send an invented reply arrives in that chat.
//
//	XDG_RUNTIME_DIR=/tmp/nuncio-demo go run ./cmd/nuncio-demo [-reply-after 2s]
//
// It refuses to start without XDG_RUNTIME_DIR or while something answers on the socket (a real
// nunciod may be running), and removes its socket and media files when it gets SIGINT or SIGTERM.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"image/png"
	"io/fs"
	"log"
	"net"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

const daemonVersion = "nuncio-demo/0.1.0"

func main() {
	replyAfter := flag.Duration("reply-after", 2*time.Second, "delay of the invented reply after each send (0 turns it off)")
	flag.Parse()
	log.SetPrefix("nuncio-demo: ")
	log.SetFlags(0)
	if err := run(*replyAfter); err != nil {
		log.Print(err)
		os.Exit(1)
	}
}

func run(replyAfter time.Duration) error {
	socket, err := ipc.DefaultSocketPath()
	if err != nil {
		return err
	}
	ln, err := listen(socket)
	if err != nil {
		return err
	}
	defer func() {
		_ = ln.Close()
		_ = os.Remove(socket)
	}()
	mediaDir, err := os.MkdirTemp(filepath.Dir(socket), "demo-media-")
	if err != nil {
		return fmt.Errorf("create the demo media dir: %w", err)
	}
	defer func() { _ = os.RemoveAll(mediaDir) }()

	events := ipc.NewBroker()
	d := newDemo(time.Now(), events, mediaDir, replyAfter)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	fmt.Printf("nuncio-demo: serving invented chats on %s (no WhatsApp); run the TUI with: XDG_RUNTIME_DIR=%s cargo run -p nuncio-tui\n",
		socket, os.Getenv("XDG_RUNTIME_DIR"))
	go func() {
		<-ctx.Done()
		_ = ln.Close()
	}()
	for {
		conn, err := ln.Accept()
		if err != nil {
			if errors.Is(err, net.ErrClosed) {
				return nil
			}
			return err
		}
		go func() { _ = ipc.ServeConnEvents(conn, d, events) }()
	}
}

// listen creates the socket like nunciod does: it refuses while something answers on it (never
// removing a live socket) and replaces a file left behind by a dead daemon.
func listen(socket string) (net.Listener, error) {
	dir := filepath.Dir(socket)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("create %s: %w", dir, err)
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		return nil, fmt.Errorf("chmod %s: %w", dir, err)
	}
	if conn, err := net.DialTimeout("unix", socket, 200*time.Millisecond); err == nil {
		_ = conn.Close()
		return nil, fmt.Errorf("a daemon is already serving %s; stop it first (the demo never replaces a live socket)", socket)
	}
	if _, err := os.Lstat(socket); err == nil {
		if err := os.Remove(socket); err != nil {
			return nil, fmt.Errorf("remove the stale socket %s: %w", socket, err)
		}
	} else if !errors.Is(err, fs.ErrNotExist) {
		return nil, err
	}
	ln, err := net.Listen("unix", socket)
	if err != nil {
		return nil, fmt.Errorf("listen on %s: %w", socket, err)
	}
	if err := os.Chmod(socket, 0o600); err != nil {
		_ = ln.Close()
		return nil, fmt.Errorf("chmod %s: %w", socket, err)
	}
	return ln, nil
}

// send is a send of mine, by client_id.
type send struct {
	jid, id string
	failed  bool
}

// demo is the in-memory ipc.Handler (with the M2 Searcher, Replier, Reactor and MarkReader).
type demo struct {
	mu         sync.Mutex
	chats      []*chat
	sends      map[string]*send
	next       int
	events     *ipc.Broker
	mediaDir   string
	replyAfter time.Duration
}

func newDemo(now time.Time, events *ipc.Broker, mediaDir string, replyAfter time.Duration) *demo {
	return &demo{chats: seed(now), sends: map[string]*send{}, events: events, mediaDir: mediaDir, replyAfter: replyAfter}
}

func (d *demo) find(jid string) *chat {
	for _, c := range d.chats {
		if c.info.JID == jid {
			return c
		}
	}
	return nil
}

func (c *chat) message(id string) *ipc.Message {
	for i := range c.msgs {
		if c.msgs[i].ID == id {
			return &c.msgs[i]
		}
	}
	return nil
}

func (d *demo) newID(prefix string) string {
	d.next++
	return fmt.Sprintf("%s%06d", prefix, d.next)
}

func (d *demo) unreadLocked() int {
	n := 0
	for _, c := range d.chats {
		n += c.info.Unread
	}
	return n
}

func (d *demo) Hello(string) ipc.Status { return d.Status() }

func (d *demo) Status() ipc.Status {
	d.mu.Lock()
	defer d.mu.Unlock()
	return ipc.Status{Daemon: daemonVersion, Linked: true, Session: ipc.SessionLinked, Unread: d.unreadLocked()}
}

// Chats lists the chats newest first, each with its last message.
func (d *demo) Chats() []ipc.Chat {
	d.mu.Lock()
	defer d.mu.Unlock()
	out := make([]ipc.Chat, 0, len(d.chats))
	for _, c := range d.chats {
		info := c.info
		if n := len(c.msgs); n > 0 {
			last := c.msgs[n-1]
			// As on the wire, a chat's last message carries no thumbnail or size.
			last.Thumb, last.ThumbMime, last.Width, last.Height, last.Animated = nil, "", 0, 0, false
			info.LastMessage = &last
		}
		out = append(out, info)
	}
	sort.SliceStable(out, func(i, j int) bool {
		return out[i].LastMessage.Timestamp > out[j].LastMessage.Timestamp
	})
	return out
}

// Messages lists up to limit messages newest first. Like nunciod, reading a chat does not mark it
// read: a UI sends mark_read for that.
func (d *demo) Messages(jid string, limit int) ([]ipc.Message, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	return newestFirst(d.find(jid), limit, func(ipc.Message) bool { return true }), nil
}

func newestFirst(c *chat, limit int, keep func(ipc.Message) bool) []ipc.Message {
	out := []ipc.Message{}
	if c == nil {
		return out
	}
	for i := len(c.msgs) - 1; i >= 0 && len(out) < limit; i-- {
		if keep(c.msgs[i]) {
			out = append(out, c.msgs[i])
		}
	}
	return out
}

// Search matches every word of query in the text, case-insensitively, newest first.
func (d *demo) Search(jid, query string, limit int) ([]ipc.Message, error) {
	words := strings.Fields(strings.ToLower(query))
	d.mu.Lock()
	defer d.mu.Unlock()
	return newestFirst(d.find(jid), limit, func(m ipc.Message) bool {
		text := strings.ToLower(m.Text)
		for _, w := range words {
			if !strings.Contains(text, w) {
				return false
			}
		}
		return true
	}), nil
}

func (d *demo) Send(jid, text, clientID string) (ipc.Sent, error) {
	return d.SendReply(jid, text, clientID, "")
}

// SendReply stores my message and answers sent, or not sent for a text starting with "fail".
func (d *demo) SendReply(jid, text, clientID, replyTo string) (ipc.Sent, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if s, ok := d.sends[clientID]; ok {
		return d.stateLocked(s)
	}
	c := d.find(jid)
	if c == nil {
		return ipc.Sent{}, errors.New("the demo has no such chat")
	}
	if replyTo != "" {
		quoted := c.message(replyTo)
		if quoted == nil {
			return ipc.Sent{}, errors.New("no message with that id in that chat")
		}
		if quoted.State != "" {
			return ipc.Sent{}, errNotSent("replied to")
		}
	}
	s := &send{jid: jid, id: d.newID("DEMOS"), failed: strings.HasPrefix(strings.ToLower(strings.TrimSpace(text)), "fail")}
	d.sends[clientID] = s
	m := ipc.Message{ID: s.id, Kind: ipc.KindText, Text: text, FromMe: true, Timestamp: ipc.Timestamp(time.Now()), ReplyTo: replyTo}
	if s.failed {
		m.State, m.ClientID = ipc.MessageNotSent, clientID
	}
	c.msgs = append(c.msgs, m)
	d.events.Publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: jid, Message: m})
	if !s.failed {
		d.replyLater(jid)
	}
	return d.stateLocked(s)
}

// stateLocked answers a send's stored state.
func (d *demo) stateLocked(s *send) (ipc.Sent, error) {
	if s.failed {
		return ipc.Sent{}, errors.New("the demo refuses texts that start with \"fail\"; retry sends it")
	}
	m := d.find(s.jid).message(s.id)
	return ipc.Sent{State: ipc.SendSent, ID: s.id, Timestamp: m.Timestamp}, nil
}

// Retry sends a not-sent send of mine, under its first id.
func (d *demo) Retry(clientID string) (ipc.Sent, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	s, ok := d.sends[clientID]
	if !ok {
		return ipc.Sent{}, errors.New("no send with that client_id")
	}
	if s.failed {
		s.failed = false
		m := d.find(s.jid).message(s.id)
		m.State, m.ClientID, m.Timestamp = "", "", ipc.Timestamp(time.Now())
		d.events.Publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: s.jid, Message: *m})
		d.replyLater(s.jid)
	}
	return d.stateLocked(s)
}

var replies = []string{"Got it 👍", "Haha, nice 😂", "OK, talk later", "¡Perfecto!", "好的，没问题", "تمام، شكراً"}

// replyLater publishes an invented incoming reply in jid after replyAfter, so live updates show.
func (d *demo) replyLater(jid string) {
	if d.replyAfter <= 0 {
		return
	}
	time.AfterFunc(d.replyAfter, func() {
		d.mu.Lock()
		defer d.mu.Unlock()
		c := d.find(jid)
		m := ipc.Message{ID: d.newID("DEMOI"), Kind: ipc.KindText, Text: replies[d.next%len(replies)], Timestamp: ipc.Timestamp(time.Now())}
		c.msgs = append(c.msgs, m)
		c.info.Unread++
		d.events.Publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: jid, Message: m})
		d.events.Publish(ipc.EventUnread{V: ipc.Version, Op: ipc.OpEventUnread, Unread: d.unreadLocked(), JID: jid, ChatUnread: c.info.Unread})
	})
}

// errNotSent refuses a reply or reaction aimed at a send of mine that WhatsApp never got (queued
// or not sent): like nunciod, there is no WhatsApp message to point at.
func errNotSent(what string) error {
	return fmt.Errorf("that message was not sent, so it cannot be %s", what)
}

// React sets (or with an empty emoji removes) my reaction, mine first.
func (d *demo) React(jid, id, emoji string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.find(jid)
	var m *ipc.Message
	if c != nil {
		m = c.message(id)
	}
	if m == nil {
		return errors.New("no message with that id in that chat")
	}
	if m.State != "" {
		return errNotSent("reacted to")
	}
	others := []ipc.Reaction{}
	for _, r := range m.Reactions {
		if !r.FromMe {
			others = append(others, r)
		}
	}
	m.Reactions = nil
	if emoji != "" {
		m.Reactions = append(m.Reactions, ipc.Reaction{Emoji: emoji, FromMe: true})
	}
	m.Reactions = append(m.Reactions, others...)
	if len(m.Reactions) == 0 {
		m.Reactions = nil
	}
	d.events.Publish(ipc.EventMessage{V: ipc.Version, Op: ipc.OpEventMessage, JID: jid, Message: *m})
	return nil
}

// DownloadMedia writes a small generated file for the media message into the demo's media dir.
func (d *demo) DownloadMedia(jid, id string) (string, string, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.find(jid)
	var m *ipc.Message
	if c != nil {
		m = c.message(id)
	}
	if m == nil || m.Media == nil {
		return "", "", errors.New("no media message with that id in that chat")
	}
	ext := ".txt"
	switch m.Kind {
	case ipc.KindImage:
		ext = ".png"
	case ipc.KindSticker:
		ext = ".webp"
	}
	path := filepath.Join(d.mediaDir, id+ext)
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return "", "", fmt.Errorf("create the demo media file: %w", err)
	}
	switch m.Kind {
	case ipc.KindImage:
		err = png.Encode(f, gradient(photoWidth, photoHeight))
	case ipc.KindSticker:
		_, err = f.Write(stickerWebP)
	default:
		_, err = f.WriteString("nuncio demo document\n\nThis file was generated by nuncio-demo; nothing came from WhatsApp.\n")
	}
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		return "", "", fmt.Errorf("write the demo media file: %w", err)
	}
	return path, m.Media.Mime, nil
}

// MarkRead clears the chat's unread (no read receipt: there is no WhatsApp), publishes its
// event_unread and returns the new total. It always succeeds; a chat with nothing unread, or one the
// demo does not have, just answers the total and publishes nothing, like nunciod.
func (d *demo) MarkRead(jid string) (int, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.find(jid)
	if c == nil || c.info.Unread == 0 {
		return d.unreadLocked(), nil
	}
	c.info.Unread = 0
	total := d.unreadLocked()
	d.events.Publish(ipc.EventUnread{V: ipc.Version, Op: ipc.OpEventUnread, Unread: total, JID: jid})
	return total, nil
}

// Wipe is always refused: the demo has nothing real to delete.
func (d *demo) Wipe(bool) error {
	return errors.New("the demo daemon never wipes anything")
}
