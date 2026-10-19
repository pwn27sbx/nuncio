# nuncio-tui

The ratatui terminal client. It talks to `nunciod` over `$XDG_RUNTIME_DIR/nuncio/core.sock`
(wire v1, `proto/wire.md`): one connection for requests and one subscribed connection for live
events. With no daemon it still starts, shows `proto/fixtures/chats_response.json` and says
`FIXTURE DATA`; a send then shows as not sent.

```
nuncio-tui [--image <png>]
```

## Screen

- **Chat list** (left): name, unread count `(n)`, `★` pinned, `⊘` muted, `▤` archived.
  Archived chats are kept in the list, sectioned after every other chat.
- **Conversation** (right): the selected chat, newest message at the bottom. Before a chat is
  opened it shows the chat's last message only.
- **Composer**: the last row of the conversation. A send shows `sending` until the daemon
  answers, then `queued` (offline, waiting for the reconnect), nothing (sent) or `not sent`
  (it waits for a manual retry).
- **Selected message**: `K`/`J` select a message (`›`); `r` replies to it, `e` reacts, `R`
  retries a send of mine that is `not sent` (one attempt per key press; nothing is ever retried
  on its own), `o` downloads its media.
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
- Colours follow the Omarchy theme live (the shared `ThemeWatcher`).

## Keys

Everything works from the keyboard. `?` shows this list on screen.

| Keys | Does |
|---|---|
| `j`/`k`, `↓`/`↑` | chats: move · conversation: scroll a line |
| `Enter`, `l`, `→` | open the selected chat |
| `i` | write a message to the selected chat |
| `Enter` | in the composer: send |
| `K`/`J` | conversation: select an older / newer message |
| `r` | reply to the selected message (the composer quotes it) |
| `e` | react to it: `1`-`6` 👍 ❤️ 😂 😮 😢 🙏, `x` removes mine, `Esc` cancels |
| `R` | retry the selected send that is not sent |
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
- A text starting with `fail` ends `not sent`; `R` on it then sends it.
- `e` reacts, `/` searches the open chat (every word, case-insensitive), `o` writes a small PNG or
  text file under the runtime dir and opens it.
- `wipe` (`nuncio logout`/`purge`) is refused. Like `nunciod`, opening a chat does not mark it read.

`Ctrl-C` stops the demo; it removes its socket and its media files. It refuses to start without
`XDG_RUNTIME_DIR` or while something answers on the socket (a real `nunciod`).
