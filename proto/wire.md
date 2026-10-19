# nuncio wire protocol — v1

The single contract between the Go daemon (`nunciod`) and every Rust client
(`nuncio-tui`, `nuncio-desktop`). Both sides are checked against the fixture
files in `proto/fixtures/` by contract tests, so a change here fails one side's
tests until both agree.

## Transport

- `AF_UNIX` stream socket at `$XDG_RUNTIME_DIR/nuncio/core.sock`, mode `0600`.
- **Framing: NDJSON** — one JSON object per line, terminated by `\n`, UTF-8, no
  BOM. A frame longer than 1 MiB is a protocol error.
- A client that cannot connect may fall back to the D-Bus face (see below),
  which activates the daemon; it must not start a second daemon itself.
- Clients bound every read with a timeout (5 s by default in both the Go and the Rust client),
  the handshake included, so a wedged daemon cannot wedge a client. A read that hits the bound
  is an error and the client drops the connection.

## Versioning

Every frame carries `"v": 1` as an integer. The **first frame the client sends
MUST be `hello`**. If the daemon cannot serve the requested version it replies
with `error` / `code: "version_mismatch"` and closes the connection.

## Field naming

All keys are `snake_case` in both languages. Timestamps are RFC 3339 in UTC with
a `Z` suffix (`2026-10-18T17:55:04Z`).

## Frames

### client → daemon

| `op` | Fields | Fixture |
|---|---|---|
| `hello` | `v`, `op`, `client` (string, e.g. `nuncio-tui/0.1.0`) | `hello_client.json` |
| `status` | `v`, `op` | `status_request.json` |
| `chats` | `v`, `op`, `limit` (int, 1..200) | `chats_request.json` |
| `messages` | `v`, `op`, `jid`, `limit` (int, 1..200) | `messages_request.json` |
| `send` | `v`, `op`, `jid`, `text` (string), `client_id` (string, chosen by the client), `reply_to` (string or absent: the id of a message of that chat this send quotes; added in M2) | `send_request.json`, `send_reply_request.json` |
| `retry` | `v`, `op`, `client_id` (of a send that ended not sent) | `retry_request.json` |
| `subscribe` | `v`, `op` | `subscribe_request.json` |
| `focus` | `v`, `op`, `jid` | `focus_request.json` |
| `wipe` | `v`, `op`, `unlink` (bool: first tell WhatsApp to unlink the device) | `wipe_request.json` |
| `download_media` | `v`, `op`, `jid`, `id` (the message id) | `download_media_request.json` |
| `search` | `v`, `op`, `jid`, `query` (string, every word must match), `limit` (int, 1..200); answered with `messages` (added in M2) | `search_request.json` |
| `react` | `v`, `op`, `jid`, `id` (the message id), `emoji` (string; empty removes my reaction) (added in M2) | `react_request.json`, `react_remove_request.json` |
| `mark_read` | `v`, `op`, `jid`; a UI sends it when the user opens the chat, which marks the chat read here and on WhatsApp (added in M2) | `mark_read_request.json` |

### daemon → client

| `op` | Fields | Fixture |
|---|---|---|
| `hello_ok` | `v`, `op`, `daemon` (string), `session` (see below), `unread` (int) | `hello_ok_daemon.json` |
| `status` | `v`, `op`, `daemon`, `linked` (bool), `session`, `unread` (int) | `status_response.json` |
| `chats` | `v`, `op`, `chats` (array of Chat) | `chats_response.json` |
| `error` | `v`, `op`, `code` (string), `message` (string) | `error_version_mismatch.json`, `error_not_linked.json` |
| `messages` | `v`, `op`, `jid`, `messages` (array of Message, newest first); answers `messages` and `search` | `messages_response.json`, `search_response.json` |
| `sent` | `v`, `op`, `client_id`, `state` (`sent` or `queued`), `id` and `timestamp` (only when `sent`); answers `send` and `retry` | `sent_response.json`, `sent_queued_response.json` |
| `send_failed` | `v`, `op`, `client_id`, `message` (string); answers `send` and `retry` | `send_failed_response.json` |
| `subscribed` | `v`, `op` | `subscribed_response.json` |
| `focused` | `v`, `op`, `jid`, `delivered` (int: subscribed UIs that got the event; 0 = none running) | `focused_response.json` |
| `wiped` | `v`, `op`, `unlinked` (bool: WhatsApp confirmed the unlink) | `wiped_response.json` |
| `media_ready` | `v`, `op`, `jid`, `id`, `path` (string: the cached file), `mime` (string) | `media_ready_response.json` |
| `reacted` | `v`, `op`, `jid`, `id`, `emoji` (as asked; empty = removed) (added in M2) | `reacted_response.json` |
| `marked_read` | `v`, `op`, `jid`, `unread` (int: the new total unread count) (added in M2) | `marked_read_response.json` |

### Events (daemon → subscribed clients)

A connection that sent `subscribe` receives these frames, unprompted, interleaved with replies;
a connection that did not subscribe never receives them. Events published after the `subscribed`
reply are delivered after it, in the order they were published.

The daemon holds at most **64 event frames** per subscriber that it has not yet written to the
socket. When an event is published while that queue is full, the subscriber is dropped: the
daemon closes its connection (the client reads the frames already sent, then end of stream) and
never waits for it. The close can cut the frame being written, so a dropped subscriber's last line
may be partial: clients treat a final line without its `\n` as end of stream, not as a malformed
frame. A client that is dropped reconnects, subscribes again and re-reads the state it shows,
because events were lost; to avoid it, a UI drains every queued event on each tick. Publishing never blocks the daemon or the other connections.

| `op` | Fields | Fixture |
|---|---|---|
| `event_message` | `v`, `op`, `jid`, `message` (Message); also published when a send of mine changes state (queued, sent, not sent): a client replaces the message with the same `id` | `event_message.json`, `event_message_not_sent.json` |
| `event_connection` | `v`, `op`, `session` | `event_connection.json` |
| `event_unread` | `v`, `op`, `unread` (int, total), `jid`, `chat_unread` (int) | `event_unread.json` |
| `event_focus` | `v`, `op`, `jid` | `event_focus.json` |
| `event_chat` | `v`, `op`, `chat` (Chat): published when app-state sync from another device changes a chat's pin, archive or mute; `chat` is the chat as it now is, as `chats` would list it (its unread count and `last_message` included), so a client replaces the chat with the same `jid` or adds it (added in M2) | `event_chat.json` |

### `session`

A string, not a boolean, so it can grow without a protocol break:

- `"unlinked"` — no WhatsApp device is paired. Everything else still works.
- `"linked"` — paired and connected.
- `"offline"` — paired but not currently connected.

### Chat (M0 shape)

| Field | Type | Notes |
|---|---|---|
| `jid` | string | WhatsApp JID, e.g. `15551230001@s.whatsapp.net` |
| `name` | string | display name, may be empty |
| `unread` | int | >= 0 |
| `archived` | bool | |
| `pinned` | bool | |
| `muted` | bool | true while the chat's mute lasts (muted with no end, or its end still ahead). Added in M1; a client reads a missing key as `false` |
| `last_message` | object or absent | omitted for a chat with no messages |

`archived`, `pinned` and `muted` follow the phone: the daemon applies WhatsApp's app-state sync
(the initial full sync and every later change) read-only and never sends a change back. Each pin,
archive or mute it applies publishes `event_chat` with the chat as it now is (added in M2; before
it, a client saw the change on its next `chats`). A chat marked read on another device
is marked read here too (only its `event_unread` is published, never `event_chat`, and no read
receipt is sent: only `mark_read` sends one); a chat
marked *unread* on another device leaves the counts as they are, since the daemon has no "marked
unread" flag.

### Message

`messages` lists a chat's stored messages together with the sends of mine to that chat that are
queued or not sent (with `state` and `client_id`, timestamped when they were queued); a sent one is
listed once, without `state`. A send's `id` is the WhatsApp message id the daemon chose when it was
queued, and every attempt of that send uses it, so it stays the same across states and retries.

| Field | Type | Notes |
|---|---|---|
| `id` | string | WhatsApp message id |
| `kind` | string | `text`, `image`, `audio`, `video` or `document` |
| `text` | string | the body, or the caption of a media kind; may be empty for media |
| `from_me` | bool | |
| `timestamp` | string | RFC 3339 UTC |
| `media` | object or absent | set for the media kinds, omitted for `text` |
| `state` | string or absent | only on a send of mine that is not sent yet: `queued` (waiting for the session to reconnect) or `not_sent` (its one attempt failed or was not confirmed; it waits for a manual `retry`). Absent once sent and on every received message. Added in M1; a client reads a missing key as sent, and an unknown value as an unknown state |
| `client_id` | string or absent | set with `state`: the `client_id` of that send, which `retry` names |
| `reply_to` | string or absent | the id of the message this one quotes (a reply received, or one of mine sent with `reply_to`). Added in M2; absent means it quotes nothing |
| `reactions` | array or absent | the current reactions on this message, mine first: `{ "emoji": string, "from_me": bool }`, one per sender. Absent when there are none. Added in M2; a client reads a missing key as no reactions |

A reaction is never a message of its own: an incoming reaction (or one of mine made on another
device) is stored against the message it targets, replacing that sender's earlier one (an empty
emoji removes it), and the target is published again as `event_message` with its new `reactions`,
so a client replaces it. Reactions never change unread counts.

`media` is `{ "mime": string, "size": int (bytes), "file_name": string (may be empty) }`. The
bytes never travel over the socket: the daemon downloads them only when a client sends
`download_media`, into its media cache `$XDG_DATA_HOME/nuncio/media` (directory `0700`, files
`0600`), and answers `media_ready` with the path of the decrypted file, whose SHA-256 it has
checked against the one the message carries. A download can take longer than an ordinary read, so
the Go client waits up to 90 s for that reply.

## Error codes

| `code` | Meaning | Fixture |
|---|---|---|
| `version_mismatch` | requested `v` is not served | `error_version_mismatch.json` |
| `bad_frame` | malformed JSON, or a frame over the size limit | |
| `not_linked` | operation requires a paired device and there is none | `error_not_linked.json` |
| `not_confirmed` | a `send` or `retry` whose acknowledgement from WhatsApp timed out: it may have been delivered, so it is neither `sent` nor `send_failed`; a `retry` reuses its WhatsApp id. Added in M2 (before it, `internal` with the same `message`); a client that does not know it reads it as an unknown code | `error_not_confirmed.json` |
| `internal` | anything else; `message` is for logs, not for users | |

## D-Bus face (thin, not the data plane)

Name `io.github.pwn27sbx.nuncio` on the **session** bus, at object path
`/io/github/pwn27sbx/nuncio`, interface `io.github.pwn27sbx.nuncio`, with an activation file at
`~/.local/share/dbus-1/services/io.github.pwn27sbx.nuncio.service` (installed by us, no root).
It exposes a `Status` method and an `UnreadCount` property (int). It exists for:

- **activation on demand** — a client calling the name starts the daemon;
- **single instance** — the daemon owns the name, so a second start exits;
- **unread count** — property `UnreadCount` (int) plus signal `UnreadChanged`.

Message bodies and media never travel over D-Bus.

## Behaviour the two implementations have fixed

These corners were left open by the first draft and are now pinned by tests on both sides. They
are part of the contract, not implementation detail:

| Situation | Behaviour |
|---|---|
| First client frame is not `hello` | `bad_frame`, then close |
| Any frame carries `v` other than 1 | `version_mismatch`, then close |
| Malformed JSON | `bad_frame`; the connection stays usable |
| Unknown `op` | `bad_frame`; the connection stays usable |
| A second `hello` on the same connection | `bad_frame`; the connection stays usable |
| `chats` `limit` outside 1..200 | `bad_frame`; the connection stays usable |
| Frame over 1 MiB | `bad_frame`, then close |

A frame of exactly 1 MiB is accepted; one byte more is not.

### M1 corners (specified now, pinned by daemon tests as each op is served)

| Situation | Behaviour |
|---|---|
| `messages` `limit` outside 1..200 | `bad_frame`; the connection stays usable |
| `messages` for a well-formed `jid` with no stored chat | `messages` with an empty list |
| `jid` that is not a WhatsApp chat JID in any request | `bad_frame`; the connection stays usable |
| `send` with empty `text` (or only whitespace) | `bad_frame`; nothing is stored or sent |
| `send` with `text` over 65 536 bytes | `bad_frame`; nothing is stored or sent |
| `send` with an empty `client_id` | `bad_frame` |
| `send` while `unlinked` | `error` / `not_linked`; nothing is stored or sent |
| `send` while `offline` | `sent` with `state: "queued"`; the send is stored in the outbox (and published as `event_message` with `state: "queued"`) and nothing is sent |
| The session reconnects | one drain pass: every queued send, oldest first, gets exactly one attempt under the WhatsApp id it was queued with. Each ends `sent` (stored, published as `event_message` without `state`) or `not_sent` (published with `state: "not_sent"`). A second reconnect event, or a later reconnect, never attempts a send again; when the connection drops mid-pass the rest stays queued for the next one. Nothing is retried on a timer, ever |
| `send` while `linked` | stored in the outbox, then attempted once right away, with the same outcomes as a drained send. It never overtakes an older send: every send queued before it (a drain in progress included) gets its one attempt first, in order |
| `send` repeating a `client_id` already accepted | the stored state of that send, whatever its `jid` and `text`: `sent` (`queued`, or `sent` with its `id` and `timestamp`), `send_failed` for one WhatsApp did not accept, or the `error` / `not_confirmed` for one whose ack never came; never a second message. Only `retry` sends a not-sent message again |
| `send` that WhatsApp does not accept | `send_failed` with a short reason that names no number and quotes no text; the message stays listed with `state: "not_sent"` until a `retry`; it is never deleted |
| `send` that WhatsApp does not acknowledge in time | `error` / `not_confirmed` (M1 answered `internal`) with the message `not confirmed: WhatsApp did not acknowledge in time; it may have been delivered`; never `send_failed`, because the frame may have reached WhatsApp. The message stays listed with `state: "not_sent"`; a `retry` reuses its WhatsApp id, so it is the same message again, never a second one with a new id |
| `retry` of a send that ended not sent (not accepted or not confirmed) | while `linked`, one more attempt under its first WhatsApp id, answered like `send`; while `offline`, it is queued again (`sent` with `state: "queued"`) for the next drain. Two retries at once make one attempt |
| `retry` of a send that is queued or sent | its stored state, as for a repeated `client_id`; nothing is sent |
| `retry` with an empty or missing `client_id` | `bad_frame`; the connection stays usable |
| `retry` of a `client_id` the daemon never stored | `send_failed` with the message `no send with that client_id`; nothing is sent |
| `retry` while `unlinked` | `error` / `not_linked`; nothing is sent |
| `subscribe` twice on one connection | `subscribed` again; events are still delivered once |
| `focus` with no subscribed UI | `focused` with `delivered: 0` |
| `focus` with subscribed UIs | `event_focus` to every subscriber (the asking connection too, if it subscribed); `focused` with `delivered` = how many got it |
| A subscriber that stops reading | dropped (connection closed) when an event finds its 64-frame queue full; never blocks the daemon or other clients |
| `wipe` with `unlink: true` | in this order: WhatsApp unlinks the device through the daemon's own session (never a second one); the keyring items, the database (with WAL/SHM), the media cache and the exports are deleted; the socket is removed; then `wiped` with `unlinked: true`, the connection closes and the daemon exits |
| `wipe` with `unlink: false` | the same without contacting WhatsApp; `wiped` with `unlinked: false` |
| `wipe` with `unlink: true` while `unlinked` | `error` / `not_linked`; nothing is deleted (a `wipe` with `unlink: false` deletes the data of a device that is already gone) |
| `wipe` with `unlink: true` that WhatsApp does not confirm | `error` / `internal` saying nothing was deleted; the daemon keeps serving |
| A media message arrives | stored with its `media` object and what a download needs; nothing is downloaded |
| `download_media` with a `jid` that is not a chat JID, or an empty or missing `id` | `bad_frame`; nothing is downloaded; the connection stays usable |
| `download_media` for an id not stored in that chat, or for a text message | `error` / `internal` with the message `no media message with that id in that chat` |
| `download_media` while `unlinked` | `error` / `not_linked`; nothing is downloaded |
| `download_media` while `offline`, not cached yet | `error` / `internal` with the message `the WhatsApp session is offline, so the media was not downloaded`; a file already cached is answered with `media_ready` while offline too |
| `download_media` for a file already cached | `media_ready` with the same `path`; nothing is downloaded again |
| `download_media` whose bytes do not match the message's SHA-256 | `error` / `internal` with the message `the downloaded media does not match its SHA-256, so nothing was saved`; nothing is cached |
| `download_media` that WhatsApp does not deliver | `error` / `internal` with the message `WhatsApp did not deliver the media`; nothing is cached |

### M2 corners (search, reply, react, mark_read, event_chat, not_confirmed; pinned by daemon tests)

| Situation | Behaviour |
|---|---|
| `search` with an empty or whitespace-only (or missing) `query` | `bad_frame`; the connection stays usable |
| `search` with a `query` over 1 024 bytes | `bad_frame` |
| `search` `limit` outside 1..200, or a `jid` that is not a chat JID | `bad_frame` |
| `search` with FTS5 syntax in `query` (quotes, `OR`, `NEAR(`, `col:`, `*`) | the words are matched literally; never an error |
| `search` | `messages` for that chat only, newest first, each message with its `reply_to` and `reactions`; an empty list when nothing matches or there is no store. It never contacts WhatsApp |
| `send` with `reply_to` naming a message stored in that chat | sent like any `send` (outbox, queue, drain, retry), as a WhatsApp extended text whose context quotes that message (its id, its sender, its text); the queued send keeps `reply_to` through queue, drain and retry, and is listed and published with it |
| `send` with `reply_to` naming no message stored in that chat | `send_failed` with the message `no message with that id in that chat`; nothing is stored or sent |
| `send` without `reply_to` | exactly the M1 behaviour: a plain text message |
| `react` with a `jid` that is not a chat JID, an empty or missing `id`, or an `emoji` over 64 bytes | `bad_frame`; nothing is sent |
| `react` while `unlinked` | `error` / `not_linked` |
| `react` while `offline` | `error` / `internal` with the message `the WhatsApp session is offline, so the reaction was not sent`; reactions are never queued, so the user reacts again once linked |
| `react` on an id not stored in that chat | `error` / `internal` with the message `no message with that id in that chat`; nothing is sent |
| `react` on someone else's message whose sender is unknown (a group message stored before M2 recorded senders) | `error` / `internal` with the message `the sender of that message is unknown, so the reaction was not sent`; nothing is sent or stored, because a reaction key without its sender would claim the message is mine |
| `react` that WhatsApp does not accept or acknowledge | `error` / `internal` with the message `WhatsApp did not accept the reaction`; one attempt, never retried |
| `react` accepted | `reacted`; my reaction is stored on the message and the message is published again as `event_message` |
| `mark_read` | the explicit view of AGENTS.md ("read receipts only on an explicit view"): a UI sends it when the user opens the chat, and it is the only thing that sends read receipts. The daemon sends WhatsApp one read receipt per sender for the chat's unread messages that are not mine (as the official clients do, so every device and the sender see them read), then marks them read in its store, publishes `event_unread` (the total and `chat_unread`, 0 unless a message arrived meanwhile) and answers `marked_read` with the new total |
| `mark_read` while the user turned read receipts off in WhatsApp's privacy settings | the receipts are `read-self`: the user's other devices mark the chat read, the sender sees no blue ticks |
| `mark_read` of a group message stored before M2 recorded senders | no receipt for it, since a group receipt must name the sender; it is still marked read in the store |
| `mark_read` of a chat with nothing unread (or no stored chat) | `marked_read` with the total; nothing is sent and nothing is published, while offline too |
| `mark_read` with a `jid` that is not a chat JID | `bad_frame`; nothing is sent |
| `mark_read` while `unlinked` | `error` / `not_linked`; the chat stays unread |
| `mark_read` while `offline` | `error` / `internal` with the message `the WhatsApp session is offline, so the chat was not marked read`; receipts are never queued, so the chat stays unread and the next view marks it read |
| `mark_read` when the daemon cannot read the read-receipt privacy setting | `error` / `internal` with the message `could not read the read-receipt privacy setting, so the chat was not marked read`; nothing is sent (a receipt could show blue ticks the user turned off) and the chat stays unread |
| `mark_read` whose receipt WhatsApp does not take | `error` / `internal` with the message `WhatsApp did not accept the read receipt, so the chat was not marked read`; no further receipt is sent, nothing is retried and the chat stays unread here (a later `mark_read` sends its receipts again, which WhatsApp reads as the same read) |
| `mark_read` timing | the daemon answers within 4 s, inside a client's ordinary 5 s read bound |
| `mark_read` of many unread messages | mark_read sends at most 50 message ids per read receipt, as consecutive receipts; the first failure stops it and the chat stays unread |
| A pin, archive or mute from app-state sync (the initial full sync included) | stored, then `event_chat` with the chat as `chats` would list it; a chat not stored yet is created and published without `last_message`. A full sync can publish many at once, and a subscriber whose queue fills is dropped and re-reads `chats` as for any event |
| App-state for a JID that is not a chat JID, or with no action | ignored; nothing is stored or published |
| A chat marked read (or unread) on another device | no `event_chat`: a read publishes `event_unread` only, an unread nothing |

A WhatsApp chat JID is one of: `digits@s.whatsapp.net` (a user), `digits@lid` (a user addressed by
its hidden id; WhatsApp uses it for direct chats too) or `digits-digits@g.us` / `digits@g.us` (a
group; the hyphen only between two runs of digits). Device suffixes (`user:2@...`), `status@broadcast`, broadcast lists and newsletters are not
chat JIDs.

## Contract tests

Both sides read **the same fixture files**:

- Go deserialises each fixture into its wire type, re-serialises, and compares
  the parsed JSON **semantically** (key order is not part of the contract).
- Rust does the same with `serde`.
- Each side additionally *constructs* the daemon→client fixtures and asserts
  they parse equal to the fixture, which is what proves the two languages agree
  on the shape rather than each agreeing with itself.
