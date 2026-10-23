package wa

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
)

// Chat names (docs/adr/0002-chat-names.md). A chat's name is resolved from whatsmeow and stored in
// the chats table, so `chats` and every `event_chat` serve it:
//
//   - a group: its subject, from one joined-groups query per Start (GetJoinedGroups, after the first
//     connection), JoinedGroup and every GroupInfo subject change;
//   - a direct chat: the address-book name (full, else first), else the business name, else the push
//     name, read from whatsmeow's contact store (Store.Contacts, a cached local read, never the
//     network). A LID chat also reads the contact stored under its phone number (whatsmeow's LID
//     map), field by field: the address book usually holds the number, WhatsApp the LID's push name.
//
// Names are re-read at Start (one pass over the stored chats), when a chat is created by its first
// message, and on whatsmeow's PushName, BusinessName and Contact (app-state) events, which whatsmeow
// emits after it stored the new name. A changed name publishes `event_chat`. Only stored chats are
// named, so the address book never fills the chat list. A failed read keeps the stored name and is
// logged by its error type only: names, numbers and JIDs never reach a log.

// nameLookupTimeout bounds one read of whatsmeow's contact store; groupsQueryTimeout the one
// joined-groups query of a Start.
const (
	nameLookupTimeout  = 2 * time.Second
	groupsQueryTimeout = 20 * time.Second
)

// Contact is the names whatsmeow stores for one user JID (types.ContactInfo).
type Contact struct{ FullName, FirstName, BusinessName, PushName string }

// Contacts is the slice of whatsmeow the session reads chat names from. It is optional: a Client
// without it leaves every name as stored.
type Contacts interface {
	// Contact is the stored contact of a user JID (Store.Contacts.GetContact), zero when unknown.
	Contact(ctx context.Context, jid string) (Contact, error)
	// JoinedGroups is the subject of every group the account is in, by group JID: one network query
	// (GetJoinedGroups).
	JoinedGroups(ctx context.Context) (map[string]string, error)
}

func isGroupChat(jid string) bool { return strings.HasSuffix(jid, "@g.us") }

// ChatPhone is what the wire serves as a chat's `phone`: the contact's phone number masked by
// ipc.MaskPhone. A phone-number chat masks its own number, a LID chat the number whatsmeow's LID map
// knows for it; a LID chat whose number is unknown and a group have none.
func (s *Session) ChatPhone(jid string) string {
	pn := jid
	if isLIDChat(jid) {
		pn = s.pnOf(jid)
	}
	if !isPhoneChat(pn) {
		return ""
	}
	user, _, _ := strings.Cut(pn, "@")
	return ipc.MaskPhone(user)
}

// contacts is the client's contact store, if it has one.
func (s *Session) contacts() (Contacts, bool) {
	if s.client == nil {
		return nil, false
	}
	c, ok := s.client.(Contacts)
	return c, ok
}

// contactName resolves the name of the direct chat jid (see the priority above); ok is false when a
// read failed, so the stored name is kept.
func (s *Session) contactName(book Contacts, jid string) (name string, ok bool) {
	users := []string{jid}
	if alt := s.altOf(jid); alt != "" {
		users = append(users, alt)
	}
	var found []Contact
	for _, u := range users {
		ctx, cancel := context.WithTimeout(context.Background(), nameLookupTimeout)
		c, err := book.Contact(ctx, u)
		cancel()
		if err != nil {
			s.log("wa: cannot read a contact's name (%s)", ErrKind(err))
			return "", false
		}
		found = append(found, c)
	}
	for _, field := range []func(Contact) string{
		func(c Contact) string { return c.FullName },
		func(c Contact) string { return c.FirstName },
		func(c Contact) string { return c.BusinessName },
		func(c Contact) string { return c.PushName },
	} {
		for _, c := range found {
			if n := field(c); n != "" {
				return n, true
			}
		}
	}
	return "", true
}

// altOf is the other JID of the same contact in whatsmeow's LID map, "" when unknown.
func (s *Session) altOf(jid string) string {
	if isLIDChat(jid) {
		return s.pnOf(jid)
	}
	return s.lidOf(jid)
}

// resolveName is the name chat jid should carry; ok is false when it is not known (a group whose
// subject has not arrived, a client without a contact store, a failed read).
func (s *Session) resolveName(jid string) (name string, ok bool) {
	if isGroupChat(jid) {
		s.mu.Lock()
		defer s.mu.Unlock()
		name, ok = s.groupNames[jid]
		return name, ok
	}
	book, has := s.contacts()
	if !has {
		return "", false
	}
	return s.contactName(book, jid)
}

// refreshName stores the resolved name of chat jid, when it is stored, and publishes it as
// event_chat when it changed.
func (s *Session) refreshName(jid string) {
	if s.storeName(jid) {
		s.publishChat(jid)
	}
}

// storeName stores the resolved name of chat jid, when it is stored, and reports whether it
// changed; it publishes nothing.
func (s *Session) storeName(jid string) bool {
	if s.db == nil {
		return false
	}
	if _, err := store.ChatName(s.db, jid); err != nil {
		if !errors.Is(err, store.ErrNoChat) {
			s.log("wa: cannot read a chat's name (%T)", err)
		}
		return false
	}
	name, ok := s.resolveName(jid)
	if !ok {
		return false
	}
	changed, err := store.SetChatName(s.db, jid, name)
	if err != nil {
		s.log("wa: cannot save a chat's name (%T)", err)
		return false
	}
	return changed
}

// nameNewChat names chat jid when it has no name yet: a chat its first message just created.
func (s *Session) nameNewChat(jid string) {
	if name, err := store.ChatName(s.db, jid); err == nil && name == "" {
		s.refreshName(jid)
	}
}

// refreshStoredNames re-reads the name of every stored direct chat; Start runs it before connecting.
func (s *Session) refreshStoredNames() {
	if _, ok := s.contacts(); !ok {
		return
	}
	jids, err := store.ChatJIDs(s.db)
	if err != nil {
		s.log("wa: cannot list the chats to name (%T)", err)
		return
	}
	for _, jid := range jids {
		if !isGroupChat(jid) {
			s.refreshName(jid)
		}
	}
}

// contactChanged re-reads the names of the chats of users whose names whatsmeow just stored: each
// user's own chat and that of her other JID in the LID map.
func (s *Session) contactChanged(users ...string) {
	seen := map[string]bool{}
	for _, u := range users {
		if !ipc.ValidJID(u) || isGroupChat(u) {
			continue
		}
		for _, jid := range []string{u, s.altOf(u)} {
			if jid != "" && !seen[jid] {
				seen[jid] = true
				s.refreshName(jid)
			}
		}
	}
}

// groupNamed records a group's subject and renames its stored chat.
func (s *Session) groupNamed(group, subject string) {
	if !isGroupChat(group) || !ipc.ValidJID(group) {
		return
	}
	s.mu.Lock()
	if s.groupNames == nil {
		s.groupNames = map[string]string{}
	}
	s.groupNames[group] = subject
	s.mu.Unlock()
	s.refreshName(group)
}

// syncGroupNames runs the joined-groups query and names every stored group from it. The session
// runs it once per Start, after the first connection.
func (s *Session) syncGroupNames() {
	book, ok := s.contacts()
	if !ok {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), groupsQueryTimeout)
	defer cancel()
	groups, err := book.JoinedGroups(ctx)
	if err != nil {
		s.log("wa: cannot read the group subjects (%s)", ErrKind(err))
		return
	}
	for jid, subject := range groups {
		s.groupNamed(jid, subject)
	}
}

// syncGroupNamesOnce starts syncGroupNames off whatsmeow's event goroutine on the first connection
// of a Start; later connections query nothing.
func (s *Session) syncGroupNamesOnce() {
	if _, ok := s.contacts(); !ok || s.db == nil {
		return
	}
	s.mu.Lock()
	done := s.groupsSynced
	s.groupsSynced = true
	s.mu.Unlock()
	if done {
		return
	}
	s.drains.Add(1)
	go func() {
		defer s.drains.Done()
		s.syncGroupNames()
	}()
}
