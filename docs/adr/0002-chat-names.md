# ADR 0002: chat names and the masked-number fallback

- Status: accepted (T15 follow-up, Part 2.3)
- Date: 2026-10-20

## Context

The first real-account run (T15) showed the chat list as LID digits: the `chats` table has had a
`name` column since M0, but nothing ever filled it, and a client falling back to the JID's user part
shows a contact's hidden id (`digits@lid`), which is not her number. The owner asked: "sync contact/
push names and group subjects from whatsmeow (`Store.Contacts`, group info) into the store, and serve
them on `chats`. Fall back to the masked number, never the raw LID digits."

whatsmeow already keeps the names (all evidence from
`go.mau.fi/whatsmeow@v0.0.0-20261017175727-b2b1e71d45e2`):

- `store.ContactStore` (`store/store.go:102-110`): `GetContact` returns `types.ContactInfo`
  (`types/user.go:74-83`: `FullName`, `FirstName`, `PushName`, `BusinessName`) from a cache or one
  row of `whatsmeow_contacts` (`store/sqlstore/store.go:747-778`); no network.
- Push and business names are stored from every message before `events.PushName` /
  `events.BusinessName` is dispatched, under the sender and her alternate JID
  (`user.go:679-735`, `message.go:57-60`). Address-book entries come from app state
  (`appstate.go:292-297`, `events.Contact`), one event per contact even on the full sync because
  nuncio sets `EmitAppStateEventsOnFullSync` (`appstate.go:181-186`).
- Group subjects are not stored by whatsmeow. `GetJoinedGroups` (`group.go:538`) answers every group
  with its `GroupName.Name` in one IQ; `events.GroupInfo.Name` (`types/events/events.go:493-500`)
  carries a subject change and `events.JoinedGroup` (`:480-490`) a new group.

## Decision

1. **Names are resolved by the daemon and stored in `chats.name`.** Group: its subject. Direct chat:
   full name, else first name, else business name, else push name; a LID chat reads both its own
   contact and the one under its phone number (whatsmeow's LID map, ADR 0001), field by field.
   Nothing known: empty.
2. **When.** At start, one pass over the stored direct chats (local reads); one `GetJoinedGroups`
   per start, off the event goroutine, after the first connection (a ban-risk bound: never per
   message, per reconnect or per group); on PushName, BusinessName, Contact, GroupInfo (with a
   subject) and JoinedGroup; and when a message creates a chat with no name yet. A changed name
   publishes `event_chat`. Only stored chats are named.
3. **The fallback is served, masked, in a new optional `phone` field** (`…0001`, the CLI's existing
   rule, now `ipc.MaskPhone`). A client cannot compute it: for a LID chat only the daemon's LID map
   knows the number, and the JID's digits are not a number. The daemon masks before the wire, so a
   full number never reaches a client or its logs. Absent for groups and unknown numbers; frames
   without it are byte for byte as before (one fixture added, `event_chat_phone.json`).
4. A failed read keeps the stored name; logs carry the error's type only.

## Alternatives considered

- **Resolve at serve time, store nothing.** No event bookkeeping, but group subjects exist only in
  memory or on the network, and every `chats` would read the contact store per chat. Rejected for
  groups; direct names are cheap either way, and one stored column serves both.
- **Put the masked number in `name`.** No new field, but a client could not tell a real name from
  the fallback (sorting, search, a future "add contact"). Rejected.
- **Serve the full number and let clients mask.** Puts full numbers on the wire and into client
  memory for no gain. Rejected.

## Consequences

- `chats` and `event_chat` carry names; clients show `name`, else `phone`, else a placeholder, never
  LID digits. `nuncio chats` prints `hidden number` for a LID chat with neither.
- Address-book contacts without a stored chat are never listed.
- One extra local contact read per chat at start and per chat created; one network query per start.
