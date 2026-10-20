# nuncio-tui

The ratatui terminal client. It talks to `nunciod` over `$XDG_RUNTIME_DIR/nuncio/core.sock`
(wire v1, `proto/wire.md`): one connection for requests and one subscribed connection for live
events. With no daemon it still starts, shows `proto/fixtures/chats_response.json` and says
`FIXTURE DATA`; a send then shows as not sent.

If the daemon goes away later (it restarted, a read hit the 5 s bound), the header says
`reconnecting…` instead of the session and the footer says `daemon connection lost: …`. The TUI
tries again after 0.5 s, doubling up to every 5 s; once back it says `reconnected to nunciod`,
says hello and subscribes again, and reloads the status, the chat list and the selected chat's
messages, keeping the draft, the selected chat and message and the open pane. A send made while
disconnected is shown as `not sent` and the footer says `not connected to nunciod`; nothing
retries it by itself (`R` does, once reconnected).

One chat per contact (ADR 0001): the daemon names the phone-number chats it merged into a
contact's LID chat (`…@lid`) in that chat's `aliases`, in the chat list and in the `event_chat` it
publishes after a merge. Whatever the TUI holds under such a number (its list entry, the open or
selected chat with its draft, loaded messages, pending requests) moves to the LID chat: the
duplicate entry goes, and the selection and draft follow. A number the same chat list also lists as
a chat of its own (a recycled number) is never moved. The reply to a request about the number that
names her LID chat is a second source. If a new chat list no longer has the selected chat and
nothing explains it, the TUI never moves a draft to another chat: in the conversation or the
composer, or in the chat list with a draft written, it keeps the vanished chat selected (no chat
highlighted), Enter sends nothing and the footer says `that chat was merged; choose it again before
sending`, until the user chooses a chat again or the daemon names the chat it became. When the chat
it became is not in the list either (more chats than the list holds, or a number recycled after
the merge), the vanished chat takes its LID JID and stays blocked the same way: the selection never
falls to another chat in the list.

```
nuncio-tui [--image <png>]
```

## Screen

- **Chat list** (left): name, unread count `(n)`, `★` pinned, `⊘` muted, `▤` archived.
  Archived chats are kept in the list, sectioned after every other chat. A pin, archive or mute
  made on the phone (`event_chat`) updates the markers at once and sorts the list again as the
  daemon's `chats` lists it (newest message first, the same time by JID, archived last), keeping
  the selection on the same chat. Under the name, the
  last message and its time; a preview too long for the pane is cut with `…` (by display width,
  so wide characters count as two columns) and the ` · HH:MM` after it is always whole.
- **Times** are local: the time zone offset is read once at startup (so a message from the other
  side of a daylight-saving change shows an hour off until the next start). If the offset cannot
  be read, times are UTC and the footer says so.
- **Conversation** (right): the selected chat, newest message at the bottom. Before a chat is
  opened it shows the chat's last message only.
- **Read**: opening a chat that has unread messages marks it read, here and on WhatsApp too, as
  the official clients do: your other devices clear it and the sender sees blue ticks, unless read
  receipts are off in WhatsApp's privacy settings (then only your own devices clear it). A new
  message arriving in the open chat is marked read the same way. The unread counts clear when the
  daemon answers; if it refuses (offline, not linked, ...) the footer shows why, the chat stays
  unread, and nothing retries on its own: opening the chat again does.
- **Composer**: the last rows of the conversation. `Enter` sends; `Alt-Enter` or `Ctrl-J` starts a
  new line (use `Ctrl-J` where the terminal keeps `Alt-Enter`, as macOS terminals do unless
  Option is set to act as Alt), and the message is sent with its line breaks. `←`/`→` move the
  cursor one character (a whole emoji sequence or accented letter is one step), `Home`/`End` to
  the start or end of its line, `Backspace`/`Delete` erase before or after it. A long line wraps at
  the pane's width; the composer grows up to five rows, then scrolls to keep the cursor in view.
  A send shows `sending` until the daemon
  answers, then `queued` (offline, waiting for the reconnect), nothing (sent), `not sent`
  (it waits for a manual retry) or `not confirmed` (WhatsApp's acknowledgement timed out, so it
  may have been delivered). The TUI keeps that mark in memory only: after a restart the daemon
  lists such a send as `not sent`, since the wire has no not-confirmed message state.
- **Selected message**: `K`/`J` select a message (`›`); `r` replies to it, `e` reacts, `R`
  retries a send of mine that is `not sent` or `not confirmed` (one attempt per key press, under
  the same WhatsApp message id, so a send that did arrive is never sent as a second message;
  nothing is ever retried on its own), `o` downloads its media. Only a message WhatsApp has (received, or a send of mine
  that was sent) can be replied or reacted to: on one still `sending`, `queued` or `not sent`,
  `r`/`e` send nothing and the footer says `that message was not sent, so it cannot be replied
  to` (or `reacted to`).
- **Quotes and reactions**: a reply shows `> ` and the start of the message it quotes (or
  `(a message not loaded)`); reactions show on the line under the message, mine in the accent
  colour. A reaction the daemon refuses (offline, unknown sender, ...) shows its error in the
  footer and changes nothing. Reactions are never queued: react again once linked.
- **Search** (`/`): searches the open chat through the daemon; the results are listed newest
  first, and `Enter` on one closes the search and selects that message in the conversation
  (scrolled into view) when it is among the loaded messages; otherwise the footer says so.
- **Media**: image, audio, video and document messages show `[kind]` before their caption. `o`
  asks the daemon to download it and opens the file with `xdg-open` (`open` on macOS). Inline
  images are only the `--image` check for now: downloaded media is not drawn in the terminal.
- Live updates: new messages, a send's state changes, unread counts, the connection state, and a
  focus request (`nuncio focus <jid>`) selects and opens that chat.
- **A newer daemon**: an event this TUI does not know is ignored (the event connection stays
  up); a reply it does not know shows `daemon: unknown reply` in the footer, and a send it
  answered shows `not confirmed`, since it may have been sent (`R` retries it safely).
- Colours follow the Omarchy theme live (the shared `ThemeWatcher`).
- **Emoji sequences are drawn taken apart.** ratatui gives a whole emoji sequence one width
  (`👨‍👩‍👧‍👦`, `🏳️‍🌈`, `❤️`, `👋🏽` are each 2 columns), but terminals draw some of them at
  another width (a terminal without grapheme clustering draws every code point; Ghostty drew the
  family and the rainbow flag differently too), so cells drift and leftovers stay on screen. The
  TUI therefore drops the joiner U+200D, the emoji selector U+FE0F and the skin-tone modifiers
  when it draws text, leaving characters every terminal draws at the same width. The trade-off:
  the family shows as four people, the rainbow flag as `🏳🌈`, `❤️` may look like a text heart,
  skin tones are not shown, and a joiner inside a non-emoji script is dropped too. Only the
  display changes: what is sent, quoted or reacted keeps the original text.

## Keys

Everything works from the keyboard. `?` shows this list on screen.

| Keys | Does |
|---|---|
| `j`/`k`, `↓`/`↑` | chats: move · conversation: scroll a line |
| `Enter`, `l`, `→` | open the selected chat, marking it read on WhatsApp too (blue ticks, unless read receipts are off) |
| `i` | write a message to the selected chat |
| `Enter` | in the composer: send |
| `Alt-Enter`, `Ctrl-J` | in the composer: a new line |
| `←`/`→`, `Home`/`End` | in the composer: move the cursor (`Home`/`End`: start / end of the line) |
| `Backspace`/`Delete` | in the composer: erase before / after the cursor |
| `K`/`J` | conversation: select an older / newer message |
| `r` | reply to the selected message (the composer quotes it); only a sent or received one |
| `e` | react to it: `1`-`6` 👍 ❤️ 😂 😮 😢 🙏, `x` removes mine, `Esc` cancels; only a sent or received one |
| `R` | retry the selected send that is not sent or not confirmed (same message, never a second) |
| `o` | download the selected media and open it externally |
| `/` | search this chat: type, `Enter` search, `j`/`k` move, `Enter` select, `/` edit, `Esc` close |
| `Esc` | unselect the message; composer → conversation → chats; quit from the chats |
| `PgUp`/`PgDn` | scroll a page |
| `g`/`G`, `Home`/`End` | scroll to the oldest / newest message |
| `Tab` | switch between chats and conversation |
| `?` | show or hide the key help |
| `q`, `Ctrl-C` | quit |

## Try the TUI with the demo daemon

`nuncio-demo` (`cmd/nuncio-demo`) is a stand-in daemon with invented chats in memory and no
WhatsApp code path: nothing is sent anywhere. From the repository root, in two terminals:

```sh
# terminal 1 (creates the runtime dir with mode 0700 if it is missing)
mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo go run ./cmd/nuncio-demo
# terminal 2
XDG_RUNTIME_DIR=/tmp/nuncio-demo cargo run -p nuncio-tui
```

What it gives you to try:

- Six chats with fake `555` numbers: one pinned, one muted, one archived, one group, one without a
  name, several unread, and one with 240 messages; emoji/ZWJ, CJK and RTL text, a quoted reply, a
  reaction, an image and a document.
- A send is answered `sent` at once, and about 2 s later an invented reply arrives in that chat
  (a live `event_message` and `event_unread`; `-reply-after 0` turns it off).
- A text starting with `fail` ends `not sent`; `R` on it then sends it. Until then the demo, like
  `nunciod`, refuses a reply to it or a reaction on it.
- `e` reacts, `/` searches the open chat (every word, case-insensitive), `o` writes a small PNG or
  text file under the runtime dir and opens it.
- Opening an unread chat clears its unread count (`mark_read`; the demo sends no receipts, since
  there is no WhatsApp).
- `wipe` (`nuncio logout`/`purge`) is refused.

`Ctrl-C` stops the demo; it removes its socket and its media files. It refuses to start without
`XDG_RUNTIME_DIR` or while something answers on the socket (a real `nunciod`).
