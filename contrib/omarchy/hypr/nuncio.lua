-- nuncio: one hotkey that opens nuncio-desktop, or focuses its window when it is
-- already open.
--
-- Omarchy 4 configures Hyprland in Lua (Hyprland 0.55+ reads ~/.config/hypr/hyprland.lua
-- instead of hyprland.conf when it exists). Copy this file to ~/.config/hypr/nuncio.lua
-- and add one line at the end of ~/.config/hypr/bindings.lua:
--
--   require("hypr.nuncio")
--
-- `o` is Omarchy's helper table (defined in $OMARCHY_PATH/default/hypr/helpers.lua).
-- `{ launch = ..., focus = ... }` expands to
--   omarchy-launch-or-focus '<focus>' 'uwsm-app -- <launch>'
-- which focuses the first window whose class (or title) matches <focus>, and otherwise
-- starts the app. A second start of nuncio-desktop does not open a second app: GTK
-- forwards it to the running one, which shows its window again (also when the window was
-- closed to the background, so there was no window for Hyprland to focus).
--
-- The class is the GTK application id, which GTK 4 sends as the Wayland app_id; Hyprland
-- reports it as the window class. Keep it in sync with APP_ID in
-- crates/nuncio-desktop/src/args.rs. It is `io.github.pwn27sbx.nuncio.Desktop`, not the
-- daemon's bus name `io.github.pwn27sbx.nuncio`; the regex is anchored at both ends.

local nuncio_class = "^io\\.github\\.pwn27sbx\\.nuncio\\.Desktop$"

-- SUPER + SHIFT + ALT + G is Omarchy's WhatsApp web app binding; nuncio replaces it.
-- Pick another key if you want to keep the web app.
-- Omarchy 4.0.6 has `o.bind` but no `o.rebind` (helpers.lua), so the key is unbound first when
-- the Lua binding API offers `hl.unbind` (guarded, so an older API does not break the config).
pcall(function() hl.unbind("SUPER + SHIFT + ALT + G") end)
o.bind("SUPER + SHIFT + ALT + G", "WhatsApp (nuncio)", {
  launch = "nuncio-desktop",
  focus = nuncio_class,
})

-- Plain Hyprland 0.55+ without Omarchy's helpers (no omarchy-launch-or-focus): the same
-- idea with hyprctl and jq. Uncomment and drop the o.rebind above.
--
-- hl.bind("SUPER + SHIFT + ALT + G", hl.dsp.exec_cmd(
--   "a=$(hyprctl clients -j | jq -r '.[] | select(.class == \"io.github.pwn27sbx.nuncio.Desktop\") | .address' | head -n1); "
--     .. "if [ -n \"$a\" ]; then hyprctl dispatch \"hl.dsp.focus({ window = \\\"address:$a\\\" })\"; "
--     .. "else setsid nuncio-desktop >/dev/null 2>&1 & fi"
-- ), { description = "WhatsApp (nuncio)" })
