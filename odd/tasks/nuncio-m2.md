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
- S7 (owner, 2026-10-19: "no podemos probar el TUI aqui?" → demo daemon proposal → "dale"): a demo daemon with invented chats, no WhatsApp, that answers sends, reactions, search and downloads, so the whole TUI flow can be tried by hand on the Mac.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| U1 | S1 (list, conversation, send, keyboard, palette), S2 (sizes, emoji/RTL, 200 msgs) | delegated writer (context backstop + parallel with U2) | done: 10 app + 9 render tests, 7 mutants killed, clippy clean; parent reran `cargo test -p nuncio-tui` 26/0 | 1ff7ec1 |
| U2 | S4 (search, reply, react: Go daemon + proto + Rust wire types) | delegated writer (context backstop + parallel with U1); Risk items 1 and 3 → independent verifier | done: RED per rule, 19 mutants killed; verifier S4 FAIL (B1) → one correction → recheck FIXED; S5 PASS; parent reran the B1 and outbox-transaction tests green | this commit |
| U3 | S1 (reply/react/search/media in the TUI over U2's ops) | delegated writer, after U1+U2 (context backstop) | done except inline images (L6): RED per rule, 11 mutants killed, one correction batch (refused download/search), cargo 82/0; parent reran `cargo test -p nuncio-tui` | d866f0d |
| U5 | S7 | delegated writer (context backstop) | done: 13 tests RED→GREEN, 5 mutants killed, smoke run over the socket, socket removed on SIGINT; parent reran `go test -count=1 ./cmd/nuncio-demo` | this commit |
| U4 | S6 push + report | inline | done: final checks green (L8), pushed to origin/feat/m2-tui | this commit |

## Log

- L1 (2026-10-19, owner): "okey hazlo para avanzar todo lo que podamos aqui con el limite de tiempo de las 5 pm asi que 5 pm ya no debe haber nada aqui trabajandose y todo debe estar pusheado"
- L2 Push authorized by L1 for `origin` / `feat/m2-tui`. Started 14:32; writing stops at about 16:20 so GGA, the final suite and the push fit before 17:00.
- L2b (2026-10-19, owner, supersedes the time box in S6/L2): "nono que terminen el trabajo olvida lo de las 5 pm". Push to `origin` / `feat/m2-tui` when done still stands.
- L2c (2026-10-19, owner): "pero por ejemplo si son las 4:30 y quieren empzezar una nueva fase ahi si nos detenemos porque no dara tiempo a menos que sea algo muy pequeño". Rule: tasks already running finish; no new task (e.g. U3) starts at or after 16:30 unless it is very small; then commit and push what is done.
- L3 Wire choice for S4: `search` op {`jid`, `query`, `limit`} → `messages` reply; `send` gains optional `reply_to` (message id); `react` op {`jid`, `id`, `emoji` (empty = remove)} → `reacted`; Message gains optional `reply_to` and `reactions`. Additive, v stays 1.
- L4 U2 verifier (independent, scratch copy): B1 react on an M1 group row with no recorded sender built a from-me key and answered `reacted` → fixed (refused with "the sender of that message is unknown, so the reaction was not sent"), recheck FIXED. Advisory A7 (outbox + quote not in one transaction) fixed in the same batch. Open advisories for later: a reaction arriving before its target is not in the target's first event; reused client_id with an unknown reply_to answers send_failed; reply/react cannot name my own still-queued send; quoting media sends caption-only text; own-JID participant is the PN even in @lid chats; incoming captioned media replies do not record reply_to; GetMessage loads the whole chat's reactions; emoji search finds nothing (FTS tokenizer).
- L5 Also in the U2 commit: `nuncio-tui` Message literals gained `reply_to`/`reactions` (mechanical, delegated writer, 10 lines).
- L6 U3 left out from S1: inline images of downloaded media in Ghostty. The `image` crate is built with `png` only (WhatsApp photos are JPEG) and a startup terminal query is a risk on foot/alacritty that only the Linux matrix can check. Media opens externally (`open`/`xdg-open`). Also: reply/react cannot target my own send that has no id yet.
- L7 Follow-up commit bc65acf: reaction build errors no longer quote JIDs (GGA observation on 37f406b).
- L8 Final checks on d866f0d (15:12): gofmt empty, go vet ok, CGO_ENABLED=0 build ok, `TMPDIR=/tmp go test ./...` all 6 packages ok, cargo fmt ok, clippy -D warnings clean, cargo test 82 passed / 0 failed, attribution grep empty. Next at home: the terminal matrix (foot/alacritty/ghostty) for S2/S3, inline images (L6, needs the `jpeg` feature of `image`), the U2 advisories in L4.
- L9 (owner): "no podemos probar el TUI aqui? y ya no podemos avanzar anda mas aqui?" then "dale" to the demo daemon (S7, U5).
