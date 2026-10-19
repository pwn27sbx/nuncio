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

### daemon → client

| `op` | Fields | Fixture |
|---|---|---|
| `hello_ok` | `v`, `op`, `daemon` (string), `session` (see below), `unread` (int) | `hello_ok_daemon.json` |
| `status` | `v`, `op`, `daemon`, `linked` (bool), `session`, `unread` (int) | `status_response.json` |
| `chats` | `v`, `op`, `chats` (array of Chat) | `chats_response.json` |
| `error` | `v`, `op`, `code` (string), `message` (string) | `error_version_mismatch.json` |

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

### Message (M0 shape)

| Field | Type | Notes |
|---|---|---|
| `id` | string | WhatsApp message id |
| `kind` | string | `text` for M0; media kinds land in M1 |
| `text` | string | may be empty for non-text kinds |
| `from_me` | bool | |
| `timestamp` | string | RFC 3339 UTC |

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

## Contract tests

Both sides read **the same fixture files**:

- Go deserialises each fixture into its wire type, re-serialises, and compares
  the parsed JSON **semantically** (key order is not part of the contract).
- Rust does the same with `serde`.
- Each side additionally *constructs* the daemon→client fixtures and asserts
  they parse equal to the fixture, which is what proves the two languages agree
  on the shape rather than each agreeing with itself.
