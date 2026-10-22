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

Pictures are drawn in the conversation (see [Pictures](#pictures)): from the thumbnail each image
or sticker message carries, with the full picture only when you press `o`. Nothing is ever
downloaded on its own.

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
- **Group senders**: in a group, an incoming message's header names who wrote it, the daemon's
  `sender` (her name, else her masked number, else `unknown`); without one (an older daemon, a
  message stored before senders were recorded) it shows the group's name as before.
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
  (it waits for a manual retry) or `not confirmed` (WhatsApp's acknowledgement or the daemon's
  answer timed out, so it may have been delivered). The TUI keeps that mark in memory only:
  after a restart the daemon lists such a send as `not sent`, since the wire has no not-confirmed
  message state. A connection that dies silently still counts as linked for up to 3 minutes,
  until its keepalive fails, so a send in that window shows `not confirmed` rather than `queued`.
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
  nothing is ever retried on its own), `o` shows a picture in full (other media: downloads and
  opens it), `O` downloads any media and opens it externally. Only a message WhatsApp has (received, or a send of mine
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
- **Media**: image, sticker, audio, video and document messages show `[kind]` before their
  caption (`[sticker] ▶` for an animated sticker). Pictures are drawn under it (see
  [Pictures](#pictures)). On audio, video and documents `o` asks the daemon to download the file
  and opens it with `xdg-open` (`open` on macOS); `O` does the same for a picture.
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
| Conversation | `o` | on a picture: fetch the full picture into its box; on other media: download it and open it externally |
| Conversation | `O` | download the selected media and open it externally (pictures too) |
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

## Pictures

An image or sticker message whose size the daemon states (`width`, `height`) gets a box under its
caption as soon as the conversation is drawn, before anything is decoded: as wide as the picture
in cells but no wider than the conversation, as tall as that keeps its shape, at most 12 rows.
Nothing moves when the picture arrives in it.

- **Thumbnail first.** The box shows the thumbnail WhatsApp embeds in the message (the wire's
  `thumb`: a JPEG for a photo, a PNG for a sticker), scaled to the box. It costs no network: the
  daemon already has it.
- **Full picture on demand.** `o` on the selected picture asks the daemon for it
  (`download_media`); the caption says `Downloading…` until it is drawn, in the same box, over the
  thumbnail. If the daemon refuses (offline, not linked) or the file does not decode, the footer
  says why and the thumbnail stays; `o` tries again. **Nothing is downloaded without a key.** `O`
  opens the picture with the desktop's viewer instead.
- **Stickers** are drawn like photos, from their PNG thumbnail; for a sticker without one (or an
  image without one) the caption's `[sticker]` (`[image]`) is the only label and the box stays
  empty until `o` fetches it (a WebP; an animated one gives its first frame). An animated sticker
  is marked `▶` and shows its first frame: no animation yet.
- **Transparency.** With half blocks every cell is painted, so a transparent pixel (a sticker's
  corners, the margin a picture leaves in its box) takes the theme's background colour, and a
  theme switch draws the pictures again on the new one. Kitty and sixel keep the transparency.
- **Scrolling** draws a picture cut at the top or the bottom of the conversation when it is only
  partly in view.
- **Untrusted bytes.** Only JPEG, PNG and WebP are decoded. The size a picture's header declares
  is checked before decoding: a thumbnail may declare at most 1024 pixels a side and 16 MiB of
  pixels, a full picture 16384 a side and 96 MiB of decode, a WebP a third of that because it
  may be animated (a 64 MiB file at most). Anything else keeps the box with its `[image]` or
  `[sticker]` label (a full picture says why in the footer). A decoded picture is shrunk to its
  box before it is encoded.
- A picture whose message states no size gets no box and no thumbnail; `o` draws the full picture
  in a box sized from the file.
- Encoded pictures are cached (48 of them, about 96 MiB) and encoded on two worker threads, so
  scrolling stays smooth; resizing the terminal encodes the visible ones again, and work queued
  for an earlier width is dropped unencoded. The caption's `Downloading…` is cut with `…` rather
  than wrap to a new row, so the box never moves.

### Which protocol

The TUI asks the terminal once at startup, before reading any key, and says the choice once in
the footer (`pictures: kitty (detected)`):

| Terminal | Protocol |
|---|---|
| Ghostty, kitty | kitty graphics with Unicode placeholders |
| foot | sixel |
| iTerm2, WezTerm | iTerm2 inline images |
| alacritty and anything that answers neither | half blocks (`▀` in colour) |
| herdr | kitty, re-rendered by herdr (0.6.0 and later); foot inside herdr is unverified (it may announce kitty and show nothing: set the variable below) |
| zellij (`ZELLIJ` set) | sixel when the terminal announces it, else half blocks (zellij rejects kitty's placeholders) |

`NUNCIO_IMAGE_PROTOCOL` chooses it without asking: `kitty`, `sixel`, `iterm2`, `halfblocks`, or
`off` (no pictures: `[image]` labels only, and `o` opens every media externally; `?` says so).
Another value is
named in the footer and ignored. The same five values under `[ui] image_protocol` in the config
file (`~/.config/nuncio/config.toml`, see [`docs/config.md`](../../docs/config.md)) choose it too;
the variable wins over the file, and a bad variable falls back to the file. A config file the TUI
cannot read, or one with a key it does not know, stops it at startup with the reason (exit 2).

The query is ratatui-image 11.1's, which reads the terminal's answer on a thread that it does not
stop when no answer comes (upstream issue #202). So the TUI queries only when stdin and stdout are
both a terminal (otherwise half blocks), waits at most 1 s, and when it timed out with no answer it
uses half blocks and makes stdin safe before the first key is read: it asks for a status report
once more and holds Rust's stdin lock for the rest of the run, so that thread can never read
again; if the terminal still does not answer, the screen asks for one key first. 100 ms later,
whatever is left of a late answer is thrown away (`tcflush`), so it never reaches the composer as
keys. Setting the variable (or the config key) skips the query.

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
  reaction, a photo, three stickers and a document.
- A send is answered `sent` at once, and about 2 s later an invented reply arrives in that chat
  (a live `event_message` and `event_unread`; `-reply-after 0` turns it off).
- A text starting with `fail` ends `not sent`; `R` on it then sends it. Until then the demo, like
  `nunciod`, refuses a reply to it or a reaction on it.
- `e` reacts, `/` searches the open chat (every word, case-insensitive).
- In "Travel group (demo)" each incoming message names its invented author (`Lena (demo)`,
  `Marco (demo)`, `…0042`), and so does the live reply.
- Pictures, in "Travel group (demo)": a photo with its JPEG thumbnail (`o` fetches a 480x320
  gradient into its box), a sticker and an animated sticker (`▶`) with PNG thumbnails, and a
  sticker with none (an empty box until `o` fetches its WebP). Each sticker's WebP is a disc of its
  own colour (yellow, pink, and blue for the one without a thumbnail), so `o` on a sticker draws
  that sticker. On the document in "Shop bot
  (demo)", `o` writes a small text file under the runtime dir and opens it; `O` opens any of them.
- Opening an unread chat clears its unread count (`mark_read`; the demo sends no receipts, since
  there is no WhatsApp).
- `wipe` (`nuncio logout`/`purge`) is refused.

`Ctrl-C` stops the demo; it removes its socket and its media files. It refuses to start without
`XDG_RUNTIME_DIR` or while something answers on the socket (a real `nunciod`).
