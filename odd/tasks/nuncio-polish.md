# nuncio: Mac-buildable leftovers (office, 2026-10-21)

Objective: build the remaining items that need no owner decision and no Linux to write.
Branch: `feat/m4-omarchy` (continues from `a028a65`). Delivery: `single-pr`. Runners: `TMPDIR=/tmp go test ./...`, `cargo test --workspace --no-fail-fast`, `packaging/check.sh`.

## Specs

- S1 (owner, 2026-10-21, verbatim): "si empieza por los primero 4" — the four items offered: (1) show who wrote each message in groups (today a group message shows the group's name); (2) the app icon in SVG that the packaging already references; (3) T18's code so the bar's unread count updates live (D-Bus `UnreadChanged` + a live `UnreadCount`), tests that need dbus-daemon skip on the Mac and run at home; (4) the minor verifier advisories recorded in the feature docs.
- S2 Rules unchanged: tests first with RED, GGA per commit, independent verifier for high-risk changes, privacy (never full numbers or LID digits on screen or in logs; names never logged), no AI attribution, no push without the owner's explicit OK naming remote and branch, never delete files you did not create, do not install dbus on the Mac to force tests.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| G1 | S1(1) group senders: wire field + store/wa + TUI + desktop | delegated writer (parallel with G2); Risk 3 → verifier | done: RED per rule, 15 mutants killed; verifier PASS, 0 blockers (no LID digits or full numbers in sender, no names in logs, 0 network calls over 200 messages, direct-chat frames and TUI renders byte-identical to baseline); parent reran cargo 425/0 + smoke, go all ok | this commit |
| G2 | S1(2) app icon SVG + packaging install | delegated writer (parallel with G1) | done: envelope + seal, scalable 128 and symbolic 16; check.sh verifies clean SVG, viewBox and every Icon= | 0454d99 |
| G3 | S1(3) T18 UnreadChanged + live UnreadCount | delegated writer after G1; Risk 3, 4 → verifier | done: coalesced notifier (200 ms window, emits only on change, never blocks the session), PropertiesChanged + UnreadChanged + Introspectable; RED per rule, 11 mutants killed; verifier B1 (alias merge lowered the total without notifying) and A1 (a failed count emitted 0) fixed, recheck FIXED; real-bus tests skip on the Mac (first run at home); parent reran the full suite | this commit |
| G4 | S1(4) minor advisories | two delegated writers in parallel (Go/docs, Rust; disjoint surfaces) | done: sender capped at 64 runes; error logs by type only; wipe errors name the entry; curl https-only on redirects; RoundingError on thiserror; config save fsyncs the directory (best effort); docs for group senders; RED + one mutant per code item; parent reran the full suite | (see commits) |

## Log

- L1 (2026-10-21, owner): "si empieza por los primero 4".
- L2 G1 verifier advisories: A1 a reply already near MaxFrameLen without thumbnails can be pushed over by senders (same class as long texts at HEAD; push names cap at 25) — cap sender at ~64 runes; A2 notification body markup unchecked (pre-existing with message text). Docs to update: SECURITY.md and INTEGRATIONS.md show_preview lines, ADR 0002, README status (icon exists now).
- L3 G3 left: a failed recount keeps the last value with no retry until the next change; the notifier is not awaited at shutdown (harmless); dbus-broker delivery of NameOwnerChanged to busctl monitor unverified (home, contrib README step 5); BarWidget.qml not linted (no qmllint on the Mac).
- L4 G4 left: internal/wa/messages.go storeMessage still logs a MessageReactions error with %v (pre-existing, same class).
