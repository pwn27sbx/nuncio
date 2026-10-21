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
| `messages` | `v`, `op`, `jid`, `messages` (array of Message, newest first, at most `limit`; fewer when they do not all fit in one frame, see "Replies too long for one frame"); answers `messages` and `search` | `messages_response.json`, `search_response.json`, `messages_group_response.json` |
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
| `event_message` | `v`, `op`, `jid`, `message` (Message); also published when a send of mine changes state (queued, sent, not sent): a client replaces the message with the same `id` | `event_message.json`, `event_message_not_sent.json`, `event_message_image.json`, `event_message_sticker.json`, `event_message_group.json` |
| `event_connection` | `v`, `op`, `session` | `event_connection.json` |
| `event_unread` | `v`, `op`, `unread` (int, total), `jid`, `chat_unread` (int) | `event_unread.json` |
| `event_focus` | `v`, `op`, `jid` | `event_focus.json` |
| `event_chat` | `v`, `op`, `chat` (Chat): published when app-state sync from another device changes a chat's pin, archive or mute; `chat` is the chat as it now is, as `chats` would list it (its unread count and `last_message` included), so a client replaces the chat with the same `jid` or adds it (added in M2). Also published after every merge of a contact's phone-number chat into her LID chat, with `aliases` (T15, see "One chat per contact"): a client then drops or renames the entries named in `aliases`. Also published when a chat's `name` changes (see "Chat names") | `event_chat.json`, `event_chat_aliases.json`, `event_chat_phone.json` |

### `session`

A string, not a boolean, so it can grow without a protocol break:

- `"unlinked"` — no WhatsApp device is paired. Everything else still works.
- `"linked"` — paired and connected.
- `"offline"` — paired but not currently connected.

`event_connection` frames follow the session's changes in order, each one differs from the one
before, and the last one a subscriber received is always the session's current state, also when a
logout or a ban lands while a drop's `offline` is being published (the client ends on `unlinked`
or `offline` as the stop says, never on a stale frame). A change undone before it was published
(`offline` then `linked` again at once) may not be published at all.

### Chat (M0 shape)

| Field | Type | Notes |
|---|---|---|
| `jid` | string | WhatsApp JID, e.g. `15551230001@s.whatsapp.net` |
| `name` | string | display name, may be empty: a group's subject, a contact's address-book, business or push name (see "Chat names") |
| `unread` | int | >= 0 |
| `archived` | bool | |
| `pinned` | bool | |
| `muted` | bool | true while the chat's mute lasts (muted with no end, or its end still ahead). Added in M1; a client reads a missing key as `false` |
| `aliases` | array of string or absent | the phone-number JIDs (`digits@s.whatsapp.net`) the daemon merged into this chat, its contact's LID chat (T15, ADR 0001; see "One chat per contact"). Listed only while WhatsApp's LID mapping still points each number to this chat, so a recycled number is never listed on the old contact's chat; omitted when there are none, and absent from an older daemon. Served in `chats` and in every `event_chat` |
| `phone` | string or absent | the contact's phone number masked to its last four digits (`…0001`): a phone-number chat's own number, a LID chat's number from WhatsApp's LID map (ADR 0002). Absent for a group, for a LID chat whose number is not known, and from an older daemon. A client shows it when `name` is empty |
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
| `kind` | string | `text`, `image`, `audio`, `video`, `document` or `sticker` (added in T15 Part 2.5; a client that does not know `sticker`, or any later kind, treats it like the other media kinds) |
| `text` | string | the body, or the caption of a media kind; may be empty for media |
| `from_me` | bool | |
| `timestamp` | string | RFC 3339 UTC |
| `media` | object or absent | set for the media kinds (`sticker` included), omitted for `text` |
| `state` | string or absent | only on a send of mine that is not sent yet: `queued` (waiting for the session to reconnect) or `not_sent` (its one attempt failed or was not confirmed; it waits for a manual `retry`). Absent once sent and on every received message. Added in M1; a client reads a missing key as sent, and an unknown value as an unknown state |
| `client_id` | string or absent | set with `state`: the `client_id` of that send, which `retry` names |
| `reply_to` | string or absent | the id of the message this one quotes (a reply received, text or media with or without a caption, or one of mine sent with `reply_to`). Added in M2; absent means it quotes nothing |
| `reactions` | array or absent | the current reactions on this message, mine first: `{ "emoji": string, "from_me": bool }`, one per sender. Absent when there are none. Added in M2; a client reads a missing key as no reactions |
| `sender` | string or absent | only on an incoming message in a group chat: who wrote it, as the daemon names her (see "Chat names"): her address-book name (full, else first), else her business name, else her push name, a LID sender read through her phone number too (WhatsApp's LID map); else her phone number masked to its last four digits (`…0001`); else `unknown`. **Never the digits of a LID JID and never a full number.** Absent in a direct chat, on every message of mine, on a group message stored before M2 recorded senders, and in a chat's `last_message`. Read locally (whatsmeow's contact store and LID map) when the message is served, never over the network; each sender is read once per reply. At most 64 Unicode code points: a longer name is cut on a code point boundary (never inside a UTF-8 sequence) to its first 63, followed by `…`. It counts inside the frame budget below: the reply is measured with its senders before thumbnails are added. Added in M4; a client that does not know it shows what it showed before (the chat's name) |
| `thumb` | string or absent | `image` and `sticker` only: base64 (standard, padded) of the thumbnail WhatsApp embeds in the message itself (an image's JPEG thumbnail, a sticker's PNG thumbnail). Absent when the message carries none, one larger than 64 KiB (before base64) or one whose bytes are not the JPEG or PNG it is labelled, and in a chat's `last_message`. Thumbnails never push a `messages` or `search` reply over the frame limit: the reply is measured without them, and they are added newest first while each still fits in the room left (and in 384 KiB before base64 in all); one that does not fit is left out, its `width` and `height` stay. Added in T15 Part 2.5 |
| `thumb_mime` | string or absent | set with `thumb`: `image/jpeg` for an image, `image/png` for a sticker |
| `width`, `height` | int or absent | `image` and `sticker` only: the picture's size in pixels, as the message states it, from 1 to 16384; each absent when 0 (unknown) or larger, and in a chat's `last_message`. Added in T15 Part 2.5 |
| `animated` | bool or absent | `sticker` only: `true` for an animated sticker; absent otherwise. Added in T15 Part 2.5 |

A message without any of `thumb`, `thumb_mime`, `width`, `height` and `animated` (every text, audio,
video and document message, and an image or sticker that states none of them) is encoded exactly
as before they existed. They are read from the stored message, so serving them never contacts
WhatsApp; a client shows the thumbnail at once and fetches the full picture only with
`download_media`.

A reaction is never a message of its own: an incoming reaction (or one of mine made on another
device) is stored against the message it targets, replacing that sender's earlier one (an empty
emoji removes it), and the target is published again as `event_message` with its new `reactions`,
so a client replaces it. Reactions never change unread counts. A reaction that arrives before its target is kept; when the target is stored, its first `event_message` (and every later read) already carries it.

`media` is `{ "mime": string, "size": int (bytes), "file_name": string (may be empty) }`. The
bytes never travel over the socket: the daemon downloads them only when a client sends
`download_media`, into its media cache `$XDG_DATA_HOME/nuncio/media` (directory `0700`, files
`0600`), and answers `media_ready` with the path of the decrypted file, whose SHA-256 it has
checked against the one the message carries. The file's extension is the document's own file-name
extension when it is a document, archive or media extension (never an executable, script or
launcher one such as `.sh`, `.desktop`, `.jar` or `.exe`), else the usual one for its mime type
(`text/plain` → `.txt`, `image/jpeg` → `.jpg`, the Office types → `.docx`/`.xlsx`/`.pptx`, …), else
the mime subtype when it is such an extension, else none. A file an earlier daemon cached under its
old name (the raw mime subtype as extension, or no extension when that subtype was not a plain
short word) is still served without a download: it is first renamed to the current name, which is
the `path` answered. A download can take longer than an ordinary
read, so the Go client waits up to 90 s for that reply.

## Error codes

| `code` | Meaning | Fixture |
|---|---|---|
| `version_mismatch` | requested `v` is not served | `error_version_mismatch.json` |
| `bad_frame` | malformed JSON, or a frame over the size limit | |
| `not_linked` | operation requires a paired device and there is none | `error_not_linked.json` |
| `not_confirmed` | a `send` or `retry` whose acknowledgement from WhatsApp timed out while the session stayed linked (one whose connection dropped meanwhile is queued instead, see the outbox corners): it may have been delivered, so it is neither `sent` nor `send_failed`; a `retry` reuses its WhatsApp id. Added in M2 (before it, `internal` with the same `message`); a client that does not know it reads it as an unknown code | `error_not_confirmed.json` |
| `internal` | anything else; `message` is for logs, not for users | |

## D-Bus face (thin, not the data plane)

Name `io.github.pwn27sbx.nuncio` on the **session** bus, at object path
`/io/github/pwn27sbx/nuncio`, interface `io.github.pwn27sbx.nuncio`, with an activation file at
`~/.local/share/dbus-1/services/io.github.pwn27sbx.nuncio.service` (installed by us, no root).
It exposes a `Status` method, an `UnreadCount` property (`i`, read-only) and an `UnreadChanged`
signal (`i count`), and answers `org.freedesktop.DBus.Introspectable` and
`org.freedesktop.DBus.Properties` on that object. It exists for:

- **activation on demand** — a client calling the name starts the daemon;
- **single instance** — the daemon owns the name, so a second start exits;
- **unread count** — property `UnreadCount` (int) plus signal `UnreadChanged`.

`UnreadCount` is the total of incoming messages not read yet across every chat, archived and muted
ones included: the sum of the per-chat `unread` that `chats` lists, and the `unread` of `status` (0
while unlinked, and 0 once a logout or purge closed the store). It is kept current: after every
change that may move it (an incoming message stored, a chat read through `mark_read` or on another
device, a contact's phone-number chat merged into her LID chat) the daemon waits 200 ms, recounts
from the store and, only when the total differs from the last one published, sets the property
(which sends `org.freedesktop.DBus.Properties.PropertiesChanged` with the new value) and emits
`UnreadChanged(count)`. Everything that happens within those 200 ms lands in one count, so a burst
(the messages a reconnect brings in) emits at most five times a second, and an unchanged total, or a
recount the store fails, emits nothing. The bus is fed from its own goroutine: a slow or absent bus
never holds up message handling. The signal carries the total only, never a chat, a name or a
number.

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

### Replies too long for one frame

A `messages` or `search` reply is never written over the frame limit (1 MiB). The daemon measures it
without thumbnails, its senders included, against the limit less 4 KiB; when it does not fit, the
reply keeps its newest messages, as many as fit, and leaves the oldest out. Every message it keeps is
whole, except one: when the newest message alone does not fit (a text longer than a frame; WhatsApp's
own limit is far below it, and the daemon refuses a `send` over 65 536 bytes, but the store takes an
incoming text of any length), its `text` is cut on a code point boundary (never inside a UTF-8
sequence) to the longest prefix that fits, followed by `…`, as a long `sender` is, and the older
messages are left out. When even an empty `text` does not fit (a `media.file_name` longer than a
frame), its `text` becomes `…` (stays empty when it was empty) and its `file_name` is cut the same
way; a message that still does not fit leaves the reply empty. Thumbnails are then added as the
`thumb` field says, to the messages the reply kept only. A reply that fits is
exactly what it was before this rule. The request has no cursor, so `messages` cannot page past the
messages it leaves out (as it cannot past `limit`); `search` still finds each of them. A client needs
no change: a reply with fewer than `limit` messages was always valid.

### M1 corners (specified now, pinned by daemon tests as each op is served)

| Situation | Behaviour |
|---|---|
| `messages` `limit` outside 1..200 | `bad_frame`; the connection stays usable |
| `messages` for a well-formed `jid` with no stored chat | `messages` with an empty list |
| `messages` or `search` whose messages, even without thumbnails, do not fit in one frame (e.g. 200 texts of 5 KiB) | the reply keeps the newest messages that fit, as many as fit, and leaves the oldest out: it never exceeds the frame limit and the connection stays usable. See "Replies too long for one frame" |
| `jid` that is not a WhatsApp chat JID in any request | `bad_frame`; the connection stays usable |
| `send` with empty `text` (or only whitespace) | `bad_frame`; nothing is stored or sent |
| `send` with `text` over 65 536 bytes | `bad_frame`; nothing is stored or sent |
| `send` with an empty `client_id` | `bad_frame` |
| `send` while `unlinked` | `error` / `not_linked`; nothing is stored or sent |
| `send` while `offline` | `sent` with `state: "queued"`; the send is stored in the outbox (and published as `event_message` with `state: "queued"`) and nothing is sent |
| The session reconnects | one drain pass: every queued send, oldest first, gets exactly one attempt under the WhatsApp id it was queued with. Each ends `sent` (stored, published as `event_message` without `state`) or not sent: `not_sent` when WhatsApp refused it, `not_confirmed` when it was not acknowledged in time, both published with `state: "not_sent"`. A second reconnect event, or a later reconnect, never attempts a send again; when the connection drops mid-pass the rest stays queued for the next one. Nothing is retried on a timer, ever |
| `send` while `linked` | stored in the outbox, then attempted once right away, with the same outcomes as a drained send (and one more: an acknowledgement lost to a dropped connection leaves it queued, below). It never overtakes an older send: every send queued before it (a drain in progress included) gets its one attempt first, in order |
| `send` repeating a `client_id` already accepted | the stored state of that send, whatever its `jid` and `text`: `sent` (`queued`, or `sent` with its `id` and `timestamp`), `send_failed` for one WhatsApp did not accept, or the `error` / `not_confirmed` for one whose ack never came; never a second message. Only `retry` sends a not-sent message again |
| `send` that WhatsApp does not accept | `send_failed` with a short reason that names no number and quotes no text; the message stays listed with `state: "not_sent"` until a `retry`; it is never deleted |
| `send` that WhatsApp does not acknowledge in time, the session still `linked` | `error` / `not_confirmed` (M1 answered `internal`) with the message `not confirmed: WhatsApp did not acknowledge in time; it may have been delivered`; never `send_failed`, because the frame may have reached WhatsApp. The message stays listed with `state: "not_sent"`; a `retry` reuses its WhatsApp id, so it is the same message again, never a second one with a new id |
| `send` or `retry` whose outcome is unknown because the connection dropped while it waited (its ack timed out, or whatsmeow answered that the socket disconnected before the response) | `sent` with `state: "queued"`, as WhatsApp itself queues a message sent as the network goes: the send stays in the outbox under the WhatsApp id it was queued with (published as `event_message` with `state: "queued"`), and the next reconnect's one drain sends it once, in order, under that id. "Dropped while it waited" is decided when the attempt returns, from what the daemon already knows: the session is `offline` then, or it left `linked` at least once during the attempt (a `Disconnected`, or the dead-keepalive drop above) even if it has reconnected since, in which case that reconnect's drain sends it right after. Nothing waits for a drop that has not been noticed yet: a connection that died silently is still `linked` until its keepalive fails (3 minutes), so a send in that window still ends `not_confirmed`. A logout during the attempt ends `not_confirmed`. Only the attempt a `send` or `retry` makes for its caller is queued this way; a drained send that is again not acknowledged ends not confirmed, like any send not acknowledged while `linked`: its row is `not_confirmed` (a `retry` reuses its WhatsApp id), listed and published with `state: "not_sent"` as every send that is not sent, and no later drain attempts it, so a message gets at most one automatic resend. A send WhatsApp refused stays `send_failed` whatever the connection did; a lost connection is never reported as refused, since the frame may have been written: while the session still counts as `linked` it is `not_confirmed`. A send that waits its turn behind an older one whose attempt takes the session offline is not attempted on the dead connection: it is queued the same way. When the session is stopped meanwhile (stream replaced, temporary ban, outdated client) the send is answered queued and stays queued until the next daemon start's first reconnect drains it; the same holds for a send whose attempt ends while nunciod shuts down: its row is left queued and resent at the next start, under the same WhatsApp id. Duplicates: whatsmeow itself resends a pending frame when the connection comes back within 5 s, so one message can go out up to three times (the first attempt, whatsmeow's own resend, the drain), always under the same WhatsApp id; whether the recipient's phone then shows it once (dedupe by id) is still to be checked with the real account (T15 Q3) (T15 follow-up, Part 2.4) |
| `retry` of a send that ended not sent (not accepted or not confirmed) | while `linked`, one more attempt under its first WhatsApp id, answered like `send`; while `offline`, it is queued again (`sent` with `state: "queued"`) for the next drain. Two retries at once make one attempt |
| `retry` of a send that is queued or sent | its stored state, as for a repeated `client_id`; nothing is sent |
| `retry` with an empty or missing `client_id` | `bad_frame`; the connection stays usable |
| `retry` of a `client_id` the daemon never stored | `send_failed` with the message `no send with that client_id`; nothing is sent |
| `retry` while `unlinked` | `error` / `not_linked`; nothing is sent |
| `subscribe` twice on one connection | `subscribed` again; events are still delivered once |
| `focus` with no subscribed UI | `focused` with `delivered: 0` |
| `focus` with subscribed UIs | `event_focus` to every subscriber (the asking connection too, if it subscribed); `focused` with `delivered` = how many got it |
| A subscriber that stops reading | dropped (connection closed) when an event finds its 64-frame queue full; never blocks the daemon or other clients |
| Any request the daemon fails to answer because of a bug in nunciod (it panicked) | `error` / `internal` with the message `the daemon failed while answering this request, so it closed this connection`, then the connection closes (with no reply when it cannot be written safely). Only that connection: the daemon, its other connections and the WhatsApp session keep running. The log names the failure by the type of its value only, never its text |
| An event the daemon fails to write to one subscriber because of a bug in nunciod (it panicked) | that subscriber is dropped (connection closed), as one that stops reading; the other subscribers still receive the event |
| `wipe` with `unlink: true` | in this order: WhatsApp unlinks the device through the daemon's own session (never a second one); the keyring items, the database (with WAL/SHM), the media cache and the exports are deleted; the socket is removed; then `wiped` with `unlinked: true`, the connection closes and the daemon exits |
| `wipe` with `unlink: false` | the same without contacting WhatsApp; `wiped` with `unlinked: false` |
| `wipe` with `unlink: true` while `unlinked` | `error` / `not_linked`; nothing is deleted (a `wipe` with `unlink: false` deletes the data of a device that is already gone) |
| `wipe` with `unlink: true` that WhatsApp does not confirm | `error` / `internal` saying nothing was deleted; the daemon keeps serving |
| A media message arrives | stored with its `media` object and what a download needs; nothing is downloaded |
| A sticker arrives (or one of mine is synced from another device) | stored as a message of kind `sticker` with no text, its `media` object (mime `image/webp`) and what a download needs; it is never downloaded on its own. `download_media` serves it exactly like an image (same cache, file extension `.webp`) |
| Thumbnails | thumbnails come from the message itself; nuncio never downloads media on its own. `thumb`, `width`, `height` and `animated` are read from the stored message in `messages`, `search` and every `event_message`; logs never carry a thumbnail |
| `download_media` with a `jid` that is not a chat JID, or an empty or missing `id` | `bad_frame`; nothing is downloaded; the connection stays usable |
| `download_media` for an id not stored in that chat, or for a text message | `error` / `internal` with the message `no media message with that id in that chat` |
| `download_media` while `unlinked` | `error` / `not_linked`; nothing is downloaded |
| `download_media` while `offline`, not cached yet | `error` / `internal` with the message `the WhatsApp session is offline, so the media was not downloaded`; a file already cached is answered with `media_ready` while offline too |
| `download_media` for a file already cached | `media_ready` with the same `path`; nothing is downloaded again |
| `download_media` whose bytes do not match the message's SHA-256 | `error` / `internal` with the message `the downloaded media does not match its SHA-256, so nothing was saved`; nothing is cached |
| `download_media` that WhatsApp does not deliver | `error` / `internal` with the message `WhatsApp did not deliver the media`; nothing is cached |
| `download_media` when the daemon cannot write its media cache (e.g. the cache dir cannot be created) | `error` / `internal` with the message `nuncio could not write its media cache, so the media was not downloaded`; it fails before anything is downloaded when the dir cannot be created |
| `download_media` while a `wipe` runs (or after it) | `error` / `internal` with the message `nuncio is wiping its data, so the media was not downloaded`; nothing is downloaded or saved and the media cache is never recreated (the daemon exits right after the wipe) |

### M2 corners (search, reply, react, mark_read, event_chat, not_confirmed; pinned by daemon tests)

| Situation | Behaviour |
|---|---|
| `search` with an empty or whitespace-only (or missing) `query` | `bad_frame`; the connection stays usable |
| `search` with a `query` over 1 024 bytes | `bad_frame` |
| `search` `limit` outside 1..200, or a `jid` that is not a chat JID | `bad_frame` |
| `search` with FTS5 syntax in `query` (quotes, `OR`, `NEAR(`, `col:`, `*`) | the words are matched literally; never an error |
| `search` with a `query` the index cannot tokenise (only emoji, symbols or punctuation, e.g. `🎉` or `?!`) | the messages of that chat whose text contains every word as a literal substring, newest first, up to `limit`; a query with any letter or number uses the index as before (whole words, any case) |
| `search` | `messages` for that chat only, newest first, each message with its `reply_to` and `reactions`; an empty list when nothing matches or there is no store. It never contacts WhatsApp |
| `send` with `reply_to` naming a message stored in that chat | sent like any `send` (outbox, queue, drain, retry), as a WhatsApp extended text whose context quotes that message (its id, its sender, and the message: its text, or for a media message its kind, below); the queued send keeps `reply_to` through queue, drain and retry, and is listed and published with it |
| `send` with `reply_to` naming a media message (image, video, audio, document or sticker) | the quote carries that media kind with its caption, mime type and (for a document) file name, as WhatsApp shows a quoted photo or file; never its media key or path. A media message stored without its reference or mime type is quoted as its text |
| `send` with `reply_to` naming a message of mine in a chat addressed by LID (`digits@lid`) | the quote names me by my own LID, when the device knows it; in every other chat by my phone-number JID |
| `send` with `reply_to` naming no message stored in that chat | `send_failed` with the message `no message with that id in that chat`; nothing is stored or sent |
| `send` with `reply_to` repeating a `client_id` already accepted | the stored state of that send, as for any repeated `client_id`, whatever `reply_to` names (a stored message or none); never a second message |
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
| Two `mark_read` of the same chat at once (two UIs) | they run one after the other: the receipts go out once; the second finds nothing unread, sends nothing and answers `marked_read` with the total |
| `mark_read` still waiting for another `mark_read` when its 4 s bound ends | `error` / `internal` with the message `another mark_read is still running, so the chat was not marked read`; nothing is sent and the chat stays unread, so a client never sees a failure the daemon later turns into a success |
| `mark_read` of many unread messages | mark_read sends at most 50 message ids per read receipt, as consecutive receipts; the first failure stops it and the chat stays unread |
| A pin, archive or mute from app-state sync (the initial full sync included) | stored, then `event_chat` with the chat as `chats` would list it; a chat not stored yet is created and published without `last_message`. A full sync can publish many at once, and a subscriber whose queue fills is dropped and re-reads `chats` as for any event |
| App-state for a JID that is not a chat JID, or with no action | ignored; nothing is stored or published |
| A chat marked read (or unread) on another device | no `event_chat`: a read publishes `event_unread` only, an unread nothing |
| The WhatsApp connection dies silently while `linked` (a NAT timeout or a network change, no socket error) | once WhatsApp keepalive pings have failed for more than 3 minutes (whatsmeow's own threshold), `event_connection` with `session: "offline"`; the daemon drops the dead connection and reconnects with its usual backoff, then publishes `linked` again. A shorter keepalive blip changes nothing |
| a frame with an `op` this client does not know (a newer daemon) | the Rust client parses it as `DaemonFrame::Unknown`; nuncio-tui ignores an unknown event and shows an unknown reply in the footer (a send answered that way is shown as not confirmed) |

### One chat per contact (T15; ADR `docs/adr/0001-one-chat-per-contact.md`)

WhatsApp addresses a direct contact two ways: by phone number (`digits@s.whatsapp.net`) and by
hidden id (`digits@lid`), and it delivers her messages in the LID chat. The daemon stores and serves
**one chat per contact**: when WhatsApp's LID mapping for a phone number is known to the daemon, the
LID JID is the contact's chat (the *canonical* JID) and the phone-number JID is an alias of it.

| Situation | Behaviour |
|---|---|
| A request names a phone-number JID whose LID is known (`messages`, `send`, `search`, `react`, `mark_read`, `focus`, `download_media`) | served as the LID chat: the reply carries the LID JID in `jid` (`messages`, `reacted`, `marked_read`, `media_ready`, `focused`, `event_focus`), and every event about it (`event_message`, `event_unread`, `event_chat`) names the LID JID. A client that sent the phone-number JID treats the reply's `jid` as the chat it asked for: it replaces the phone-number JID it holds with the reply's, keys the conversation by it from then on, and re-reads `chats`, where the alias is no longer listed as a chat of its own |
| `send` to a phone-number JID whose LID is known | sent to, stored in and published for the LID chat; `retry` of it acts there too |
| `send` to a phone-number JID whose LID is not known yet | as before, except that WhatsApp's routing teaches the daemon the LID during the send: the sent message is then stored in the LID chat and anything stored under the number moves there (see the merge below) |
| `reply_to` across the two JIDs | resolves: a quote of a message sent to the phone number, or of one received in the LID chat, finds it in the one chat |
| A chat stored under a phone number whose LID becomes known (at daemon start, or when a message, a send or WhatsApp itself teaches the mapping) | merged into the LID chat in one transaction: its messages (an id stored in both kept once, read if either copy was read), reactions, outbox entries, media references and search index move; pinned if either was pinned, archived only if both were, muted until the later end, the LID chat's name or else the alias's. Then `event_chat` with the merged chat and the number in its `aliases`, followed by `event_unread` with the new total and the LID chat's count (a message stored in both is counted once, so the total can drop). A merge that fails changes nothing and leaves both chats listed; until the daemon restarts, the phone-number JID is then served as its own chat, as when its LID is not known |
| A chat merged into its LID chat, as clients see it | the LID chat lists the phone-number JID in `aliases`, in `chats` and in the `event_chat` published after the merge (at daemon start too, so a client that reconnects learns it from `chats`). A client holding that phone-number JID anywhere (a list entry, the open or selected chat, a draft's chat, loaded messages, pending requests) moves it to the LID chat: it drops the phone-number entry (or renames it when the LID chat is not listed), keys everything by the LID JID, and a selection or draft on the number follows it. It never adopts a number that the same `chats` list also lists as a chat of its own. The reply to a request naming the number (its `jid`) is a second source. When a `chats` list no longer has the chat a client has open and neither `aliases` nor a reply explains it, the client never moves a draft to another chat (nuncio-tui keeps the vanished chat selected and blocks sending until the user chooses a chat again) |
| A phone number whatsmeow remaps to another LID (a recycled number), or a merge that failed | not listed in `aliases` of the old chat; the number is served as its own chat |
| The LID of a phone number is not known | exactly the behaviour before T15: the phone-number chat is its own chat; nothing is guessed, and no chat lists `aliases` |
| Groups, and a chat named by its LID | unchanged |

### Chat names (ADR `docs/adr/0002-chat-names.md`)

The daemon names each stored chat from WhatsApp and serves the name in `name`, in `chats` and in
every `event_chat`:

| Chat | `name` |
|---|---|
| A group | its subject: read once per daemon start (one joined-groups query after the first connection), and kept current from WhatsApp's group notifications (a subject change, a group joined) |
| A direct chat | the contact's address-book name (full name, else first name), else her business name, else her push name. A LID chat also reads the contact stored under its phone number (WhatsApp's LID map), field by field, so an address-book entry saved under the number beats the push name of the LID |
| None of these known | empty |

- Names are re-read at daemon start (from what the device already stores, no network), when a chat
  is created by its first message, and whenever WhatsApp reports a new push name, business name or
  address-book entry; never a network call per message. A changed name publishes `event_chat` with
  the chat as it now is.
- Only chats already stored are named: a contact or a group with nothing stored never appears in
  `chats` because of its name.
- The author of an incoming group message is named the same way in the message's `sender` (see
  Message): her name by the priority of a direct chat, else her masked number, else `unknown`. A
  client shows `sender` as the author of that message (the TUI's message header, the app's bubble
  and its accessible label) and falls back to the chat's name when it is absent. A desktop
  notification for it keeps the group's name as its title; the sender is prefixed to the body
  (`Ana: Running late`) only with `[notifications] show_preview = true`, so the default
  notification still says nothing about who wrote it.
- A client shows `name`; when it is empty, `phone` (the masked number); when that is absent too, a
  neutral placeholder. **Never the digits of a LID JID**, which are not a phone number. The daemon
  logs no name, number or JID while doing this.

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
