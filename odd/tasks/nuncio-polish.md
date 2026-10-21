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
| G3 | S1(3) T18 UnreadChanged + live UnreadCount | after G1 (shares internal/wa); Risk 4 → verifier | open | |
| G4 | S1(4) minor advisories | after G3 | open | |

## Log

- L1 (2026-10-21, owner): "si empieza por los primero 4".
- L2 G1 verifier advisories: A1 a reply already near MaxFrameLen without thumbnails can be pushed over by senders (same class as long texts at HEAD; push names cap at 25) — cap sender at ~64 runes; A2 notification body markup unchecked (pre-existing with message text). Docs to update: SECURITY.md and INTEGRATIONS.md show_preview lines, ADR 0002, README status (icon exists now).
