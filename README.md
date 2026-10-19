# nuncio

A native WhatsApp client for [Omarchy](https://omarchy.org): a terminal UI
(ratatui) and a desktop app (GTK 4 + libadwaita) sharing one client library, over
a Go daemon that owns the linked-device session.

**Status: M0 (spikes).** Nothing is usable yet, and no WhatsApp account has been
linked in development. See `odd/tasks/nuncio-m0.md` for the plan of record.

## Licence

GPL-3.0-only. This is forced, not chosen: `whatsmeow`'s Signal-protocol layer
(`go.mau.fi/libsignal` → `tulir/libsignal-protocol-go`) is GPL-3.0, so any client
built on it is a combined GPL-3.0 work. `whatsmeow` itself is MPL-2.0 and its own
files stay MPL-2.0; see `CREDITS.md` (lands with M1).

## Layout

```
cmd/nunciod/          Go daemon: owns the one linked-device session
internal/ipc/         NDJSON framing + wire types
internal/store/       SQLite store + keyring-backed device container
crates/nuncio-client/ shared Rust client: IPC, models, Omarchy theme following
crates/nuncio-tui/    ratatui terminal client
crates/nuncio-desktop/ GTK 4 + libadwaita desktop client
proto/                the wire contract (wire.md) + shared fixtures
odd/tasks/            the build plan of record
docs/ contrib/ packaging/
```

`internal/wa/` (the whatsmeow integration) and `internal/media/` land with M1.

## Wire protocol

`proto/wire.md` is the contract; `proto/fixtures/*.json` are read by **both**
sides' tests, so the Go daemon and the Rust clients cannot drift apart silently.

## CLI

`nuncio` is a thin client of the running daemon. It has no automation surface: `send` takes
exactly one recipient and one message per invocation and never reads stdin; `chats` and `status`
only read.

```
nuncio status                 print the daemon's session status (also: nuncio alone)
nuncio chats                  list the chats: name, unread count, pinned/archived, last message time
nuncio send <jid> <message>   send one message to one chat; quote the message, it is one argument
nuncio focus <jid>            ask the running UI to open that chat
nuncio link --qr              link by scanning a QR code (needs --consent)
nuncio link --phone N         link with an 8-digit pairing code (needs --consent)
nuncio logout --confirm       unlink this device from WhatsApp, delete all local data and keys, stop nunciod
nuncio purge --confirm        the same without contacting WhatsApp, for a device already removed on the phone
```

- `chats` never prints message text; a chat without a name shows only the last four digits of
  its number.
- `send` prints `sent`, `queued`, `not sent: <reason>` or, when WhatsApp did not acknowledge in
  time, `not confirmed: …; it may have been delivered`, and exits 1 unless the message was sent
  or queued. With no linked device it says so and exits 1. Until the offline outbox lands, a send
  while the session is offline is `not sent`, never silently dropped.
- `focus` prints how many running UIs got the request; 0 means no UI is running, and that is not
  an error.
- `link` refuses to connect an already linked device while `nunciod` answers on `core.sock`: the
  daemon owns that session.
- `logout` and `purge` without `--confirm` only list what they would delete and exit 2. With it,
  the teardown runs in this order: (1) WhatsApp unlinks the device (`logout` only), (2) the
  keyring items under the service `nuncio` are deleted, (3) `nuncio.db` with its `-wal` and
  `-shm` files, the `media` cache and the `exports` in `$XDG_DATA_HOME/nuncio` are deleted (the
  directory too, when nothing else is in it), (4) `nunciod` removes its socket and exits.
  `logout` needs a running `nunciod`, because only the daemon's session may talk to WhatsApp for
  the device; when WhatsApp does not confirm, nothing is deleted. `purge` never contacts
  WhatsApp: it asks a running daemon to do the same, or deletes the local data itself when no
  daemon runs. Today no binary writes keyring items yet (whatsmeow keeps the Signal state inside
  `nuncio.db`), so step 2 finds nothing until the keyring-backed device store is wired.

## Build

```sh
go build ./cmd/nunciod
cargo build --workspace
```
