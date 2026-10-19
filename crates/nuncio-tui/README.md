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
| `Esc` | composer → conversation → chats; quit from the chats |
| `PgUp`/`PgDn` | scroll a page |
| `g`/`G`, `Home`/`End` | scroll to the oldest / newest message |
| `Tab` | switch between chats and conversation |
| `?` | show or hide the key help |
| `q`, `Ctrl-C` | quit |
