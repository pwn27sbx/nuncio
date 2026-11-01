# What's new in nuncio

One section per release; the tag equals the version (`v0.1.0` → `pkgver=0.1.0`), and the section
becomes the release notes.

## v0.2.1 — the message history, no demo chats, and a readable window

- **The daemon never serves demo chats.** Without a linked device, `chats` answers an empty list
  instead of the fixtures' invented conversations (*Ada Lovelace*, *Grace Hopper*, *Omarchy devs*),
  which misread as the user's own; the terminal app then says `No device linked · open nuncio to
  link it`. The demo data stays test and `nuncio-demo` material only, so this is a visible change
  in what the wire answers.
- **The unlink closes its dialogs, and the daemon stopping with it is not an error.** Confirming
  **Unlink this device** closes the confirmation and Settings even when the daemon goes away before
  it can answer, and the cut that follows a wipe is no longer shown as a connection error: the app
  lands on the link screen, and **Link** starts the daemon again when nothing is serving it.
- **Your chats arrive with their messages.** After a device is linked, the daemon imports the
  message history WhatsApp sends it, so a conversation is not empty the first time it is opened
  and the chat list shows its last message. The imported messages are stored read (an old message
  never counts as unread and never lights the badge), a blob applied twice stores nothing twice,
  and each chat is refreshed in an open app as its messages land. Nothing is downloaded, no history
  is requested and no read receipt is sent.
- **Unlinking no longer leaves the daemon alive with no socket.** When the phone was still sending
  the message history, the wipe waited for the whole import to be stored, so the process kept the
  bus name with nothing serving the socket and every client failed. Closing the session now stops
  the import at its next message and closes the store once nothing is writing to it, so a wipe
  right after linking exits the daemon as it should.
- **The conversation takes the room it has.** The transcript no longer stops at a 72-character
  measure: it fills its pane up to a broad maximum and the bubbles sit at the two ends, while the
  chat list keeps its fixed width, the way WhatsApp Web lays it out.
- **One type scale, one step denser.** Every size the window paints comes from a single table (the
  transcript's measure reads it too), so the same conversation fits more lines and nothing is
  clipped: title 14, body 12, label 11, meta 10.
- **A drawn photo no longer says `[image]` next to it.** A bubble whose picture or sticker is
  drawn drops the bracketed label; the label stays as the fallback when there is nothing drawn,
  and a document keeps its row. Video is unchanged: the wire carries no thumbnail for it yet.

## v0.2.0 — link a device inside the app

The wire and the first-run flow changed, so the version moves. The tag follows the home acceptance
run that installs this package and scans a code from a spare phone.

- **Linking happens in the app.** An unlinked **nuncio** opens on **Link this device to WhatsApp**
  with the warning, one **Link** button and the code drawn once you press it; scan it and the chat
  list appears without a restart. `Esc` cancels and leaves no device, no data and no keyring entry.
- **The daemon is the linker.** Six frames (`link_start`, `link_cancel`, `link_started`,
  `link_cancelled`, `link_qr`, `link_state`) carry the flow, with fixtures and both contract
  suites. `nunciod` owns the store, the data key and the codes; it refuses a start without
  consent, rotates the codes, and removes only what its own flow created.
- **`nuncio link --qr` is a client of those frames**: it draws each code the daemon publishes, and
  it needs the daemon running instead of stopped. `--phone` keeps its own pairing path (and its
  refusal while the daemon answers) until a frame can carry a pairing code.
- **Unlink from Settings**: **This device → Unlink this device**, with the word `unlink` typed,
  sends the daemon's existing wipe and lands on the link screen without a restart.
- **The terminal app's unlinked copy** says `No device linked · open nuncio to link it` instead of
  naming the CLI.
- **Docs**: [INSTALL.md](INSTALL.md) is now the whole user story (install line, open, press Link,
  scan, uninstall) and carries no `systemctl` step; the CLI reference moved to
  [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md) and the README keeps the command list and a
  pointer. The package ships `INSTALL.md` and the daemon's unit points its `Documentation=` at it.

## v0.1.1 — the apps start the daemon

- **No more `systemctl`.** The terminal and desktop clients now ask the session bus for the
  daemon (the activation file the package ships), so a fresh install works by opening the app;
  the desktop app also enables the daemon's login item itself, once, without a terminal.
- **A font read caught mid-rewrite is no longer reported as "no font"** (it made the app fall
  back for a frame on every theme switch, and it made CI flaky).
- The image test that assumed a queued encode is always cancellable now states the real
  contract: the running encode plus the newest width. It blocked the release build while CI
  passed, because the release runs the same suite under more load.

Linking a device is still done with `nuncio link --qr --consent`; linking from inside the app is
the next piece of work.

## v0.1.0 — the first release

The first public package, and the first time the whole thing installs with one line.

- **A terminal client that reads like a chat app.** One status line in the footer (the session
  state, in the warning tone when it is not linked), the app's name and the unread count in the
  header, and no version numbers in the chrome. Chat rows have a fixed time column, the markers
  the chat actually has and a `●n` badge; only the pane with the keys fills its selection; the
  list is 40 % of the screen at 100 columns and up, a fixed 32 from 60 to 99, and one pane under
  60 with `Tab`. The transcript opens with `Beginning of conversation` and says `▲`/`▼` when
  there is more above or below. The composer says what Enter does, where it does it.
- **Filters**: `[` and `]` cycle All / Unread / Archived, and the rule shows which one is on with
  its count. One `Filter` definition is shared with the desktop app.
- **Meta text is legible**: previews, times and hints use the palette's `text_muted` role, which
  holds 4.5:1 against both surfaces, instead of the terminal's own dimming.
- **The overlay vocabulary is one rule**: help, search and the reaction picker share
  `min(screen − 4, 76)` columns, with right-aligned keys.
- **Packaging and CI**: `makepkg` builds the package offline after `prepare()`, CI checks every
  installed path against `packaging/MANIFEST`, verifies the systemd units and runs
  `packaging/check.sh`; this release adds the job that turns a tag into the release assets.
- **Install**: one line from the release page, `INSTALL.md` for the first run, update and
  uninstall.

Known limitations are in [README.md](README.md): it is an unofficial client, use a spare number.
