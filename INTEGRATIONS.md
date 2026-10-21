# Integrations

How nuncio fits into an Omarchy (Arch + Hyprland) desktop. **None of this has run on Linux yet**
(M4 acceptance is pending, see [README.md](README.md#status)); each part lists what is still to
verify in the document it links to.

## Omarchy theme

Both apps follow the current Omarchy theme live: they read
`~/.local/state/omarchy/current/theme/colors.toml` (honouring `XDG_STATE_HOME`) and redraw when it
changes (the shared `ThemeWatcher` in `crates/nuncio-client/src/theme.rs`). Without Omarchy they
keep a built-in palette. `nuncio-desktop --theme-dir DIR` follows `DIR/colors.toml` instead.

## Omarchy font (desktop app)

Omarchy has one font setting, the system monospace font, held by fontconfig. Where Omarchy is
installed (`~/.local/state/omarchy` exists), nuncio-desktop asks `fc-match monospace` at start and
uses that family for its whole UI, and follows a font switch live by watching
`~/.config/fontconfig`. Without Omarchy, libadwaita's font stays. The terminal app uses the
terminal's font. Details: [crates/nuncio-desktop/README.md](crates/nuncio-desktop/README.md#font-and-corners).

## Hyprland: corners and no title bar (desktop app)

- **Corners.** Under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set) the app asks
  `hyprctl -j getoption decoration:rounding` once at start (200 ms limit) and gives its windows
  that radius (Omarchy's default is 0: square). Elsewhere libadwaita's corners stay. A change is
  seen on the next start.
- **No title bar.** With `[app] title_bar = "auto"` (the default) the window controls are hidden on
  Hyprland and shown elsewhere; `"shown"` and `"hidden"` force it ([docs/config.md](docs/config.md)).
- The window class is the app id, `io.github.pwn27sbx.nuncio.Desktop`, for window rules.

## Hotkey and bar widget

[contrib/omarchy/](contrib/omarchy/README.md) has two pieces of Omarchy 4 glue, installed by hand
(the package does not install them):

- `hypr/nuncio.lua`: `SUPER + SHIFT + ALT + G` opens nuncio-desktop, or focuses its window. It
  replaces Omarchy's WhatsApp web app binding on that key.
- `bar-widget/`: a Quickshell bar plugin that shows the unread count (hidden at 0, `99+` above 99)
  and opens or focuses the app on a click. It reads only the `UnreadCount` number, never names or
  text.

Install, uninstall and the checks to run: [contrib/omarchy/README.md](contrib/omarchy/README.md).

## D-Bus names

Two names on the session bus, never mixed:

| Name | Owner | What it offers |
| --- | --- | --- |
| `io.github.pwn27sbx.nuncio` | `nunciod` | object `/io/github/pwn27sbx/nuncio`, interface `io.github.pwn27sbx.nuncio`: the `Status` method and the read-only `UnreadCount` property (`int32`, the total unread) |
| `io.github.pwn27sbx.nuncio.Desktop` | `nuncio-desktop` | its `GApplication` id (activate, open, `--gapplication-service`) |

The daemon takes its name only after its socket listens, so owning the name means clients can
connect. Messages and media never travel over D-Bus; the apps use the Unix socket
(`$XDG_RUNTIME_DIR/nuncio/core.sock`, protocol in [proto/wire.md](proto/wire.md)).

**Pending (T18):** `UnreadCount` is set once when the daemon starts and is not kept current, and
the `UnreadChanged` signal is not emitted yet. Until then the bar badge polls every 30 s and only
changes when the daemon restarts.

To read it by hand:

```sh
busctl --user --auto-start=no get-property io.github.pwn27sbx.nuncio \
  /io/github/pwn27sbx/nuncio io.github.pwn27sbx.nuncio UnreadCount
```

## systemd user unit and autostart

The package installs (see [packaging/README.md](packaging/README.md)):

- `/usr/lib/systemd/user/nunciod.service`: the daemon as a `systemd --user` service (`Type=dbus`,
  restarted only after a crash, slowly, and sandboxed; see [SECURITY.md](SECURITY.md#the-systemd-unit)).
  The package does not enable it; `install.sh` offers to, or run
  `systemctl --user enable --now nunciod.service`. Enabled, it starts at login
  (`WantedBy=default.target`), which keeps the bar widget fed.
- `/usr/share/dbus-1/services/io.github.pwn27sbx.nuncio.service`: D-Bus activation, so `nuncio`
  can start the daemon through its unit when it is not running.
- `/etc/xdg/autostart/io.github.pwn27sbx.nuncio.Desktop.desktop`: starts nuncio-desktop at login as
  a background service with no window (`--gapplication-service`), so notifications arrive without
  opening it; the next launch shows the window. To turn it off for yourself, copy the file to
  `~/.config/autostart/` and add `Hidden=true`.
- `/usr/share/dbus-1/services/io.github.pwn27sbx.nuncio.Desktop.service`: D-Bus activation of the
  app (the desktop entry is `DBusActivatable=true`).

The daemon's unit assumes the default `XDG_DATA_HOME`; with another one, add a drop-in
(`systemctl --user edit nunciod`).

## `whatsapp:` links

The app's desktop entry, `io.github.pwn27sbx.nuncio.Desktop.desktop`, registers
`x-scheme-handler/whatsapp` with `Exec=nuncio-desktop %U`, so a browser or `xdg-open` hands
`whatsapp://send?phone=…&text=…` links to it. `https://wa.me/…` and
`https://api.whatsapp.com/send?…` links stay with the browser unless you run
`nuncio-desktop LINK`. A link opens the chat and fills in the composer; **it never sends**. Group
invite links are not supported yet. Details: [docs/config.md](docs/config.md#links).

## Notifications

nuncio-desktop shows notifications through `GApplication`, so the desktop's own notification
daemon draws them. By default they carry no message text (only "New message",
"Photo", …); `[notifications] show_preview = true` adds it, `enabled = false` turns them off. One
notification per chat, withdrawn when you open or read the chat; a click opens it. Not for muted
chats, your own messages or the chat open in a visible window. Details:
[docs/config.md](docs/config.md#when-a-message-notifies). The terminal app does not notify.
