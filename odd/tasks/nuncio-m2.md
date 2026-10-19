# nuncio M2: TUI (office part)

Objective: as much of M2 as the work Mac allows before 17:00 on 2026-10-19, everything pushed.
Branch: `feat/m2-tui` (from `feat/m1-core` at `87e4d9e`). Delivery: `single-pr`. Runners: `cargo test --workspace --exclude nuncio-desktop`, `TMPDIR=/tmp go test ./...`.

## Specs

- S1 Deliverables (handoff 7b, verbatim): "chat list with unread/pin/mute/archive, conversation view, send/reply/react, in-chat search, keyboard-first navigation, the same Omarchy palette as the desktop app (the shared `ThemeWatcher`), media shown as "download and open externally" with inline images in Ghostty via `ratatui-image` where the terminal answers the query."
- S2 Acceptance (verbatim): "the whole read/reply flow is possible with no mouse; correct at 80x24 and 240x60; emoji/ZWJ and RTL text do not corrupt the frame; scrolling 200 messages stays interactive; no terminal-specific crash on foot/alacritty/ghostty."
- S3 Mac scope (verbatim): "almost all of it (TUI + client against a fake daemon). Final terminal-matrix checks on Linux." The foot/alacritty/ghostty matrix is Linux-only.
- S4 Wire additions needed by S1 that wire v1 lacks: in-chat search, reply (quote a message) and react. Additive only (new ops / optional fields); spec, fixtures and both contract suites change together (AGENTS.md). Assumption: the shape is the orchestrator's choice, recorded in L3.
- S5 Hard rules from the handoff still hold: no real WhatsApp run, no keyring tests, no attribution trailer, no message text or full numbers in logs.
- S6 Time box (owner, verbatim): "con el limite de tiempo de las 5 pm asi que 5 pm ya no debe haber nada aqui trabajandose y todo debe estar pusheado".

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| U1 | S1 (list, conversation, send, keyboard, palette), S2 (sizes, emoji/RTL, 200 msgs) | delegated writer (context backstop + parallel with U2) | done: 10 app + 9 render tests, 7 mutants killed, clippy clean; parent reran `cargo test -p nuncio-tui` 26/0 | this commit |
| U2 | S4 (search, reply, react: Go daemon + proto + Rust wire types) | delegated writer (context backstop + parallel with U1) | open | |
| U3 | S1 (reply/react/search/media in the TUI over U2's ops) | delegated writer, after U1+U2 | open | |
| U4 | S6 push + report | inline | open | |

## Log

- L1 (2026-10-19, owner): "okey hazlo para avanzar todo lo que podamos aqui con el limite de tiempo de las 5 pm asi que 5 pm ya no debe haber nada aqui trabajandose y todo debe estar pusheado"
- L2 Push authorized by L1 for `origin` / `feat/m2-tui`. Started 14:32; writing stops at about 16:20 so GGA, the final suite and the push fit before 17:00.
- L2b (2026-10-19, owner, supersedes the time box in S6/L2): "nono que terminen el trabajo olvida lo de las 5 pm". Push to `origin` / `feat/m2-tui` when done still stands.
- L2c (2026-10-19, owner): "pero por ejemplo si son las 4:30 y quieren empzezar una nueva fase ahi si nos detenemos porque no dara tiempo a menos que sea algo muy pequeño". Rule: tasks already running finish; no new task (e.g. U3) starts at or after 16:30 unless it is very small; then commit and push what is done.
- L3 Wire choice for S4: `search` op {`jid`, `query`, `limit`} → `messages` reply; `send` gains optional `reply_to` (message id); `react` op {`jid`, `id`, `emoji` (empty = remove)} → `reacted`; Message gains optional `reply_to` and `reactions`. Additive, v stays 1.
