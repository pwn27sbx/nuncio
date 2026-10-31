# The nuncio command line

`nuncio` is a thin client of the running daemon. Everything that touches WhatsApp belongs to
`nunciod`, which owns the one linked session; `send` takes exactly one recipient and one message per
invocation, `retry` exactly one client id, and neither reads stdin.

This page documents the commands whose behaviour depends on the daemon. The rest of the CLI is in
the [README](../README.md#cli).

## `nuncio link`

Links this machine to your WhatsApp account. Nothing links without `--consent`: without it the
command prints the ban warning and exits 2, having touched nothing. It never sends a message.

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
  and try again. Nothing was linked.`
- `Ctrl-C` cancels the flow. The daemon answers `link_cancelled` only after it has removed what the
  flow created, so no device, no store and no keyring entry is left behind.
- A ban, a logout or an expired code ends the flow with one fixed sentence and exit 1:
  `WhatsApp stopped the linking: ask for a new QR`. Run the command again for a new code.

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
linked.`). It also needs the keyring (Secret Service) unlocked, because the device's keys are
sealed under a data key kept there.

Once the phone accepts the code, it prints `nuncio: linked as …0001` (the number masked to its last
four digits) and how to start the daemon, then exits.
