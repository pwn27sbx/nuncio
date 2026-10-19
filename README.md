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

## Build

```sh
go build ./cmd/nunciod
cargo build --workspace
```
