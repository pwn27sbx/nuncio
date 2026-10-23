# Configuration file

nuncio-tui and nuncio-desktop read one plain TOML file:

- `$XDG_CONFIG_HOME/nuncio/config.toml`, or
- `~/.config/nuncio/config.toml` when `XDG_CONFIG_HOME` is unset or empty.

Every key is optional and has the default shown below. A missing file means all the defaults.

nuncio-desktop's settings (Ctrl+,) write this file after each change: atomically (a temporary file
renamed over it), mode 0600, creating a missing directory with mode 0700 (the mode of a
directory that already exists is not changed), following a symlink to its target. A `config.toml`
symlink whose target does not exist makes saving fail with an error, and nothing is created. They write only the keys below, so **comments and blank lines in the file are not
preserved** (the written file starts with a comment saying so). A file that does not load (an
unknown key, a wrong type, bad TOML) is never overwritten: the settings show why and change
nothing until it is fixed or removed.
A key nuncio does not know (a typo such as `show_previews`) is an error that names it, and so is a
value of the wrong type: nuncio-tui prints it and exits with status 2 instead of silently ignoring
the setting.

## Example (every key at its default)

```toml
[ui]
# How nuncio-tui draws pictures: "kitty", "sixel", "iterm2", "halfblocks" or "off".
# Leave it out to ask the terminal at startup. NUNCIO_IMAGE_PROTOCOL wins over this key.
# image_protocol = "kitty"

[notifications]
# Desktop notifications for new messages.
enabled = true
# Put the message text in the notification. Off by default for privacy: a notification
# then says only "New message", "Photo", "Sticker", "Video", "Audio" or "Document".
show_preview = false

[app]
# nuncio-desktop: closing the window only hides it; the app keeps running (connected, and
# notifying) until Quit (Ctrl+Q or the menu). false: closing the window quits.
background = true
# nuncio-desktop: start with no window, in the background; launching it again shows the window.
start_hidden = false
# nuncio-desktop: the window controls in the header bars. "auto" hides them on Hyprland
# (HYPRLAND_INSTANCE_SIGNATURE set, or Hyprland in XDG_CURRENT_DESKTOP) and shows them elsewhere;
# "shown" and "hidden" force it.
title_bar = "auto"
```

## Keys

| Key | Type | Default | Read by |
|---|---|---|---|
| `[ui] image_protocol` | string: `kitty`, `sixel`, `iterm2`, `halfblocks`, `off` | unset: ask the terminal | nuncio-tui (`NUNCIO_IMAGE_PROTOCOL` wins) |
| `[notifications] enabled` | bool | `true` | nuncio-desktop |
| `[notifications] show_preview` | bool | `false` | nuncio-desktop |
| `[app] background` | bool | `true` | nuncio-desktop |
| `[app] start_hidden` | bool | `false` | nuncio-desktop |
| `[app] title_bar` | string: `auto`, `shown`, `hidden` | `auto` | nuncio-desktop |

## When a message notifies

A new message shows a notification unless it is mine, its chat is muted, its chat is open in a
visible window, the same message was already notified (the daemon publishes a message again when
a reaction on it changes), or `enabled = false`. An archived chat that is not muted still
notifies, as WhatsApp does; a chat open in a hidden window notifies too. The title is the chat's
name, else its number masked to the last four digits, else `hidden number` or `unnamed group`.
Clicking the notification shows nuncio-desktop's window and opens that chat (marking it read once
its messages are shown); a message that arrived under a contact's phone number opens her merged
chat. Each chat has one notification at a time (a newer message replaces it), and opening or
reading the chat withdraws it.

## Links

`whatsapp://send?phone=…&text=…`, `https://wa.me/<number>?text=…` and
`https://api.whatsapp.com/send?phone=…&text=…` open the chat with that number and put `text` in
the composer. A link never sends anything. Group invite links (`https://chat.whatsapp.com/…`) are
not supported yet. nuncio-desktop opens them given as arguments (`nuncio-desktop LINK`) or through
its `GApplication` `open`; see `crates/nuncio-desktop/README.md`.
