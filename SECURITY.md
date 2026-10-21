# Security

nuncio is pre-release software (see the status in [README.md](README.md#status)). This file says
what it protects, where it keeps your data, what it never does, and how to report a problem.

## Threat model, in short

nuncio runs as a linked WhatsApp device on one Linux machine, for one user. It aims to protect:

- **Your messages and keys on disk** from other local users: everything is private to your user
  (directories `0700`, files `0600`).
- **Your messages from leaving the machine** by any path other than WhatsApp itself: no telemetry,
  no cloud, no message text in logs, notifications or on D-Bus unless you turn previews on.
- **Your account from a ban** caused by the client: no automation, no retry loops, no presence.

It does **not** protect against malware or anyone running as your user, against root, or against a
stolen machine without disk encryption: today the Signal session state sits in an ordinary SQLite
file (see below). Using an unofficial client is against WhatsApp's Terms of Service; nuncio reduces
the ban risk, it cannot remove it.

## What is stored where

| What | Where | Protection |
| --- | --- | --- |
| Messages, chats, reactions, the outbox, the search index | `$XDG_DATA_HOME/nuncio/nuncio.db` (default `~/.local/share/nuncio/`), with its `-wal` and `-shm` files | directory `0700`, files `0600` (an existing looser file is tightened when it is opened) |
| **The WhatsApp device's Signal state** (identity and session keys, pre-keys, sender keys, app-state keys) | **inside the same `nuncio.db`**, in whatsmeow's own tables | `0600` only, **not encrypted**. Moving it behind the keyring (Secret Service) is planned (T17) and not done: the keyring-backed device store exists in `internal/store/device.go` but no binary writes keyring items yet |
| Media you open (photos, stickers, documents) | `$XDG_DATA_HOME/nuncio/media/` | directory `0700`, files `0600`; downloaded only on demand, never on their own, and checked against WhatsApp's SHA-256 |
| `exports` | `$XDG_DATA_HOME/nuncio/exports/` | reserved for chat exports (no command writes it yet); deleted by the wipe |
| The socket the apps talk to | `$XDG_RUNTIME_DIR/nuncio/core.sock` | directory `0700`, socket `0600` |
| Settings | `~/.config/nuncio/config.toml` | written by nuncio-desktop's settings with mode `0600`, its directory created `0700` ([docs/config.md](docs/config.md)) |

Never copy `~/.local/share/nuncio/` to another machine or into a bug report: it holds the device's
keys as long as T17 is open.

## What never leaves the machine or reaches a log

- **Message text** is never written to a log. Notifications carry it only with
  `[notifications] show_preview = true` (off by default; otherwise "New message", "Photo", …).
  The daemon's D-Bus face carries only `Status` and the `UnreadCount` number: no message bodies, no
  media, no chat names.
- **Full phone numbers** are never printed or logged: the CLI and the apps mask a number to its last
  four digits (`…0001`), and a hidden id's (LID) digits are never shown.
- **Key material** is never logged or exported; whatsmeow's own logger is off (`nil`).
- nuncio talks to WhatsApp's servers (through whatsmeow) and to nothing else: no telemetry, no
  update check, no link previews.

These are review rules for every change ([AGENTS.md](AGENTS.md)); tests never connect to WhatsApp
and never touch a real keyring.

## Ban-risk rules

- **One message per explicit action.** `nuncio send` takes exactly one recipient and one message
  and never reads stdin; the apps send only when you press Enter. A `whatsapp:` or `wa.me` link
  only fills in the composer, it never sends.
- **No automatic retries,** with one exception: a message that could not go out because the session
  was offline, or whose connection dropped before WhatsApp acknowledged it, is queued and sent
  **once** when the session reconnects (one pass, oldest first, under the same message id). A
  message that ends `not sent` or `not confirmed` stays visible and is sent again only by an
  explicit `retry`, under the same id. (Whether whatsmeow's own frame resend on a quick reconnect
  can repeat a frame is still to be checked on the real account.)
- **No reconnect storms.** whatsmeow's own unbounded reconnect loop is off; nuncio uses capped
  exponential backoff with jitter (2 s base, 5 min cap), and stops for good after a logout, a ban,
  a replaced stream or a refused connection. The systemd unit restarts the daemon only after a
  crash, 30 s later and doubling up to 10 min, at most 4 starts in 30 min.
- **No presence.** nuncio never sends "available" (it never calls whatsmeow's `SendPresence`).
- **Read receipts only on an explicit view:** when you open a chat in an app (or a new message
  arrives in the chat you have open and can see), and only as your WhatsApp read-receipt privacy
  setting allows. Read receipts are never queued while offline.
- **Linking needs consent.** `nuncio link` does nothing without `--consent` and prints the ban
  warning. Note that `nuncio link` is still the first-milestone check: once linked it waits for one
  message from a one-to-one chat and sends one fixed reply to it ([README.md](README.md#first-run)).

## The installer

`packaging/install.sh` downloads only the package and the release's `SHA256SUMS`, **checks the
package's SHA-256 against its line and stops on a mismatch**, then hands the file to
`sudo pacman -U`. It never pipes a download into a shell, refuses without a terminal unless
`--yes`, refuses on anything that is not Arch, and calls `sudo` for pacman only. The checksum list
comes from the same release as the package, so it proves the download is intact, not who made it.
Read the script before running it. See [packaging/README.md](packaging/README.md).

## The systemd unit

`nunciod.service` (a `systemd --user` unit) runs the daemon sandboxed: the file system read-only
except the data directory and its runtime directory (`ProtectSystem=strict`, `ReadWritePaths=`),
`NoNewPrivileges`, `UMask=0077`, a private `/tmp`, no devices, only Unix and IP sockets, no new
namespaces, no writable-and-executable memory, the `@system-service` system-call filter, and the
kernel, clock and cgroup knobs protected. Each setting is commented in
[packaging/nunciod.service](packaging/nunciod.service); `systemd-analyze --user security nunciod`
scores it (still to run on Linux).

## Wiping the device

- `nuncio logout --confirm`: WhatsApp unlinks the device first (through the running daemon); then
  the keyring items, `nuncio.db` with `-wal`/`-shm`, the media cache and the exports are deleted
  and the daemon stops. If WhatsApp does not confirm, nothing is deleted.
- `nuncio purge --confirm`: the same without contacting WhatsApp, for a device already removed
  under Linked devices on the phone.
- Without `--confirm` both only list what they would delete. Under the systemd sandbox the emptied
  data directory itself may stay; it holds nothing.

Uninstalling the package does not delete your data: run `nuncio purge --confirm` (or `logout`)
first, and remove the device under Linked devices on your phone.

## Reporting a vulnerability

Please report it privately: **open a GitHub security advisory on the repository**
(`pwn27sbx/nuncio`). Do not open a public issue, and never
attach a database, keyring dump, log with numbers, or anything from `~/.local/share/nuncio/`.
