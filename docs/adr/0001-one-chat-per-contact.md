# ADR 0001: one chat per contact

- Status: accepted (T15 fix round, Q2 option 2)
- Date: 2026-10-20

## Context

WhatsApp addresses a direct contact in two ways: by phone number (`digits@s.whatsapp.net`, PN) and
by hidden id (`digits@lid`, LID). Today it delivers a contact's messages in her LID chat, and
whatsmeow routes a send to a PN JID to the contact's LID as well.

Before this decision nuncio stored every message under the JID the event or the request named.
The T15 test at home (the home handoff of 2026-10-19, appendix item 8 and "Q2 / Q3 / Q9 evidence")
showed what that costs:

- A send to `<pn>@s.whatsapp.net` printed `sent`, reached the phone, and created a **second chat**:
  the store held `2` chats, one `@lid` and one `@s.whatsapp.net`.
- Every incoming message stayed in the `@lid` chat, so the conversation was split in two.
- The reply from the phone quoted the message stored in the PN chat. The quote cannot be resolved
  across the split, so the TUI showed `> (a message not loaded)`.

whatsmeow already knows the pairs (all evidence from
`go.mau.fi/whatsmeow@v0.0.0-20261017175727-b2b1e71d45e2`):

- `store.LIDStore` (`store/store.go:187-193`) is exposed as `Device.LIDs` (`store/store.go:255`),
  backed by the `whatsmeow_lid_map` table inside `nuncio.db` with a cache
  (`store/sqlstore/lidmap.go:27-140`). `GetLIDForPN` / `GetPNForLID` return an empty JID when the
  pair is unknown (`lidmap.go:91-136`).
- It learns a pair from every incoming message that carries `SenderAlt` or `RecipientAlt`, before
  the message event is dispatched (`message.go:51-55`, `client.go:1081-1095`), from usync answers
  (`user.go:206-226`, `user.go:264-279`), notifications and group info. It emits no event for a
  new pair.
- A send to a PN JID looks up its LID (`GetLIDForPN`) and, when it is unknown, fetches it with
  `GetUserInfo`, which stores it; it then sends to the LID JID (`send.go:339-362`) and reports that
  JID as `SendResponse.Chat` (`send.go:131-132`, `:383`).
- `MessageSource` carries `AddressingMode`, `SenderAlt` and `RecipientAlt` (`types/message.go:14-35`).

## Decision

1. **The canonical chat of a direct contact is the JID her incoming messages use: the `@lid` JID
   whenever whatsmeow's LID map knows the pair.** The PN JID is an alias of it.
2. The daemon resolves every chat JID it is given, from a request (`messages`, `send`, `retry`,
   `search`, `react`, `mark_read`, `focus`, `download_media`) or from WhatsApp (messages,
   reactions, app-state), to the canonical chat, and stores, serves and publishes under that JID
   only. A client that named the PN JID gets replies and events with the LID JID, and is told in
   `proto/wire.md` to adopt it.
3. A send to a PN JID is stored in the canonical chat, so a reply quoting it resolves in the same
   chat. When whatsmeow learns the LID during the send itself, the message is stored in the LID
   chat and the PN chat is merged.
4. **Merge.** When a pair is known and anything is stored under the PN JID, `store.MergeChat` moves
   it into the LID chat in one SQLite transaction. It runs at daemon start over every stored PN chat
   (before the session connects or serves), and whenever a pair may have been learnt: for every
   message stored in a LID chat, at every request naming a PN JID, and around every send attempt.
   Rules:
   - Every table keyed by chat follows: `messages` (the FTS index shares their rowids, so moved
     rows stay indexed; the index row of a dropped duplicate is deleted), `message_meta`,
     `reactions`, `media_refs`, `outbox` (`outbox_replies` is keyed by `client_id` and follows it),
     `chat_mutes` and the `chats` row.
   - A message id stored in both chats is kept once: the LID chat's row, read if either copy was
     read, taking from the PN copy only what its own row lacks (sender, quoted id, media
     reference). Reactions: one per sender and message, the LID chat's wins.
   - A sender recorded as the PN JID (the contact herself) becomes the LID JID.
   - Pinned if either chat was pinned; archived only if both were (a copy in the inbox keeps the
     conversation there); the LID chat's name, or the PN chat's when it has none; muted until the
     later end (no end beats any end). When the LID chat had no row, it takes the PN chat's flags.
   - Unread counts are derived from the merged rows, so they are correct by construction.
   - `chat_aliases` records `alias -> canonical` and when.
   - Nothing that cannot be merged is deleted. Any failure rolls the whole transaction back, the
     daemon logs one line naming neither JID (only the error's type), keeps both chats as they
     are, and does not try that pair again until it restarts. Until then the PN JID is served as
     its own chat again, as when no mapping is known: requests naming it read and write the PN
     chat, so the history left there stays reachable over the wire. A second merge of the same pair finds
     nothing to move and writes nothing.
5. **No mapping, no change.** When whatsmeow does not know the pair, every chat stays as named and
   the daemon behaves, frame for frame, as before. Nothing is guessed from names, numbers or
   message content.

## Alternatives considered

- **`SendResponse.Chat` only.** It tells which JID a send went to, so the send could be stored in
  the LID chat. But it covers only sends made after the fix: it cannot merge the split stores that
  already exist, it does not resolve requests (`messages` or `mark_read` by PN JID) or app-state
  events named by PN, and whatsmeow itself documents the neighbouring `Sender` field as "not
  reliable in all cases". Changing `Client.SendMessage` would also touch every send path for a
  partial answer. Rejected as the only source; whatsmeow's own routing makes it equal to the LID
  map's answer anyway.
- **A mapping table of our own.** nuncio could record PN ↔ LID pairs itself. It would duplicate
  `whatsmeow_lid_map`, which lives in the same database and is already kept current from sources
  nuncio never sees (usync, notifications). Two copies drift: a recycled phone number that
  whatsmeow remaps (`lidmap.go:244-260` deletes the old pair) would keep merging into the old
  contact's chat. Rejected. `chat_aliases` is only a record of merges done; nothing resolves a JID
  through it, and the wire lists a recorded alias only after whatsmeow's map confirms it.
- **whatsmeow's LID store (chosen).** One source of truth, already persisted and cached, read
  through two small calls (`GetLIDForPN`, `GetPNForLID`) behind an optional `wa.LIDMapper`
  interface, so a client without it (tests, a future backend) keeps today's behaviour.

## Consequences

- One conversation per contact; quotes resolve across the old split.
- Clients see the LID JID for a contact even if they named her by phone number; `proto/wire.md`
  says how to treat it.
- The wire's Chat gains one optional field, `aliases` (protocol version unchanged): the phone-number
  JIDs merged into a LID chat, read from `chat_aliases` but listed only while whatsmeow's LID map
  still points each number to that chat (no LID map, an unknown pair, a remapped number or a merge
  that failed in this session lists nothing). `chats` serves it, and the `event_chat` published
  after every successful merge (at start, for an incoming message, around a send, for a request)
  carries it. A client adopts the LID chat from it for everything it holds under the number: the
  pairing of a request with its reply alone could not cover a merge done while the client was
  disconnected (the selection could fall to another chat with a draft written) or a live merge
  (a ghost phone-number entry, replies missing the open chat). Frames without aliases are byte for
  byte as before; one fixture is added (`event_chat_aliases.json`).
- One extra cached lookup and one cheap existence check per message stored in a LID chat and per
  request naming a PN JID.
- Messages of a PN chat whose LID whatsmeow never learns stay where they are, as before.
- `chat_aliases` is additive; a binary from before it ignores it and reads a merged database
  normally (tested in `internal/store/aliases_test.go`).

## Migration

No migration step: the schema only adds `chat_aliases` (`CREATE TABLE IF NOT EXISTS`). The first
daemon start with this change merges every stored PN chat whose LID whatsmeow knows; the rest are
merged when their pair is learnt. Downgrading is safe: an older binary opens the merged database,
ignores `chat_aliases`, and finds every message in the LID chat.
