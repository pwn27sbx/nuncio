# nuncio-desktop

The GTK 4 + libadwaita client. It draws the shared view model (`nuncio_client::Model`: chat list,
transcript, composer and their rules) and talks to `nunciod` with the shared driver rules
(`nuncio_client::driver`), so it behaves as `nuncio-tui` does: one chat per contact, a draft never
changes recipient, sends marked sending, queued, sent, not sent or not confirmed, a retry only when
you ask, read receipts only when you open a chat. A connection that dies silently still counts as
linked for up to 3 minutes, until its keepalive fails, so a send in that window is marked not
confirmed rather than queued.

## Run it against the demo daemon

Nothing here touches WhatsApp. In two terminals:

```sh
mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo go run ./cmd/nuncio-demo
XDG_RUNTIME_DIR=/tmp/nuncio-demo cargo run -p nuncio-desktop
```

On a Mac, install GTK first: `brew install gtk4 libadwaita pkgconf`. The demo answers sends at once
(a text starting with `fail` ends not sent, so Retry can be tried), sends an invented reply after
each send, and has a photo, stickers and a document to open. Stop the demo and start it again to see
the header say `reconnecting…` and the app resync on its own.

After the session the headers' subtitle names why it is not connected when the daemon says so
(`stop_reason`, `proto/wire.md`), in fixed text: a WhatsApp stop (`offline · WhatsApp temporarily
banned this account`, …), `offline · keyring locked: unlock it to connect` while the keyring is
locked, or `offline · the stored key does not open this device: see SECURITY.md` when the stored
key does not open the device. It goes away once the daemon's next frame has no reason (the keyring
was unlocked and the device opened). A reason this build does not know shows as
`WhatsApp stopped the session`.

`--theme-dir DIR` follows `DIR/colors.toml` instead of Omarchy's theme; `--probe` runs the theme
switch check without a window. `GApplication`'s own `--gapplication-*` options are passed on to it
(the app's D-Bus `.service` starts it with `nuncio-desktop --gapplication-service`); any other
unknown option is a usage error.

The app id is `io.github.pwn27sbx.nuncio.Desktop`, which `GApplication` also owns as a D-Bus name.
It is deliberately not the daemon's bus name (`io.github.pwn27sbx.nuncio`): one name has one owner.

Links: `XDG_RUNTIME_DIR=/tmp/nuncio-demo cargo run -p nuncio-desktop -- 'whatsapp://send?phone=15551230001&text=hola'`
opens that chat with `hola` in the composer (see [Links](#links)).

## Notifications

A new message shows a desktop notification (through `GApplication`, so the desktop's own
notification daemon draws it) when `nuncio_client::notify` says it deserves one: not for my own
messages, a muted chat, a message already notified (the daemon publishes a message again when a
reaction on it changes), the chat open in a window you can see, or with `[notifications] enabled =
false`. The title is the chat's name (else its masked number); the body says only `New message`,
`Photo`, `Sticker`, … unless `[notifications] show_preview = true`, which shows the text and, for a
group message, who wrote it first (`Ana: Running late`; the title stays the group's name).

- Each chat has one notification: a newer message replaces the older one's.
- Clicking it (its `default` action, `app.open-chat` with the chat's JID) shows the window and
  opens that chat, which marks it read once its messages are shown, as a click in the list does. A
  message that came under a contact's phone number opens her merged chat. Clicked at a cold start
  (the desktop starts the app to run the action), the chat opens as soon as the chat list arrives.
- Opening the chat, or reading it here or on another device, withdraws its notification.

## Background mode

With `[app] background = true` (the default), closing the window only hides it: the app keeps
running, connected to `nunciod`, and keeps notifying. Quit (Ctrl+Q, or Quit in the sidebar
header's menu) really quits. Launching `nuncio-desktop` again, clicking a notification or opening a link brings the
window back. With `background = false`, closing the window quits.

`[app] start_hidden = true` starts with no window, in the background; the next launch shows it.

## Links

`nuncio-desktop LINK` (or a running app's `GApplication` `open`) opens a
`whatsapp://send?phone=…&text=…`, `https://wa.me/<number>?text=…` or
`https://api.whatsapp.com/send?phone=…&text=…` link (`nuncio_client::link`): the chat with that
number opens (its merged chat for a contact the daemon knows by LID; an empty conversation for a
number with no chat yet, which the first send creates), and the link's text replaces the draft in
the composer. **A link never sends anything**: the message goes only when you press Enter. A group
invite link (`https://chat.whatsapp.com/…`) says `group invite links are not supported yet`; any
other link says why it was not opened. A link given at a cold start waits for the chat list.

For a browser to hand `whatsapp:` links to the app, its desktop entry (packaging, M4) needs:

```ini
Exec=nuncio-desktop %U
MimeType=x-scheme-handler/whatsapp;
```

(`https://wa.me` links stay with the browser unless you open them with `nuncio-desktop LINK`.)
On a Mac nothing registers the scheme, so `open 'whatsapp://…'` does not reach the app: pass the
link on the command line.

## Command palette

F1 or Ctrl+K (or Command palette in the sidebar header's menu) opens a search box over the window
listing what can be done now, each with its key:

- `Chat: <name>` for every chat (named as the list names it): opens it, as a click does;
- `Search in this chat` (Ctrl+F), while a chat is open;
- for the focused message, only what its menu enables: `Reply to the selected message` (`r`),
  `React to the selected message…` (`e`, opens the picker), `Retry the selected message`
  (Shift+R, a send that ended not sent) and `Open the selected media` (`o`);
- `Settings` (Ctrl+,) and `Quit` (Ctrl+Q).

Typing filters the list: the commands holding the typed letters in order, ignoring case and spaces
(`bn` finds `Chat: Ben`), best matches first. Up and Down choose, Enter runs, Escape closes. The
palette never sends a message: there is no send command, and a draft stays in the composer.

## Settings

Ctrl+, (or Settings in the menu or the palette) opens the settings over the configuration file
([`docs/config.md`](../../docs/config.md)): notifications and their text, the title bar, background
mode, starting hidden, and nuncio-tui's picture protocol. Each change is written at once
(atomically: a temporary file renamed over it, mode 0600, in a 0700 directory), keeping only the
known keys, so **comments in the file are not kept**.

- Notifications and the title bar change at once; background mode and start hidden take effect on
  the next start; the picture protocol is read by nuncio-tui at its next start.
- A change that cannot be written keeps the old file and the old value, and a toast says why.
- A file the app cannot read (a key it does not know, a wrong type, bad TOML) is shown at the top
  of the settings with every setting disabled, and is never overwritten: fix or remove it.
- A `config.toml` that is a symlink (a dotfiles repository) is followed: its target is rewritten
  and the link stays; a link to nothing makes saving fail and creates nothing.

## Pictures

Photos and stickers are drawn inside their bubble, under their `[image]` / `[sticker]` label, with
nuncio-tui's rules (the decode rules are shared: `nuncio_client::pictures`):

- **Thumbnail first.** The small picture WhatsApp embeds in the message (the wire's `thumb`) is drawn
  at once: nothing is downloaded for it. It is decoded off the main loop, two pictures at a time, and
  kept in a cache of decoded pictures (the 256 last used, about 64 MiB).
- **No jumps.** The bubble reserves the picture's box from the `width` and `height` the message states
  (a photo at most 300x300 px, a sticker at most 160x160, never larger than the picture) before
  anything is decoded, and the thumbnail and later the full picture are drawn in that same box. A
  picture whose message carries no size takes its box when its first image arrives (a thumbnail's
  shape scaled up to those caps, or the full picture's own size) and keeps it from then on, also
  for the full picture and after the cache let its pictures go: it changes size once.
- **While the full picture downloads** (`downloading…` in its label) the thumbnail stays in the box;
  one the cache let go is decoded again.
- **Full picture on request only.** A click on the picture, Enter or `o` on it, or Open in its menu
  fetches the full file (`download_media`) and draws it in the same box; nothing is ever downloaded by
  itself. The palette's `Open the selected media` does the same, and asking again for a full picture
  that could not be shown tries it once more. Open externally (Shift+O) hands the file to the desktop's opener, as before. There is no
  separate viewer: open it externally to see it at full size.
- **Untrusted bytes.** Only JPEG, PNG and WebP are decoded; the size a picture's header declares is
  checked before anything is decoded (a thumbnail may be at most 1024 px a side, a full picture
  16384, and a downloaded file at most 64 MiB). A picture that is corrupt, too large or another format
  is not drawn: the label stays, and for a full picture a toast says why.
- **Stickers.** Transparent pixels show the bubble behind them. An animated sticker (its label says
  `▶`) shows its first frame, and plays once its full picture was fetched on request: looping
  forever, each frame for its own delay (one under 10 ms is shown for 100 ms). Its frames are shrunk
  to its box (once, not twice as a still picture) as they are decoded and may keep at most 16 MiB,
  each frame counted as at least 4 KiB (they count in the cache's 64 MiB). A sticker over that cap
  shows its first frame only, as sharp as a still sticker: a 512 px sticker keeps 160x160 frames
  (100 KiB each), so one with more than 163 frames stays still. So does one
  with more than 1000 frames, whose canvas is over 1024 px a side, or with a frame that does not
  decode. The thumbnail never plays. When the desktop turns animations off (GTK's
  `gtk-enable-animations` setting) a playing sticker stops at its first frame and none starts;
  turned back on, a sticker plays the next time its bubble is drawn. nuncio-tui keeps drawing the
  first frame.
- **Screen readers.** A picture bubble reads as what it is, then its caption: `Ana, 10:05: Photo,
  the beach`, `Ana, 10:05: Sticker`; the picture itself says the same (`Photo, the beach`).

To try it, run the demo daemon and the app as in [Run it](#run-it-against-the-demo-daemon): the
demo's `Travel group (demo)` chat has a photo, `The view from the hotel` (a 480x320 PNG colour
gradient, its thumbnail a 48x32 JPEG of it), and three stickers (a yellow disc, an animated pink
one, and a blue one with no thumbnail, which shows only its label and an empty box until you click
it; each sticker's file is a 128x128 WebP of its own disc). Each thumbnail shows at once, blurry, in a box that does not move;
click the photo (or select it and press Enter or `o`) and the sharp gradient replaces it; click a
sticker and its own disc replaces the thumbnail (the demo's sticker files are still pictures, so the
animated one does not play). Shift+O opens the file with the desktop's opener.

## Layout

After [Sealwax](https://github.com/PixDevsApps/sealwax) (MIT; its design was copied, no code): the
chat list in the sidebar and the conversation as the content of an `AdwNavigationSplitView`, one
pane at a time below 600 px, flat surfaces in the live Omarchy palette, toasts for notices.

- **Chat list:** title, unread count, `★` pinned, `⊘` muted, `▤` archived (archived chats last), the
  last message and its time.
- **Conversation:** my messages on the right in the accent colour, theirs on the left with the
  sender; time and send state under each; the quoted message (`↪`), reactions, and `[image]`,
  `[sticker]`, `[document]` labels; photos and stickers are drawn in their bubble (see
  [Pictures](#pictures)).
- **Composer:** several lines; a "Replying to" bar with a cancel button; a banner when the open chat
  was merged away, which blocks the send until you choose the chat again.

The title bar's window controls follow `[app] title_bar` in `config.toml`: `auto` (the default)
hides them on Hyprland (`HYPRLAND_INSTANCE_SIGNATURE`, or Hyprland in `XDG_CURRENT_DESKTOP`) and
shows them elsewhere; `shown` and `hidden` force it. The header bars carry the `nuncio-headerbar`
class and the window `nuncio-no-title-bar` when they are hidden, for your own CSS.

## Font and corners

The app follows the rest of an Omarchy desktop, as it follows the palette:

- **Font.** Omarchy has one font setting, the system monospace font, and fontconfig holds it
  (`omarchy-font-set` writes `~/.config/fontconfig/fonts.conf`; `omarchy-font-current` asks
  `fc-match monospace`). Where Omarchy is installed (`~/.local/state/omarchy` exists), the app asks
  `fc-match monospace -f '%{family}\n'` at start (at most 200 ms) and uses that family for its whole
  UI, the settings window and the popovers included, as Omarchy uses it for the terminal and the
  bar. When fontconfig is slower than that (a cold cache at login), it is asked once more in the
  background (at most 2 s) and the font is applied when it arrives. Only the first instance asks:
  a second launch that just hands over to the running app does not. A font switch is followed live: the app watches `~/.config/fontconfig` (and `~/.config`, for
  the first switch, which creates it) and asks fontconfig again. Without Omarchy, or when fontconfig
  gives no usable answer, libadwaita's font stays.
- **Corners.** Under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set) the app asks
  `hyprctl -j getoption decoration:rounding` once at start, with a 200 ms limit, and gives its
  windows that corner radius (Omarchy's default is 0: square corners, like its other windows). Not
  on Hyprland, or with no `hyprctl`, a slow one or output that is not a rounding, libadwaita's
  corners stay and one line says why. A change of the rounding is seen on the next start. Bubbles
  keep their own radius.
- **Title bar.** On Hyprland the window controls are hidden ([Layout](#layout)).

The windows carry the `nuncio-window` class and their content `nuncio-root`, for your own CSS.

## Keys

Tab moves between the chat list, the conversation and the composer.

| Where | Key | Does |
| --- | --- | --- |
| chat list | Up, Down | move |
| chat list | Enter, click | open the chat (only opening marks it read) |
| conversation | Up, Down | move between messages |
| conversation | `r` | reply |
| conversation | `e` | react (👍 ❤️ 😂 😮 😢 🙏, or remove mine) |
| conversation | Shift+R | retry a send that was not sent |
| conversation | `o`, click on a picture | a photo or sticker: show the full picture in its bubble; any other media: open the file with the desktop's opener |
| conversation | Shift+O | open the media file (a picture too) with the desktop's opener |
| conversation | Enter | on a photo or sticker, show it in full; on any other message, its menu |
| conversation | Menu, Shift+F10, right click | the message's menu |
| conversation | Escape | go to the composer |
| composer | Enter | send |
| composer | Shift+Enter | new line |
| composer | Escape | cancel the reply (else go to the conversation) |
| anywhere | Ctrl+F | search the open chat (Enter searches, Escape closes) |
| anywhere | F1, Ctrl+K | the command palette (type to filter, Up/Down, Enter runs, Escape closes) |
| anywhere | Ctrl+, | the settings |
| anywhere | Ctrl+Q | quit (closing the window only hides it in background mode) |

Requests go to `nunciod` one at a time, in order, on one connection. Opening a media file waits up
to 90 s for the daemon to fetch it, and a chat opened meanwhile stays loading until that download
is answered. Sends and retries go on a connection of their own, one at a time, in order, each
answer awaited up to 25 s (nunciod's own send timeout is 20 s): the message shows "sending" until
then, and other requests never wait for it. A send not answered within 25 s is marked not
confirmed and the app reconnects; one still waiting when the connection is lost is answered all
the same and marks only that send. Events keep arriving on their own connection.

A new message in the open chat is marked read only while the window is active and shown; when the
window comes back, the open chat's unread messages are marked read then.

Screen readers: the chat list and the transcript are named lists; each row reads its whole content
(for a message: sender, time, send state, the quoted line, the text and the reactions; a photo or
sticker says `Photo` or `Sticker` instead of `[image]` / `[sticker]`).

## Tests

`cargo test -p nuncio-desktop`: the command line, the Omarchy font and Hyprland rounding in the
generated CSS, `hyprctl`'s answer (through a fake runner: never the real one), the view data, the controller (notifications, links), the
lifecycle (background, start hidden), the palette's commands and matching, the settings' rules, the
pictures (boxes, what is decoded when, the cache, two decoding threads) and the worker (against a
fake daemon) are plain unit tests; `tests/smoke.rs` builds the real window on GTK's main thread (its own `main`,
`harness = false`) and skips when GTK cannot start. It never shows a window or a real notification:
no `GApplication` is attached, so notifications are only recorded, and the `open-chat` action is
activated from a plain action group. The palette is opened through its F1 shortcut's action and the
settings window is built (not shown) over a temporary `config.toml`, never yours. A photo and a
sticker bubble are checked to reserve their box before decoding, to get a texture of the
thumbnail's size, and to take the full picture only after a click; an animated sticker's full
picture to keep all its frames and play them, and to stay still with animations turned off.
