# nuncio on Omarchy: hotkey and bar widget

Two pieces of Omarchy glue, written against Omarchy 4 (Hyprland configured in Lua, bar on
Quickshell). Nothing here has run on Linux yet: the checks at the end are what decides it
works.

| File | What it does | Goes to |
|---|---|---|
| `hypr/nuncio.lua` | `SUPER + SHIFT + ALT + G` opens nuncio-desktop, or focuses its window | `~/.config/hypr/nuncio.lua` |
| `bar-widget/manifest.json`, `bar-widget/BarWidget.qml` | Unread badge in the bar, hidden at 0; a click opens or focuses the app | `~/.config/omarchy/plugins/pwn27sbx.nuncio-unread/` |

The widget only reads the daemon's `UnreadCount`: a number. No chat names, no message text.

## Names

The daemon owns `io.github.pwn27sbx.nuncio` on the session bus (`internal/ipc/connect.go`);
the widget reads `UnreadCount` from it. nuncio-desktop's GTK application id, which is also
its window class in Hyprland, is `io.github.pwn27sbx.nuncio.Desktop`. They used to be the
same string, and since a GTK application registers its id as a bus name, the two competed
for one name; renaming the app's id resolved that. The class in `hypr/nuncio.lua` and
`windowClass` in `BarWidget.qml` follow the app's id (`APP_ID` in
`crates/nuncio-desktop/src/args.rs`).

## Install

Hotkey:

```sh
cp contrib/omarchy/hypr/nuncio.lua ~/.config/hypr/nuncio.lua
echo 'require("hypr.nuncio")' >> ~/.config/hypr/bindings.lua
hyprctl reload
omarchy menu keybindings --print | grep -i nuncio
```

The binding replaces Omarchy's WhatsApp web app key. To keep the web app, edit the key in
`nuncio.lua`. The file also carries a commented variant for plain Hyprland without
Omarchy's helpers.

Bar widget:

```sh
dest=~/.config/omarchy/plugins/pwn27sbx.nuncio-unread
mkdir -p "$dest"
cp contrib/omarchy/bar-widget/manifest.json contrib/omarchy/bar-widget/BarWidget.qml "$dest"/
omarchy plugin validate "$dest"
qmllint -I "$OMARCHY_PATH/shell" "$dest/BarWidget.qml"
omarchy-shell shell rescanPlugins
omarchy plugin enable pwn27sbx.nuncio-unread --section right
```

Errors from the widget show up in `qs log -p "$OMARCHY_PATH/shell" --tail 100`.

Uninstall: `omarchy plugin disable pwn27sbx.nuncio-unread`, remove the plugin folder and
`~/.config/hypr/nuncio.lua`, and delete the `require("hypr.nuncio")` line.

## How the widget reads the count

Quickshell has no generic D-Bus API, so the widget runs `busctl` (part of systemd):

- `busctl --user --auto-start=no get-property io.github.pwn27sbx.nuncio /io/github/pwn27sbx/nuncio io.github.pwn27sbx.nuncio UnreadCount`
  prints `i <n>`. `--auto-start=no` means the bar never starts the daemon just to read it.
  If the daemon is not running, the read fails and the badge hides.
- `busctl --user monitor --match "type='signal',path='/io/github/pwn27sbx/nuncio'" --match
  "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='io.github.pwn27sbx.nuncio'"`
  runs for as long as the bar does. The daemon emits `PropertiesChanged` and `UnreadChanged`
  whenever the total moves (recounted 200 ms after a change, so at most five times a second),
  and the bus sends `NameOwnerChanged` when the daemon starts or stops; any of those lines
  triggers a read right away. A monitor that ends is started again after 5 s, backing off to
  5 min while it keeps failing.
- A read every 5 minutes as a fallback, for the time the monitor is down.

The count is the total of incoming messages not read yet across every chat, archived and muted ones
included: the sum of the per-chat `unread` that `chats` lists, and the `unread` of `status`.

## Verify at home

Acceptance (plan, M4): "the bar widget shows and clears unread; the hotkey launches-or-focuses".

1. Names: with nunciod and nuncio-desktop both running (start them in either order),
   `busctl --user status io.github.pwn27sbx.nuncio` names nunciod and
   `busctl --user status io.github.pwn27sbx.nuncio.Desktop` names nuncio-desktop.
2. Class: `hyprctl clients -j | jq -r '.[].class'` with the app open lists
   `io.github.pwn27sbx.nuncio.Desktop`.
3. Hotkey with the app closed: it starts. With its window on another workspace: focus
   moves to it, and no second window appears. With the window closed to the background:
   it comes back.
4. Read by hand: the `get-property` command above prints `i <n>` while the daemon runs,
   and fails with the daemon stopped (it must not start it).
5. Badge: with unread messages it shows the number (99+ above 99). A new message raises it
   and reading the chat in the app brings it down within a second, without a restart; at 0
   the badge hides. Stopping nunciod hides it right away (the name loses its owner), and
   starting it again shows the count. `busctl --user introspect io.github.pwn27sbx.nuncio
   /io/github/pwn27sbx/nuncio` lists `UnreadCount` and `UnreadChanged`.
6. Monitor: `busctl --user monitor --match "type='signal',path='/io/github/pwn27sbx/nuncio'"`
   runs without an error (it confirms the `--match` option and that monitoring the user bus
   needs no extra rights).
7. Click on the badge: same launch-or-focus as the hotkey.
8. Uninstall as above: the widget leaves the bar and the key does nothing.

## Assumptions to confirm

- The widget uses Omarchy's own bar types (`BarWidget`, `BarIconButton` from `qs.Ui`, `Util`
  from `qs.Commons`), as the first-party `omarchy.system-update` widget does. That Omarchy
  hands them to third-party plugins too comes from its plugin guide, not from a run.
- `` (the Font Awesome WhatsApp glyph) exists in the bar's Nerd Font.
- A second `nuncio-desktop` start is forwarded to the running one and presents the window
  (the app's `activate` handler does this; `[app] start_hidden` can keep it hidden).

## Sources

- Hyprland moves to Lua configs from 0.55: https://hypr.land/news/26_lua/
- Hyprland binds and dispatchers: https://wiki.hypr.land/configuring/core/binds/, https://wiki.hypr.land/configuring/core/dispatchers/
- Omarchy plugin development: https://plugins.omarchy.org/develop.html
- Omarchy source (branch `quattro`, read 2026-10-21): `config/hypr/bindings.lua`,
  `default/hypr/helpers.lua` (`o.bind`, `o.rebind`, `launch_sole`),
  `default/hypr/bindings/applications.lua`, `bin/omarchy-launch-or-focus`,
  `shell/plugins/bar/README.md`, `docs/omarchy-shell.md`,
  `shell/plugins/bar/widgets/SystemUpdate.qml`: https://github.com/omacom/omarchy/tree/quattro
- Quickshell modules (no generic D-Bus; `Process`, `SplitParser`, `StdioCollector`):
  https://quickshell.org/docs/v0.2.1/types/
