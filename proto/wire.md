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
| `send` | `v`, `op`, `jid`, `text` (string), `client_id` (string, chosen by the client) | `send_request.json` |
| `subscribe` | `v`, `op` | `subscribe_request.json` |
| `focus` | `v`, `op`, `jid` | `focus_request.json` |

### daemon → client

| `op` | Fields | Fixture |
|---|---|---|
| `hello_ok` | `v`, `op`, `daemon` (string), `session` (see below), `unread` (int) | `hello_ok_daemon.json` |
| `status` | `v`, `op`, `daemon`, `linked` (bool), `session`, `unread` (int) | `status_response.json` |
| `chats` | `v`, `op`, `chats` (array of Chat) | `chats_response.json` |
| `error` | `v`, `op`, `code` (string), `message` (string) | `error_version_mismatch.json`, `error_not_linked.json` |
| `messages` | `v`, `op`, `jid`, `messages` (array of Message, newest first) | `messages_response.json` |
| `sent` | `v`, `op`, `client_id`, `state` (`sent` or `queued`), `id` and `timestamp` (only when `sent`) | `sent_response.json`, `sent_queued_response.json` |
| `send_failed` | `v`, `op`, `client_id`, `message` (string) | `send_failed_response.json` |
| `subscribed` | `v`, `op` | `subscribed_response.json` |
| `focused` | `v`, `op`, `jid`, `delivered` (int: subscribed UIs that got the event; 0 = none running) | `focused_response.json` |

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
| `event_message` | `v`, `op`, `jid`, `message` (Message) | `event_message.json` |
| `event_connection` | `v`, `op`, `session` | `event_connection.json` |
| `event_unread` | `v`, `op`, `unread` (int, total), `jid`, `chat_unread` (int) | `event_unread.json` |
| `event_focus` | `v`, `op`, `jid` | `event_focus.json` |

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
| `last_message` | object or absent | omitted for a chat with no messages |

### Message

| Field | Type | Notes |
|---|---|---|
| `id` | string | WhatsApp message id |
| `kind` | string | `text`, `image`, `audio`, `video` or `document` |
| `text` | string | the body, or the caption of a media kind; may be empty for media |
| `from_me` | bool | |
| `timestamp` | string | RFC 3339 UTC |
| `media` | object or absent | set for the media kinds, omitted for `text` |

`media` is `{ "mime": string, "size": int (bytes), "file_name": string (may be empty) }`. The
bytes never travel over the socket: the daemon downloads them on demand into its media cache.

## Error codes

| `code` | Meaning |
|---|---|
| `version_mismatch` | requested `v` is not served |
| `bad_frame` | malformed JSON, or a frame over the size limit |
| `not_linked` | operation requires a paired device and there is none |
| `internal` | anything else; `message` is for logs, not for users |

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
| `send` while `offline` | `sent` with `state: "queued"`; drained once on reconnect |
| `send` repeating a `client_id` already accepted | the stored state of that send; never a second message |
| `subscribe` twice on one connection | `subscribed` again; events are still delivered once |
| `focus` with no subscribed UI | `focused` with `delivered: 0` |
| A subscriber that stops reading | dropped (connection closed) when an event finds its 64-frame queue full; never blocks the daemon or other clients |

A WhatsApp chat JID is one of: `digits@s.whatsapp.net` (a user), `digits@lid` (a user addressed by
its hidden id; WhatsApp uses it for direct chats too) or `digits-digits@g.us` / `digits@g.us` (a
group). Device suffixes (`user:2@...`), `status@broadcast`, broadcast lists and newsletters are not
chat JIDs.

## Contract tests

Both sides read **the same fixture files**:

- Go deserialises each fixture into its wire type, re-serialises, and compares
  the parsed JSON **semantically** (key order is not part of the contract).
- Rust does the same with `serde`.
- Each side additionally *constructs* the daemon→client fixtures and asserts
  they parse equal to the fixture, which is what proves the two languages agree
  on the shape rather than each agreeing with itself.
