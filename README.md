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
  is written but has **never been built or installed on Linux**. Acceptance on Arch/Omarchy is
  pending.
- The Signal state in `nuncio.db` is sealed under a data key held in the Secret Service (T17,
  [ADR 0003](docs/adr/0003-where-the-signal-state-lives.md), see [SECURITY.md](SECURITY.md)): the
  binaries are wired to it and tested against a fake keyring only. With the real Secret Service it
  is planned until the home acceptance run.

Linking an unofficial client is against WhatsApp's Terms of Service and accounts have been banned
for it. Use a spare number, never the one that carries your two-factor authentication.

## Install on Arch / Omarchy

### From a release (once one is published)

```sh
curl -fLO https://github.com/pwn27sbx/nuncio/raw/v<version>/packaging/install.sh
less install.sh                         # read it first
bash install.sh --version <version>
```

The installer prints its plan and asks first, downloads `nuncio-<version>-1-x86_64.pkg.tar.zst`
and `SHA256SUMS` from the GitHub release, **checks the package's SHA-256 and stops on a mismatch**,
runs `sudo pacman -U` and enables the daemon's user unit (`--no-enable` skips that). It never pipes
a download into a shell and does nothing outside Arch. `bash install.sh --uninstall` removes it,
and with it the Omarchy bar widget and hotkey if you installed them and did not change them
([contrib/omarchy/README.md](contrib/omarchy/README.md)).
Details: [packaging/README.md](packaging/README.md).

### From source

You need `base-devel`, `git`, `go` (1.26 or later), `rust` (1.92 or later) and `pkgconf`.

```sh
packaging/package-source.sh            # fills packaging/PKGBUILD.in into .local/package/
cd .local/package
makepkg --cleanbuild --force -si       # offline build, runs the tests, installs with pacman
systemctl --user enable --now nunciod.service
```

Only `prepare()` touches the network (`go mod download`, `cargo fetch --locked`); the build and the
tests run offline. See [packaging/README.md](packaging/README.md) for what the package installs and
how to uninstall it.

## First run

1. Link this machine as a device. The daemon must not be running while you link, and nothing links
   without `--consent` (without it the command prints the ban warning and does nothing):

   ```sh
   systemctl --user stop nunciod
   nuncio link --qr --consent              # or: nuncio link --phone <E.164 number> --consent
   ```

   `--qr` draws a QR code in the terminal for Settings → Linked devices → Link a device;
   `--phone` prints an 8-digit pairing code to enter on the phone instead. Once the phone accepts
   it, the command prints `nuncio: linked as …` (the number masked to its last four digits), says
   how to start the daemon, and exits. It never sends a message.

2. Start the daemon again and check it:

   ```sh
   systemctl --user start nunciod
   nuncio status
   nuncio chats
   ```

3. Open `nuncio-tui` in a terminal or `nuncio-desktop` (the launcher entry "nuncio").

To unlink and delete everything: `nuncio logout --confirm` (see [CLI](#cli) and
[SECURITY.md](SECURITY.md#wiping-the-device)).

## CLI

`nuncio` is a thin client of the running daemon (`status`, `chats`, `send`, `retry` and `focus`
start it through D-Bus activation when it is not running). It has no automation surface: `send`
takes exactly one recipient and one message per invocation, `retry` exactly one client_id, and
neither reads stdin; `chats` and `status` only read.

```
nuncio status                 print the daemon's session status (also: nuncio alone)
nuncio chats                  list the chats: name, unread count, pinned/archived, last message time
nuncio send <jid> <message>   send one message to one chat; quote the message, it is one argument
nuncio retry <client_id>      send again, once, a message that ended not sent (or not confirmed)
nuncio focus <jid>            ask the running UI to open that chat
nuncio link --qr              link by scanning a QR code (needs --consent)
nuncio link --phone N         link with an 8-digit pairing code (needs --consent)
nuncio logout --confirm       unlink this device from WhatsApp, delete all local data, backups and keys, stop nunciod
nuncio purge --confirm        the same without contacting WhatsApp, for a device already removed on the phone
```

A `<jid>` is a WhatsApp chat: `15551230001@s.whatsapp.net` (a person) or
`120363000000000000@g.us` (a group).

- `chats` never prints message text. A chat is shown by its name (a group's subject, else the
  contact's address-book, business or push name); without one, by its number masked to the last
  four digits (`…0001`), or `hidden number` when the number is not known (a LID chat): a LID's
  digits are never printed. The TUI does the same, and shows `unnamed group` for a group with no
  subject ([ADR 0002](docs/adr/0002-chat-names.md)).
- `send` prints `sent`, `queued`, `not sent: <reason>` or, when WhatsApp did not acknowledge in
  time, `not confirmed: …; it may have been delivered`, and exits 1 unless the message was sent or
  queued. With no linked device it says so and exits 1. While the session is offline, or when the
  connection drops before WhatsApp acknowledges the message, it is queued and sent once when the
  session reconnects (one pass, oldest first, one second apart, same message id). A connection
  that dies silently still counts as linked for up to 3 minutes, until its keepalive fails, so a
  send in that window ends `not confirmed` rather than queued. A send whose answer from nunciod
  does not come in time ends not confirmed too, in the CLI and in both apps, never not sent:
  nunciod may already have handed it to WhatsApp. Nothing else is ever retried on
  its own: a message that ends `not sent` or `not confirmed` stays listed as not sent, and `send`
  prints its client_id.
- `retry <client_id>` sends that one message again, once, under the same WhatsApp message id, so it
  is the same message again, never a second one with a new id; a message that is queued or sent is
  only reported.
- `focus` prints how many running UIs got the request; 0 means no UI is running, and that is not
  an error.
- `link` exits as soon as the device is linked, and never sends a message. Run on a device that is
  already linked, it connects it once to check that WhatsApp still accepts it, reports it and
  exits. It refuses to run at all while `nunciod` answers on `core.sock`, before it opens the
  database: the daemon owns that session. `link` needs the keyring (Secret Service) unlocked: it
  creates the data key only when pairing saves the new device, reads it back before the device is
  saved, and a link that never pairs leaves no key and no device behind. A `nuncio.db` an older
  build left in plain is backed up (`nuncio.db.pre-t17-<UTC time>.bak`, which still holds the keys
  in plain: delete it once nuncio works) and sealed first, by `link` or by `nunciod` at start.
- With the keyring locked or absent, `nunciod` stays up and answers `offline`, never connects to
  WhatsApp and writes nothing; it tries again on the session's backoff. When the data key is gone
  but a sealed device is stored, nothing creates a new one: the device cannot be read, and the way
  out is `nuncio purge --confirm`, then `nuncio link`.
- `logout` and `purge` without `--confirm` only list what they would delete and exit 2. With it,
  the teardown runs in this order: (1) WhatsApp unlinks the device (`logout` only), (2) the
  keyring items under the service `nuncio` are deleted (the data key `store.data_key` among them),
  (3) `nuncio.db` with its `-wal` and `-shm` files, every migration backup
  (`nuncio.db.pre-t17-*.bak`), the `media` cache and the `exports` in `$XDG_DATA_HOME/nuncio` are
  deleted (the
  directory too, when nothing else is in it and the sandbox allows it), (4) `nunciod` removes its
  socket and exits. `logout` needs a running `nunciod`, because only the daemon's session may talk
  to WhatsApp for the device; when WhatsApp does not confirm, nothing is deleted. `purge` never
  contacts WhatsApp: it asks a running daemon to do the same, or deletes the local data itself when
  no daemon runs.

One chat per contact: a contact's phone-number chat is merged into her hidden-id (`@lid`) chat
when whatsmeow knows the pair ([ADR 0001](docs/adr/0001-one-chat-per-contact.md)).

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
docs/                  config reference and ADRs
odd/tasks/             the build plan of record
```

## Documents

- [SECURITY.md](SECURITY.md): what is stored where, what never leaves the machine, ban-risk rules,
  how to report a vulnerability.
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
