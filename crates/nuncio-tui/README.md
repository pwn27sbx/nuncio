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

- **Chat names**: a chat shows its `name` from the daemon (a group's subject, else the contact's
  address-book, business or push name), else its `phone` (the number masked to its last four
  digits, `…0001`), else, as `nuncio chats` does, a phone-number chat's own number masked the
  same way (an older daemon sends no `phone`), else `unnamed group` for a group or `hidden number`
  for anyone else. A LID chat's digits are never shown: they are a hidden id, not a number (ADR
  0002).
- **Chat list** (left): name and unread count `(n)`, then `★` pinned, `⊘` muted, `▤` archived in
  a fixed column at the end of the row (one cell apart, a missing marker left blank), so the
  markers line up whatever the name; a name too long is cut with `…` before the column. In a list
  too narrow for that column (under six columns left for the name), only a chat's own markers are
  drawn, right after its name.
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
- **Focus**: one pane has the keys at a time, the chat list or the conversation (the composer
  counts as the conversation); it is drawn with a thick border in the theme's accent colour, the
  other with a thin dim one. `Tab` switches between them.
- **Footer**: always one line of hints. Outside the composer it starts with
  `? help · Tab switch pane · Enter open · / search · q quit` (`Enter write` in the conversation)
  and adds the focused pane's keys after it; in the composer, where `q` and `/` are text, it says
  how to send and `Esc conversation · Ctrl-C quit`. `?` shows every key, grouped by context.
- **Selected message**: in the conversation, `k`/`↑` select a message (`›`, the newest first,
  then older) and `j`/`↓` a newer one; `r` replies to it, `e` reacts, `R`
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

Everything works from the keyboard. `?` shows these keys on screen, grouped the same way.

`Tab` switches the focus between the chat list and the conversation; `j`/`k` and the arrows move
inside the focused pane. `Enter` on a chat opens it with the composer ready to type, as WhatsApp
does; `Esc` goes back one step: composer → conversation → chat list.

| Where | Keys | Does |
|---|---|---|
| Chats | `j`/`k`, `↓`/`↑` | move to the next / previous chat |
| Chats | `Enter` | open the chat with the composer focused, marking it read on WhatsApp too (blue ticks, unless read receipts are off) |
| Chats | `l`, `→` | open the chat to read, without the composer |
| Chats | `Tab` | focus the conversation |
| Chats | `q`, `Esc` | quit |
| Conversation | `j`/`k`, `↓`/`↑` | select a newer / older message (`k` first selects the newest) |
| Conversation | `r` | reply to the selected message (the composer quotes it); only a sent or received one |
| Conversation | `e` | react to it (see Reactions); only a sent or received one |
| Conversation | `R` | retry the selected send that is not sent or not confirmed (same message, never a second) |
| Conversation | `o` | download the selected media and open it externally |
| Conversation | `Enter`, `i` | write (focus the composer) |
| Conversation | `PgUp`/`PgDn` | scroll a page |
| Conversation | `g`/`G`, `Home`/`End` | scroll to the oldest / newest message |
| Conversation | `Esc`, `h`, `←`, `Tab` | back to the chat list |
| Composer | `Enter` | send |
| Composer | `Alt-Enter`, `Ctrl-J` | a new line |
| Composer | `←`/`→`, `Home`/`End` | move the cursor (`Home`/`End`: start / end of the line) |
| Composer | `Backspace`/`Delete` | erase before / after the cursor |
| Composer | `Esc` | back to the conversation (the message selection) |
| Search | `/` (chats or conversation) | search this chat: type, `Enter` search, `j`/`k` move, `Enter` select, `/` edit, `Esc` close |
| Reactions | `1`-`6` | 👍 ❤️ 😂 😮 😢 🙏; `x` removes mine, `Esc` cancels |
| Everywhere | `?` | show or hide the key help (outside the composer, where `?` is text) |
| Everywhere | `Ctrl-C` | quit (`q` quits outside the composer) |

## Mouse

The TUI turns on the terminal's mouse reporting:

- a left click on a chat selects it and opens it with the composer focused (as `Enter`);
- a click in the conversation focuses it and selects the message clicked (its header or its text);
- a click in the composer focuses it;
- the wheel scrolls the pane under the pointer: three lines of the conversation, or one chat in
  the list (which moves the selection, as `j`/`k` do).

Clicks do nothing while the help, the reaction picker or the search is open. With mouse reporting
on, the terminal no longer selects text on a plain drag: **hold Shift while dragging** to select
and copy text (foot, alacritty and ghostty all work this way; a few terminals use another
modifier).

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
