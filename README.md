# nuncio

A native WhatsApp client for [Omarchy](https://omarchy.org), as a linked device.

It has four parts, all from this repository:

- **`nunciod`**: a Go daemon (on [whatsmeow](https://github.com/tulir/whatsmeow)) that owns the one
  linked-device session, keeps the messages in a local SQLite database and serves its clients over
  a Unix socket (`$XDG_RUNTIME_DIR/nuncio/core.sock`).
- **`nuncio-tui`**: a terminal client (ratatui).
- **`nuncio-desktop`**: a desktop app (GTK 4 + libadwaita).
- **`nuncio`**: a small command line for linking, status, one-off sends and wiping the device.

Both apps are thin clients of the daemon and share one Rust library (`crates/nuncio-client`).

## Status

**Pre-release. No release has been published, and nothing here is ready for an account you care
about.**

- M1 (the daemon and the CLI), M2 (the TUI) and M3 (the desktop app) are built and tested, almost
  entirely against fakes: the tests never connect to WhatsApp. The core had one real-account run
  (T15, on a spare number); the fixes it led to have not been re-checked on the real account yet.
- M4 (packaging and the Omarchy glue: everything below about Arch, systemd, the hotkey and the bar)
  builds and installs: the package is built by `makepkg`, installed with `pacman -U`, and CI checks
  every installed path against `packaging/MANIFEST`. **No release has been published yet**; until
  one is, install it from a local build with `packaging/README.md`.
- The Signal state in `nuncio.db` is sealed under a data key held in the Secret Service (T17,
  [ADR 0003](docs/adr/0003-where-the-signal-state-lives.md), see [SECURITY.md](SECURITY.md)): the
  binaries are wired to it and tested against a fake keyring only. With the real Secret Service it
  is planned until the home acceptance run.

Linking an unofficial client is against WhatsApp's Terms of Service and accounts have been banned
for it. Use a spare number, never the one that carries your two-factor authentication.

## Install on Arch / Omarchy

### From a release (once one is published)

One line, pasted in a terminal (Super + Return on Omarchy):

```sh
curl -L -o /tmp/nuncio.pkg.tar.zst https://github.com/pwn27sbx/nuncio/releases/latest/download/nuncio-x86_64.pkg.tar.zst && sudo pacman -U /tmp/nuncio.pkg.tar.zst
```

Run the same line to update; `sudo pacman -R nuncio` to uninstall. [INSTALL.md](INSTALL.md) says
how the first run links the device from the app, what the package does not do for you, and where
your data lives.

`packaging/install.sh` is the alternative for an offline install or a scripted one: it downloads
`nuncio-<version>-1-x86_64.pkg.tar.zst` and `SHA256SUMS` from the GitHub release, **checks the
package's SHA-256 and stops on a mismatch**, and runs `sudo pacman -U`. It also enables the
daemon's user unit (`--no-enable` skips that), which the app does by itself on first launch
anyway. It never pipes a download into a shell and does nothing outside
Arch. `bash install.sh --uninstall` removes it, and with it the Omarchy bar widget and hotkey if
you installed them and did not change them
([contrib/omarchy/README.md](contrib/omarchy/README.md)).
Details: [packaging/README.md](packaging/README.md).

### From source

You need `base-devel`, `git`, `go` (1.26 or later), `rust` (1.92 or later) and `pkgconf`.

```sh
packaging/package-source.sh            # fills packaging/PKGBUILD.in into .local/package/
cd .local/package
makepkg --cleanbuild --force -si       # offline build, runs the tests, installs with pacman
```

Only `prepare()` touches the network (`go mod download`, `cargo fetch --locked`); the build and the
tests run offline. Then open **nuncio** from your launcher: it starts the daemon through D-Bus
activation and adds it to your login items, so there is no `systemctl` step. See
[packaging/README.md](packaging/README.md) for what the package installs and how to uninstall it.

## First run

1. Open **nuncio** from your launcher. With no device linked it opens on **Link this device to
   WhatsApp**: press **Link**, then scan the code with your phone (WhatsApp → **Settings → Linked
   devices → Link a device**). The chat list appears as soon as the phone accepts it, without a
   restart, and the app adds the daemon to your login items. `Esc` cancels and leaves nothing
   behind: no device, no data, no keyring entry. [INSTALL.md](INSTALL.md) is the same path, with
   the trouble-shooting.

2. The power-user alternative is the terminal, a client of the same daemon:
   `nuncio link --qr --consent` asks the **running** daemon for the code (open **nuncio**, or any
   other command, to start it; the daemon owns the store and the session it pairs), while
   `nuncio link --phone <E.164 number> --consent` still pairs from the command itself, so it needs
   the daemon **stopped**. Nothing links without `--consent`: without it the command prints the
   ban warning and does nothing. Both are in
   [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md#nuncio-link).

3. Both apps then open on your chats: **nuncio** (the desktop app) and **nuncio (terminal)**.

To unlink and delete everything: **Settings → This device → Unlink this device** in the app, or
`nuncio logout --confirm` (see [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md#nuncio-logout) and
[SECURITY.md](SECURITY.md#wiping-the-device)).

## CLI

`nuncio` is a thin client of the running daemon and the terminal way to do what the apps do with a
window. Linking a device is done in the app ([INSTALL.md](INSTALL.md)). Every command, what it
prints and what it exits with is in [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md).

```
nuncio status                 print the daemon's session status (also: nuncio alone)
nuncio chats                  list the chats: name, unread count, pinned/archived, last message time
nuncio send <jid> <message>   send one message to one chat; quote the message, it is one argument
nuncio retry <client_id>      send again, once, a message that ended not sent (or not confirmed)
nuncio focus <jid>            ask the running UI to open that chat
nuncio link --qr              link by scanning a QR code nunciod draws (needs --consent)
nuncio link --phone N         link with an 8-digit pairing code (needs --consent)
nuncio logout --confirm       unlink this device from WhatsApp, delete all local data, backups and keys, stop nunciod
nuncio purge --confirm        the same without contacting WhatsApp, for a device already removed on the phone
```

It has no automation surface: `send` takes exactly one recipient and one message per invocation,
`retry` exactly one client_id, and neither reads stdin; `chats` and `status` only read. Nothing
links without `--consent`.

## The terminal app

`nuncio-tui`: chat list, conversation, composer, reply, react, retry, search, pictures drawn in the
terminal (kitty, sixel, iTerm2 or half blocks), mouse support and live Omarchy colours. See
[crates/nuncio-tui/README.md](crates/nuncio-tui/README.md) for the screen, the keys and the
picture protocols.

## The desktop app

`nuncio-desktop`: the same rules in GTK 4 + libadwaita, with notifications, a background mode,
`whatsapp:` links, a command palette (F1) and settings (Ctrl+,). It follows the Omarchy theme,
font and Hyprland rounding. See [crates/nuncio-desktop/README.md](crates/nuncio-desktop/README.md)
for its layout and keys.

## Configuration

Both apps read one optional TOML file, `~/.config/nuncio/config.toml`: picture protocol,
notifications, background mode, title bar. Every key and its default:
[docs/config.md](docs/config.md).

## Omarchy integration

Theme and font following, Hyprland rounding and no title bar, the `SUPER + SHIFT + ALT + G` hotkey,
the unread badge for the bar, the D-Bus names, the systemd unit and autostart:
[INTEGRATIONS.md](INTEGRATIONS.md).

## Development

```sh
go build ./cmd/nunciod && go build ./cmd/nuncio
cargo build --workspace

TMPDIR=/tmp go test ./...               # TMPDIR=/tmp on macOS: its default temp path is too long for a Unix socket
cargo test --workspace --no-fail-fast   # nuncio-desktop needs GTK 4 + libadwaita; its smoke test skips without a display
CGO_ENABLED=0 go build ./...            # the daemon stays pure Go
packaging/check.sh                      # static checks of the packaging files
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the same checks on every push to
`main` and every pull request, plus `gofmt`, `go vet`, `go test -race`, `cargo fmt --check`,
`cargo clippy -D warnings`, `govulncheck`, `cargo deny check` ([deny.toml](deny.toml): licences
compatible with GPL-3.0, advisories, crates.io only), a `cargo check` with the oldest supported
Rust (`rust-version` in `Cargo.toml`), and the Arch package build (`makepkg`, `namcap`,
`systemd-analyze verify`; [packaging/README.md](packaging/README.md)). The Rust toolchain is pinned
in [rust-toolchain.toml](rust-toolchain.toml); rustup installs it on first use.

On a Mac, install GTK first: `brew install gtk4 libadwaita pkgconf`. The review rules for code and
agents are in [AGENTS.md](AGENTS.md); the wire protocol both sides test against is
[proto/wire.md](proto/wire.md), with shared fixtures in `proto/fixtures/`.

### Try the apps with the demo daemon

`cmd/nuncio-demo` serves the wire protocol with invented chats held in memory and has no WhatsApp
code path (no network, no database, no keyring). It refuses to start while a real `nunciod`
answers on the socket. In two terminals:

```sh
mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo go run ./cmd/nuncio-demo
XDG_RUNTIME_DIR=/tmp/nuncio-demo cargo run -p nuncio-tui        # or: cargo run -p nuncio-desktop
```

`--reply-after` sets the delay of the demo's invented reply after each send (0 turns it off).

### Layout

```
cmd/nunciod/           Go daemon: owns the one linked-device session
cmd/nuncio/            the CLI
cmd/nuncio-demo/       a fake daemon for trying the apps
internal/wa/           the whatsmeow integration: session, backoff, sends, receipts
internal/store/        SQLite store, the wipe, the keyring-held data key and the sealed device store
internal/media/        on-demand media downloads and their cache
internal/ipc/          NDJSON framing, wire types, socket and D-Bus names
crates/nuncio-client/  shared Rust client: IPC, view models, config, links, theme and font
crates/nuncio-tui/     ratatui terminal client
crates/nuncio-desktop/ GTK 4 + libadwaita desktop client
proto/                 the wire contract (wire.md) + shared fixtures
packaging/             PKGBUILD, systemd unit, desktop entries, installer
contrib/omarchy/       Hyprland hotkey and Quickshell bar widget
docs/                  config reference, the command line and ADRs
odd/tasks/             the build plan of record
```

## Documents

- [SECURITY.md](SECURITY.md): what is stored where, what never leaves the machine, ban-risk rules,
  how to report a vulnerability.
- [INSTALL.md](INSTALL.md): install, link the device from the app, update, uninstall.
- [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md): every `nuncio` command, its output and exit codes.
- [INTEGRATIONS.md](INTEGRATIONS.md): Omarchy, Hyprland, D-Bus, systemd.
- [docs/config.md](docs/config.md): the configuration file.
- [docs/adr/](docs/adr/): design decisions
  ([0001 one chat per contact](docs/adr/0001-one-chat-per-contact.md),
  [0002 chat names](docs/adr/0002-chat-names.md),
  [0003 where the Signal state lives](docs/adr/0003-where-the-signal-state-lives.md)).
- [CREDITS.md](CREDITS.md): the projects and dependencies nuncio is built on.

## Licence

GPL-3.0-only. This is forced, not chosen: whatsmeow's Signal-protocol layer
(`go.mau.fi/libsignal`) is GPL-3.0, so any client built on it is a combined GPL-3.0 work.
whatsmeow itself is MPL-2.0 and its own files stay MPL-2.0; see [CREDITS.md](CREDITS.md).
