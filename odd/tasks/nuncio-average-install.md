# nuncio — link a device inside the app (the average-user install, 2026-10-31)

Objective: a user pastes the install line, opens **nuncio** from the launcher, sees a QR, scans
it, and chats — no terminal, no `systemctl`, no CLI after the install line.
Branch: `feat/link-in-app`, from `main@e1bef8d`. Delivery: `ask-on-risk` (single-pr when the
count stays under ~400 authored lines; otherwise chained). Test runners:
`CGO_ENABLED=0 go test ./...`, `cargo test --workspace`, `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `packaging/check.sh`.

## Specs

- S1 (owner, 2026-10-27, verbatim): "ya lo instale pero la instalacion no esta para un usuario
  promedio eso de systemctl nada que ver la instalacion debe ser sencilla y amigable para el
  usuario".
- S2 (owner, 2026-10-31, verbatim): "si terminalo para poder probarlo con la instlar de usuario
  promedio" — finish the average-user install so it can be tested.
- S3 Approved plan: `~/Documents/ObsidianVault/nuncio/2026-10-27-instalacion-sin-terminal/README.md`
  (v2, cold-audited). Its §3 target flow and §4 P2–P5 are the specification; P0 (the clients start
  the daemon through the session bus) and P1 (the app enables the login item) are already in
  `v0.1.1`. This feature is P2, P3, P4 and P5.
- S4 Target flow: paste the one line; open **nuncio** from the launcher; the first screen says
  "Link this device to WhatsApp" with the consent text, a **QR** and one **Link** button; scan
  from the phone; the chat list appears **without restarting the app**.
- S5 The QR is drawn, not printed as text: the daemon owns generation, the client draws the
  matrix (no new Rust dependency).
- S6 Linking is gated by explicit consent given in the app; the consent reason never reaches the
  logs (repo rule, `AGENTS.md`).
- S7 `Esc`/Cancel leaves no device, no store and no keyring entry behind.
- S8 The QR refreshes while it waits. A ban or a logout during pairing shows one fixed sentence
  and offers a new QR; it is not a `stop_reason`.
- S9 `nuncio link --qr --consent` keeps working unchanged for power users, as a thin client of
  the same wire frames (it is the only path that exists today).
- S10 TUI, unlinked: `No device linked · open nuncio to link it` — no CLI instruction.
- S11 Unlink from the app: a Settings row **Unlink this device** with a typed confirmation,
  calling the daemon's existing logout path.
- S12 `INSTALL.md` is install line, open, scan, uninstall, and trouble-shooting without a
  terminal step; the CLI moves to `docs/COMMAND-LINE.md`. Every `systemctl` line disappears from
  the install path.
- S13 The unit keeps `ProtectSystem=strict` and the daemon's write budget is exactly its own data
  directory: a user-tmpfiles entry creates it `0700` before the unit starts (the unit pulls that
  service in with `Wants=`/`After=`), so nothing else under `~/.local/share` — other
  applications' stores, the keyring's own, and `~/.local/share/applications`, from which a
  launcher entry is selected and executed — becomes writable by the process that parses
  untrusted WhatsApp input.
- S14 Repo rules hold: private directories `0700` and files `0600`, no secrets in the tree, no
  real phone numbers, no new automation surface, no reconnect storm, nothing links or unlinks
  without consent.
- S15 End to end: the package builds, installs on this machine and the flow works from a fresh
  data directory with `nunciod` neither running nor enabled.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| T1 | S13: the unit keeps the daemon's write budget to its own data dir and pulls in the user-tmpfiles service that creates it 0700 before start; `check.sh` pins the exact single-path `ReadWritePaths` and the tmpfiles line | inline | done: RED (no tmpfiles entry, `check.sh` unguarded) → GREEN; the temporary `ReadWritePaths=%h/.local/share` was rejected by review as a sandbox widening and replaced | this commit |
| T2 | S13: the daemon can bootstrap on first run — a "no device yet" state that opens the store and creates the data key, reached only when a link is asked | inline | pending | — |
| T3 | S4/S5/S9: the wire carries the flow — `link_start`, `link_qr`, `link_state`, `link_cancel`, with fixtures and both contract suites | inline | pending | — |
| T4 | S4/S5/S8: the daemon serves it — handler, QR rotation, cancel, fixed failure sentences | inline | pending | — |
| T5 | S9: `nuncio link` becomes a thin client of the frames, unchanged from the outside | inline | pending | — |
| T6 | S4/S5: `nuncio-client` parses the frames and exposes the link flow to both clients | inline | pending | — |
| T7 | S4/S6/S7/S8: the GTK app's link screen — consent, drawn QR, Link, cancel, error states, then the chat list | inline | pending | — |
| T8 | S10: the TUI's unlinked copy stops pointing at the CLI | inline | pending | — |
| T9 | S11: unlink from the app with a typed confirmation | inline | pending | — |
| T10 | S12: `INSTALL.md` for the average user, the CLI into `docs/COMMAND-LINE.md`, CHANGELOG and README | inline | pending | — |
| T11 | S15: build the package, install it, run the flow from a fresh data dir | inline | pending | — |

## Log

- L1 (owner, 2026-10-31, verbatim): "si terminalo para poder probarlo con la instlar de usuario
  promedio" — authorizes finishing the average-user install, i.e. P2–P5 of the 2026-10-27 plan.
- L2 (evidence, 2026-10-31) Baseline on `main@e1bef8d`: `CGO_ENABLED=0 go test ./...` green
  (cmd/nuncio, cmd/nuncio-demo, cmd/nunciod, internal/ipc, internal/media, internal/store,
  internal/wa) and `cargo test --workspace` green (every suite, doc-tests included).
- L3 (evidence, 2026-10-31) P0 and P1 are already shipped in `v0.1.1`:
  `crates/nuncio-client/src/bus.rs:37` (`StartServiceByName`) and `bus.rs:59`
  (`EnableUnitFiles`). What is missing is P2 (the daemon as linker), P3 (the app's link screen
  and the TUI copy), P4 (unlink from the app) and P5 (the docs).
- L4 (review, 2026-10-31) T1's first form widened `ReadWritePaths` to `%h/.local/share` with the
  keyring store carved back read-only. The risk lens found it a BLOCKER (a writable
  `~/.local/share` exposes `applications/` and every sibling store to the daemon), the refuter
  corroborated it, and the targeted validator then rejected the narrowed correction because
  nothing created the directory any more. Both are right. The final form keeps the narrow path
  and has `systemd-tmpfiles` create the directory: `/usr/share/user-tmpfiles.d/nuncio.conf`
  (`d %h/.local/share/nuncio 0700`) pulled in by `Wants=`/`After=systemd-tmpfiles-setup.service`.
  Verified on the real user manager: tmpfiles creates the path 0700, a unit with the narrow
  `ReadWritePaths` under `ProtectSystem=strict` writes inside it, and
  `~/.local/share/applications` is not writable. The first transaction escalated without a
  receipt, so its boundary does not advance; the final state is reviewed again.
