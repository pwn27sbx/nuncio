# The nuncio command line

`nuncio` is a thin client of the running daemon. Everything that touches WhatsApp belongs to
`nunciod`, which owns the one linked session; the terminal and desktop apps read the same socket.
Linking a device in the app, and installing or removing the package, is in
[INSTALL.md](../INSTALL.md): this page is the reference for the commands, what each one prints and
what it exits with.

Every command exits `0` on success, `1` when it failed, and `2` on a usage error (the wrong
arguments, or the confirmation flag it needs and did not get).

`status`, `chats`, `send`, `retry` and `focus` start the daemon through the session bus when it is
not running (D-Bus activation), exactly as opening **nuncio** does, and wait for it. `link --qr`
needs it running; `logout` needs it running and never starts it; `purge` works either way. Nothing
below starts or stops a unit by hand, except the one `--phone` note further down.

There is no automation surface: `send` takes exactly one recipient and one message per invocation,
`retry` exactly one client_id, and neither reads stdin; `chats` and `status` only read.

A `<jid>` is a WhatsApp chat: `15551230001@s.whatsapp.net` (a person) or
`120363000000000000@g.us` (a group).

## `nuncio status`

`nuncio` alone is the same command.

```
linked=true session=linked unread=3
```

`session` is `linked`, `offline` (a device is stored but not connected: WhatsApp is unreachable,
the keyring is locked, or the stream was replaced by another device) or `unlinked` (no device on
this machine). When neither the socket nor the session bus answers, the command prints why and
exits 1.

## `nuncio chats`

One line per chat: its name, its unread count, `pinned`/`archived` when it has them, and the local
time of its last message:

```
Alice                 2 unread  pinned          2026-10-31 12:34
…0001                 0 unread  -               2026-10-30 09:12
```

It never prints message text. A chat is shown by its name (a group's subject, else the contact's
address-book, business or push name); without one, by its number masked to the last four digits
(`…0001`), or `hidden number` when the number is not known (a LID chat): a LID's digits are never
printed. A group with no subject is `unnamed group` ([ADR 0002](adr/0002-chat-names.md)). With no
chats it prints `no chats` and exits 0.

One chat per contact: a contact's phone-number chat is merged into her hidden-id (`@lid`) chat when
whatsmeow knows the pair ([ADR 0001](adr/0001-one-chat-per-contact.md)).

## `nuncio send <jid> <message>`

Sends one message to one chat. The message is one argument: quote it, so an unquoted space cannot
smuggle in a second recipient.

- `sent`: WhatsApp acknowledged it. Exit 0.
- `queued: it will be sent once the session reconnects (client_id …)`: exit 0. Offline, or when
  the connection drops before WhatsApp acknowledges the message, it is queued and sent once when
  the session reconnects (one pass, oldest first, one second apart, the same message id).
- `not sent: <reason>` on stderr, with `nuncio: to try again: nuncio retry <client_id>`: exit 1.
- `not confirmed: …; it may have been delivered` on stderr, with the same retry hint: exit 1. A
  connection that dies silently still counts as linked for up to 3 minutes, until its keepalive
  fails, so a send in that window ends not confirmed rather than queued. So does a send whose
  answer from `nunciod` does not come in time, in the CLI and in both apps. It is never `not sent`,
  which would invite a duplicate: `nunciod` may already have handed it to WhatsApp.

Nothing is ever retried on its own: a message that ends `not sent` or `not confirmed` stays listed
as not sent, and `send` prints its client_id.

## `nuncio retry <client_id>`

Sends that one message again, once, under the same WhatsApp message id, so it is the same message
again, never a second one with a new id. A message that is queued or already sent is only
reported. It prints what `nuncio send` prints.

## `nuncio focus <jid>`

Asks the running UIs to open that chat and prints how many got the request: `focus delivered to 1
UI`, or `focus delivered to 0 UIs: no UI is running`. 0 means no UI is running, and that is not an
error.

## `nuncio link`

Links this machine to your WhatsApp account. **The app does the same with no terminal**: open
**nuncio** and press **Link** ([INSTALL.md](../INSTALL.md#link-this-device)). Nothing links without
`--consent`: without it the command prints the ban warning and exits 2, having touched nothing. It
never sends a message.

### `nuncio link --qr --consent`

```sh
nuncio link --qr --consent
```

Asks the **running** daemon for a device-linking flow and draws each QR code it publishes, until
the phone answers. `nunciod` owns the whole flow: it opens the store, creates the data key only
when pairing saves the device, generates the codes and keeps the session it pairs, so the terminal
only draws the matrix and exits. A link from here and one from the app leave exactly the same data
behind.

- The daemon must be running: `nuncio link: nunciod is not running; open nuncio, or start nunciod,
  and try again. Nothing was linked.` Any other command, or opening the app, starts it.
- `Ctrl-C` cancels the flow. The daemon answers `link_cancelled` only after it has removed what the
  flow created, so no device, no store and no keyring entry is left behind.
- A ban, a logout or an expired code ends the flow with one fixed sentence and exit 1:
  `WhatsApp stopped the linking: ask for a new QR`. Run the command again for a new code.
- A device is already linked: `nuncio link: nunciod already holds a linked WhatsApp device.
  Nothing was linked.` The daemon never replaces or re-pairs a device it already serves; unlink
  first (Settings → **This device** → **Unlink this device**, or `nuncio logout --confirm`).
- The code is pairing material: anyone who scans it while it is valid links their device to your
  account. It is drawn at the terminal's own size, with no escape sequences, and it is never
  logged.

### `nuncio link --qr --plain-qr --consent`

Writes only the code, frame by frame, one frame per QR code, with the 4-module margin a QR needs to
scan; a new code starts after a blank line. It carries no prose and no escape sequences, so it is
for a script or a GUI that draws the matrix itself, and the outcome is the exit status: 0 linked,
1 not linked. Errors still go to stderr.

### `nuncio link --phone <E.164 number> --consent`

**This is the one path that still pairs from the command itself**, and it will stay that way until
a frame carries a pairing code: no released path disappears. It prints an 8-digit code to enter on
the phone (`Settings → Linked devices → Link a device → Link with phone number instead`) and waits
up to 5 minutes for it.

It is the only path that needs the daemon **stopped**: it opens the store and creates the data key
itself, so it refuses while `nunciod` answers on `core.sock`
(`nuncio link: nunciod is running and owns this device's session; stop it first. Nothing was
linked.`). This is the one place a unit is stopped by hand (`systemctl --user stop nunciod`); the
app's flow never needs it. It also needs the keyring
(Secret Service) unlocked, because the device's keys are sealed under a data key kept there.

Once the phone accepts the code, it prints `nuncio: linked as …0001` (the number masked to its last
four digits), then the `systemctl --user start nunciod` line this command held the session for, and
exits. Opening **nuncio** from the launcher starts it too, and needs no terminal.

Run on a device that is already linked, it connects it once to check that WhatsApp still accepts
it, reports it and exits. A `nuncio.db` an older build left in plain is backed up
(`nuncio.db.pre-t17-<UTC time>.bak`, which still holds the keys in plain: delete it once nuncio
works) and sealed first, by `link` or by `nunciod` at start.

## `nuncio logout`

`nuncio logout --confirm` unlinks this device from WhatsApp and deletes everything nuncio keeps for
it here. The app's **Settings → This device → Unlink this device** does the same, with the word
`unlink` typed.

Without `--confirm` it only lists what it would delete and exits 2. With it, the teardown runs in
this order: (1) WhatsApp unlinks the device, (2) the keyring items under the service `nuncio` are
deleted (the data key `store.data_key` among them), (3) `nuncio.db` with its `-wal` and `-shm`
files, every migration backup (`nuncio.db.pre-t17-*.bak`), the `media` cache and the `exports` in
`$XDG_DATA_HOME/nuncio` are deleted (the directory too, when nothing else is in it and the sandbox
allows it), (4) `nunciod` removes its socket and exits.

It needs a **running** `nunciod`, because only the daemon's session may talk to WhatsApp for the
device: `nuncio logout: nunciod is not running. …` start it, or, if the device is already gone from
your phone's Linked devices, use `nuncio purge --confirm`. When WhatsApp does not confirm, nothing
is deleted.

## `nuncio purge`

`nuncio purge --confirm` is the same teardown without the network step, for a device already
removed from the phone's **Linked devices** (or a link that never finished). WhatsApp is never
contacted. It asks a running daemon to do it, or deletes the local data itself when no daemon runs.
Without `--confirm` it only lists what it would delete and exits 2.

## When something goes wrong

- **`the keyring (Secret Service) is locked or unavailable`**: unlock your login keyring (it
  normally unlocks with your session), or install a Secret Service — Omarchy ships one, plain Arch
  needs `gnome-keyring`. Nothing was linked, and nothing was written.
- **`nunciod is running and owns this device's session`** (`link --phone`): stop the unit, pair,
  then open **nuncio**, which starts it again.
- **`WhatsApp stopped the linking: ask for a new QR`**: a ban, a logout or an expired code. Run the
  command again; a ban will refuse again.
- **The daemon is up but says `offline`, with the keyring locked or absent**: it stays up, never
  connects to WhatsApp and writes nothing; it tries again on the session's backoff. When the data
  key is gone but a sealed device is stored, nothing creates a new one: the device cannot be read,
  and the way out is `nuncio purge --confirm`, then `nuncio link`. The apps show either state in
  their header (`keyring locked: unlock it to connect`, or `the stored key does not open this
  device: see SECURITY.md`); see
  [SECURITY.md](../SECURITY.md#a-locked-keyring-or-an-unreadable-device).
- **Logs**: `journalctl --user -u nunciod`. They never contain message text, full phone numbers or
  keys.
- **Your data**: `~/.local/share/nuncio` (directory `0700`, files `0600`), sealed under the data
  key in the keyring. Back it up by copying that folder plus the keyring item; the sealed database
  is useless without it.
