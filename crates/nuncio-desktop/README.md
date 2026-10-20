# nuncio-desktop

The GTK 4 + libadwaita client. It draws the shared view model (`nuncio_client::Model`: chat list,
transcript, composer and their rules) and talks to `nunciod` with the shared driver rules
(`nuncio_client::driver`), so it behaves as `nuncio-tui` does: one chat per contact, a draft never
changes recipient, sends marked sending, queued, sent, not sent or not confirmed, a retry only when
you ask, read receipts only when you open a chat.

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

`--theme-dir DIR` follows `DIR/colors.toml` instead of Omarchy's theme; `--probe` runs the theme
switch check without a window.

## Layout

After [Sealwax](https://github.com/PixDevsApps/sealwax) (MIT; its design was copied, no code): the
chat list in the sidebar and the conversation as the content of an `AdwNavigationSplitView`, one
pane at a time below 600 px, flat surfaces in the live Omarchy palette, toasts for notices.

- **Chat list:** title, unread count, `★` pinned, `⊘` muted, `▤` archived (archived chats last), the
  last message and its time.
- **Conversation:** my messages on the right in the accent colour, theirs on the left with the
  sender; time and send state under each; the quoted message (`↪`), reactions, and `[image]`,
  `[sticker]`, `[document]` labels (pictures are not drawn yet: open them).
- **Composer:** several lines; a "Replying to" bar with a cancel button; a banner when the open chat
  was merged away, which blocks the send until you choose the chat again.

The title bar's window controls follow `[app] title_bar` in `config.toml`: `auto` (the default)
hides them on Hyprland (`HYPRLAND_INSTANCE_SIGNATURE`, or Hyprland in `XDG_CURRENT_DESKTOP`) and
shows them elsewhere; `shown` and `hidden` force it. The header bars carry the `nuncio-headerbar`
class and the window `nuncio-no-title-bar` when they are hidden, for your own CSS.

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
| conversation | `o` | open the media file with the desktop's opener |
| conversation | Enter, Menu, Shift+F10, right click | the message's menu |
| conversation | Escape | go to the composer |
| composer | Enter | send |
| composer | Shift+Enter | new line |
| composer | Escape | cancel the reply (else go to the conversation) |
| anywhere | Ctrl+F | search the open chat (Enter searches, Escape closes) |

Requests go to `nunciod` one at a time, in order, on one connection. Opening a media file waits up
to 90 s for the daemon to fetch it, and a send or a chat opened meanwhile stays "sending" (or
loading) until that download is answered. Events keep arriving on their own connection.

A new message in the open chat is marked read only while the window is active and shown; when the
window comes back, the open chat's unread messages are marked read then.

Screen readers: the chat list and the transcript are named lists; each row reads its whole content
(for a message: sender, time, send state, the quoted line, the text and the reactions).

## Tests

`cargo test -p nuncio-desktop`: the view data, the controller and the worker (against a fake daemon)
are plain unit tests; `tests/smoke.rs` builds the real window on GTK's main thread (its own `main`,
`harness = false`) and skips when GTK cannot start.
