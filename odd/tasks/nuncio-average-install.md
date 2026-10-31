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
| T2 | S13: the daemon can bootstrap on first run — a "no device yet" state that opens the store and creates the data key, reached only when a link is asked | inline | done: RED (undefined `openLinkBootstrap`) → GREEN; tmpfiles' empty 0700 dir yields one 0600 store, one data key and an unpaired device with no device row written; a linked store is refused before the keyring is read, byte for byte unchanged, and its device still opens; a stopped keyring refuses with `store.ErrKeyringUnavailable` and leaves no key and no device; a key the flow did not create is never reported as its own | this commit |
| T3 | S4/S5/S9: the wire carries the flow — `link_start`, `link_cancel`, `link_started`, `link_cancelled`, `link_qr`, `link_state`, with fixtures and both contract suites | inline | done: RED (9 unresolved types in both suites) → GREEN; 8 fixtures, spec table rows plus a "The link flow" section (no quiet zone, the QR never logged, rows as the only representation), Go types in `internal/ipc`, Rust variants with the `link_qr`/`link_state` serde renames; `go test ./internal/ipc/...` and `cargo test -p nuncio-client` green, the 177-module frame is 31.9 kB against a 1 MiB cap | this commit |
| T4 | S4/S5/S8: the daemon serves it — handler, QR rotation, cancel, fixed failure sentences | inline | done: RED (undefined `LinkStarter`, `LinkCanceller`, `MaxLinkQRSize`, `LinkFailureReason`) → GREEN; `link_start` without consent (false or absent) is refused with `bad_frame` before the daemon opens anything, and the flow gates again with `wa.Gate`; one `link_qr` per code, `size`+`rows` only, at most 177 modules per side; `waiting` → codes → `linked`, and a stop carries exactly `WhatsApp stopped the linking: ask for a new QR`; cancel removes the store and only the key this flow made, and a linked store is refused before the keyring is read; a paired device is handed to the daemon, which serves it without a restart; no frame, log line or error carries the payload | this commit |
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
- L5 (evidence, 2026-10-31, T3) The wire half landed with 8 fixtures. Three things a later unit
  must not forget, all found while implementing: (a) `crates/nuncio-client/src/conn.rs`'s
  `is_event` is a hand-maintained allow-list, so `link_qr`/`link_state` drop the connection until
  T6 adds them there; (b) `LinkState` is not re-exported from `nuncio-client`'s `lib.rs` yet, also
  T6; (c) the failed sentence in `proto/fixtures/event_link_state_failed.json` is
  "WhatsApp stopped the linking: ask for a new QR", and T4 must emit exactly that text (or change
  the fixture with it), and T4 owns clamping `link_qr.size` (177 modules is the largest QR).
- L6 (review, 2026-10-31, T3) Approved with two advisory findings, both binding on T4:
  (a) the tracked `event_link_qr.json` is synthetic (version 1 carries ~25 characters, far below a
  real payload) but nothing proves it, so T4 must keep the fixture synthetic by construction and
  must not add any logging or `Debug` path for `link_qr`; (b) `LinkStartRequest.Consent` is a
  client-supplied boolean with no enforcement in this slice — T4 must refuse `false` or an absent
  value (Go's zero value) before it generates or publishes a single QR.
- L7 (evidence, 2026-10-31, T2) `openLinkBootstrap` is the daemon-side path a link flow will use
  (T4): it opens/creates the store at `wa.DBPath()` and creates the data key in the keyring only
  while the store holds no device, and answers an unpaired `*wastore.Device`. It refuses with
  `errLinkRefused` before asking the keyring at all when a device is stored, so a linked device's
  store and key are never touched, and it refuses with `store.ErrKeyringUnavailable` when the
  keyring is stopped (the same refusal `nuncio link` gives today). `createdKey` says whether this
  call made the key, so T4's cancel deletes only a key the flow created; an existing (orphan) key is
  reused and never claimed. A failed bootstrap forgets the key it just made. RED reproduced by
  stashing the implementation: 6 undefined symbols in the new test. Verified: `gofmt -l cmd internal`
  and `go vet ./...` clean, `CGO_ENABLED=0 go build ./...` clean, `CGO_ENABLED=0 go test -count=1
  ./...` and `go test -race -count=1 ./...` green, `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo test --workspace` green. `packaging/check.sh` passes
  every group except `systemd-analyze --user verify`, which on this machine reports
  `Command /usr/bin/nunciod is not executable` because the package is only installed in T11; `T2`
  touches nothing under `packaging/` and CI runs that step in a container that installs the binary.
- L8 (review, 2026-10-31, T2) Approved (medium tier, lenses risk and reliability) with four
  advisory findings. None blocked and none reopens this candidate; all four are inputs for T4 and
  are recorded so they are not lost: (a) `openLinkBootstrap` carries no consent check of its own —
  the only barrier is the caller's `wa.Gate`, so T4 must reach it behind the gate and never at
  daemon start; (b) the 0700 assertion in the bootstrap test re-reads the mode its own helper set,
  so it proves nothing about the daemon — the real directory is proven by T1's tmpfiles check and
  by T11's fresh-profile run, and a future test must not claim more; (c) the failure path discards
  `ForgetDataKey`'s error, so a keyring that refuses the delete leaves a key no later flow claims —
  T4's cancel must surface a failed delete instead of ignoring it; (d) the same root from the
  reliability lens: a cleanup failure after the key was created is untested.
- L9 (evidence, 2026-10-31, T4) The daemon serves the flow in two commits. The wire half adds
  `LinkStarter`/`LinkCanceller` (optional, like `Searcher`, so a Handler written before them still
  compiles), the dispatch of `link_start`/`link_cancel`, `ipc.LinkFailureReason` (the exact fixture
  sentence) and `ipc.MaxLinkQRSize` (177), and documents the refusal in `proto/wire.md`: a
  `link_start` without consent is answered `bad_frame` before anything opens. The daemon half adds
  `linkFlow`: one flow at a time (`errLinkBusy`), the QR encoded with `rsc.io/qr` (now a direct
  dependency; `qrterminal` already used it) into `size`+`rows`, `waiting`/`linked`/`failed` states,
  and `discard` on every non-pairing end (the store it opened, and only the data key it created).
  Pairing hands the device back through `daemon.linkedDevice`, which opens the session and swaps it
  in: `session`/`db` now sit behind a `sync.RWMutex` (`sess()`/`store()`/`handOver`) because a flow
  replaces them while requests are served, and the unlinked session it replaces is inert. Two bugs
  found in self-review before the review: a finished flow left `f.run` set, so a second `link_start`
  would have answered `errLinkBusy` forever (now cleared by the flow itself, with a test), and a
  `link_cancel` during a start answered before the cleanup (now it waits for the start's discard).
  Tests never reach WhatsApp: `prepareLink` is a fake Linker, `openDaemonSession` hands the daemon a
  session that never connects, and every keyring is a fake. Verified: `gofmt`, `go vet ./...`,
  `CGO_ENABLED=0 go build ./...`, `CGO_ENABLED=0 go test -count=1 ./...`, `go test -race -count=1
  ./...`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` green; `packaging/check.sh` green except `systemd-analyze --user verify`
  (needs `/usr/bin/nunciod`, installed in T11).
