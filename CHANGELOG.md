# What's new in nuncio

One section per release; the tag equals the version (`v0.1.0` → `pkgver=0.1.0`), and the section
becomes the release notes.

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
