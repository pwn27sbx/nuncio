# nuncio: fix round after T15 (office, 2026-10-20)

Objective: the four owner-approved fixes from the first real-account run, then the plan's Parts 2–4 in order.
Branch: `feat/m2-tui` (from `9441b6c`). Delivery: `single-pr`. Runners: `TMPDIR=/tmp go test ./...`, `cargo test --workspace --exclude nuncio-desktop`.

## Specs

Source: `~/Downloads/handoff-2026-10-19-home-t15-fixes.md` (home Claude, owner-approved). Quoted verbatim:

- S1 B1: "**B1, delivery receipts are `inactive`, so the sender sees one grey tick.** The owner wants two grey ticks as normal (blue only via read receipts). Candidate: `SetForceActiveDeliveryReceipts(true)` (normal delivery receipts without going online). Verify in whatsmeow that it does not send presence `available` (AGENTS.md bans that)."
- S2 B2: "**B2, a silently dead connection is never noticed: the daemon stays `linked` forever and stops receiving.** … Fix: on keepalive failure, go `offline` and reconnect through T10's own backoff, keeping T10's stop rules. Test with a fake that times out the keepalive."
- S3 B3: "**B3, the TUI never reconnects to the daemon** (`crates/nuncio-tui/src/main.rs:148-150`, `:174-176` set `daemon = None` forever). Fix: retry with a small capped backoff, show the state in the header/footer, resubscribe on reconnect. Test with a daemon restart."
- S4 Q2 option 2: "**Q2 option 2, the phone-number ↔ LID mapping.** A send to `<pn>@s.whatsapp.net` created a second chat; the replies landed in the `@lid` chat, and quotes across the split showed `> (a message not loaded)`. Store and serve one conversation per contact, using whatsmeow's LID mapping (`Store.LIDs` / `GetLIDForPN`) or `resp.Chat`. Migrate or merge existing split chats safely: **stored data, high risk**. An ADR is welcome. Quotes must resolve across both JIDs."
- S5 Rules: "Tests first, with RED observed; GGA on each commit." "An independent verifier on high-risk changes. B2 and Q2 touch concurrency and stored data, so both are high risk." "**No AI attribution, no Co-Authored-By.**" "**No push without the owner's OK naming the remote and branch.**" "No real account actions from the Mac."
- S6 When done: "Write `handoff-2026-10-20-office.md` in this same folder (`Comunicaciones - Claudes`). It must say: the commits, with their RED/GREEN and verifier evidence; what to re-check at home with the real account (B1 two ticks, B2 by a network change/NAT idle, B3 by a daemon restart, Q2 by a send to the PN JID); open questions." Assumption: "this same folder" is the folder the home handoff came from; it reached the Mac in `~/Downloads`, so the office handoff goes to `~/Downloads` unless the owner names another place.
- S7 Parts 2–4 of the home handoff ("The full plan after the fix round"), in order, with its **ASK** points asked one at a time.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| F1 | S1, S2 (B1 + B2: internal/wa session/events) | delegated writer (context backstop), parallel with F3; Risk 4 → verifier | done: RED per rule, 10 mutants killed (3 equivalent, explained); verifier PASS S1/S2, no blockers (whatsmeow sendActiveReceipts only picks the receipt type; -race x50 concurrency probes); advisory A1 (late old-connection keepalive timeout) fixed; parent reran the keepalive tests | 8dcad27 |
| F3 | S3 (B3: TUI reconnect) | delegated writer, parallel with F1 | done: 6 tests RED→GREEN (daemon restart on the same socket, backoff 0.5 s→5 s cap, draft/selection kept, send while disconnected not retried), 10 mutants killed; parent reran `cargo test -p nuncio-tui` | 02b9926 |
| F4 | S4 (Q2 PN↔LID: store merge + ADR) | delegated writer after F1 (shares internal/wa/session.go); Risk 1 and 3 → verifier | done: RED per rule, 15 + 9 + 8 + 13 + 3 mutants killed across the batches; verifier: daemon/store PASS first time; TUI blocker → 4 corrections (owner chose A in L5, OK'd the 4th in L6) → final recheck FIXED (no path left where a draft reaches another contact); full suite green (Go -race all packages, cargo 142/0) | this commit |
| F5 | S6 handoff | delegated writer | open | |

## Log

- L1 (2026-10-20, owner): "mi claude de mi omarchy te manda esto (/Users/sebastian/Downloads/handoff-2026-10-19-home-t15-fixes.md)". The handoff says the owner approved B1, B2, B3 and Q2 option 2.
- L2 F1 evidence: whatsmeow receipt.go:255-264/287 (SetForceActiveDeliveryReceipts only stores 2; read solely to choose the delivery receipt type; no presence); keepalive.go:26-63 (KeepAliveMaxFailTime 3 min; forced reconnect only with EnableAutoReconnect). Verifier advisories left: A2 a theoretical second reconnect if Disconnected races a slow Disconnect (whatsmeow would answer ErrAlreadyConnected and it is rescheduled); A3 a logout/ban landing between offline and publish leaves subscribers' last frame offline.
- L3 F3 open point: a send whose socket read timed out or whose connection broke after writing is shown as "not sent" though the daemon may have queued it; "not confirmed" would be more honest (safe either way: R retries under the same client_id).
- L4 F4 verifier: daemon/store probes all PASS (integrity, 16-step rollback, no-mapping byte-identical, recycled number, -race x30, old/new DB); TUI B1 failed → correction 1 (suffix heuristic) → recheck: R1–R3 new/remaining → correction 2 (exact request/reply pairing, orphan keeps Compose) → recheck still failing: F1 a draft kept while in Pane::Chats goes to Grace after a restart-time merge; F2 a live merge leaves a ghost PN entry and replies miss the open chat (event_chat only announces the LID chat; a sent reply clears client_id before its event). Needs your decision raised: A explicit aliases on the wire, B commit as is, C daemon only.
- L5 (2026-10-20, owner): "A". Third correction: Chat gains optional `aliases` (PN JIDs merged into it, only while whatsmeow's mapping still points there), served in `chats` and `event_chat`; the daemon publishes `event_chat` after every merge; the TUI adopts exactly from those aliases.
- L6 Verifier recheck after option A: F1, F2, R1, R3, (a), recycled number FIXED; frames without aliases byte-identical. Still failing: adopt() with an orphan whose canonical chat is not listed (more than CHAT_LIMIT=200 chats, or a number recycled after the merge) falls back to the old list index and the draft goes to another contact. (owner, 2026-10-20): "dale" to a fourth, narrow correction.
- L7 Final F4 recheck FIXED. Checks before commit: gofmt empty, go vet ok, CGO_ENABLED=0 build ok, `TMPDIR=/tmp go test -race -count=1 ./...` all ok, cargo fmt ok, clippy clean, cargo test 142 passed / 0 failed (the macOS theme-watcher timing tests flaked once in the verifier's run; theme.rs unchanged).
