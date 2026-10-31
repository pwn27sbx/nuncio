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
| T4 | S4/S5/S8: the daemon serves it — handler, QR rotation, cancel, fixed failure sentences | inline | done: RED (undefined `LinkStarter`, `LinkCanceller`, `MaxLinkQRSize`, `LinkFailureReason`) → GREEN; `link_start` without consent (false or absent) is refused with `bad_frame` before the daemon opens anything, and the flow gates again with `wa.Gate`; one `link_qr` per code, `size`+`rows` only, at most 177 modules per side; `waiting` → codes → `linked`, and a stop carries exactly `WhatsApp stopped the linking: ask for a new QR`; cancel removes the store and only the key this flow made, and a linked store is refused before the keyring is read; a paired device is handed to the daemon, which serves it without a restart; no frame, log line or error carries the payload | f2cfc78 (wire), 70b44ab (daemon), fixes 261c69a, 0efa996, c1bb599 |
| T5 | S9: `nuncio link` becomes a thin client of the frames, unchanged from the outside | inline | done: RED (undefined `RenderMatrix`, `Client.LinkStart`/`LinkCancel`/`Subscribe`/`NextEvent`, `linkByQR` — `go vet` refused the three packages) → GREEN; `--qr` subscribes, sends `link_start` with consent, draws each published matrix and stops at `link_state`, opening no store and no keyring; `--phone` keeps the direct path and its refusal; Ctrl-C sends `link_cancel`; three intended output changes recorded (below) | f954648 |
| T6 | S4/S5: `nuncio-client` parses the frames and exposes the link flow to both clients | inline | done: RED (7th event fixture dropped the connection with `Unexpected { expected: "event", got: "link_qr" }`, then `LinkFlow`/`LinkState`/`LinkUpdate` unresolved at the crate root) → GREEN; `is_event` accepts `link_qr`/`link_state`, every event fixture is received through `next_event`, and one `LinkFlow` folds the flow with `LinkQr` (row-first, bounds-safe `module()`, `Debug` without the matrix) and the daemon's failure sentence verbatim | 4378882 |
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
- L10 (review, 2026-10-31, T4) Approved with observations only, and five of them were real and
  fixed in two follow-up commits on the same task branch: a cancelled flow cleared `f.run` before
  its cleanup finished (a `link_start` in that window could start a second flow over the files the
  first was deleting), a cancelled flow published the failure sentence (saying WhatsApp stopped what
  the user stopped), a handover could install a session while a wipe deleted the data (the `closed`
  flag is read under the daemon's lock and a wipe sets it before it closes the session), the flow
  never cancelled its own context (the linker's QR producer could stay blocked), and the flow's
  client was disconnected only after the daemon had opened the same device. The gate itself changed
  in the same PR: `gga`'s `claude` provider is out of quota, `codex` returns nothing and `gemini` is
  retired, so `.gga` now reviews with `opencode:opencode-go/deepseek-v4.1-flash#high` through the
  tool-free `gga-reviewer` agent, and `STRICT_MODE` stays on.
- L11 (incident, 2026-10-31, T4) The review's bounded correction landed (c1bb599) but its targeted
  validator could not be captured: the provider task for `provider_targeted_validator` carries
  `lineage_id`, `revision`, `target_identity`, `repository_context` and `role`, and the capture
  arguments carry `lineage`, `expected-revision`, `target`, `repository-context`, `agent`, `role` —
  neither a `request_hash` nor the validation request itself, while `review inspect-candidate
  --purpose targeted-validation` requires "the exact provider-issued repository context, revision,
  lineage, target, and request hash". The relay refused twice with
  `opencode_review_transport_relay_refused (reason: output_refused, cause: role_capture_failed)` and
  STATUS keeps re-offering the identical slot, so the lineage stays in `correction_required` with no
  runnable exit. This is the equivalent already tracked as
  https://github.com/Gentleman-Programming/gentle-ai/issues/4972 (open, same contract and runtime),
  on which one occurrence was added with the observed evidence and no label changed; the installed
  build is 4.3.1 stable, so no published fix is claimed. The correction is verified by the gates
  (`gofmt`, `go vet`, `CGO_ENABLED=0 go test ./...`, `go test -race ./cmd/nunciod/`, the GGA
  review) but carries no native receipt; its authority is not burned, and a fresh review of the
  corrected commit can start when the defect is fixed. T5 proceeds.
- L12 (evidence, 2026-10-31, T6) The client half landed in one commit. `conn.rs`'s `is_event` gains
  `EventLinkQr`/`EventLinkState`, so a subscribed client no longer drops the connection on the first
  frame the daemon publishes (L5a), and `EVENT_FIXTURES` now lists all seven event fixtures, so a
  new event kind without a client test is noticed there. `LinkState` is re-exported (L5b) and
  `crates/nuncio-client/src/link_flow.rs` adds `LinkFlow`: `apply(&DaemonFrame) -> Option<LinkUpdate>`
  folds the two event ops and leaves every other frame to the client, `qr()` holds the live code
  (`LinkQr`: `size()`, `rows()`, row-first `module()` that reads light outside the matrix, and a
  hand-written `Debug` that prints the size only, because the matrix is pairing material),
  `phase()` is `None` until the first `link_state`, and `reason()` carries the daemon's sentence
  verbatim (a state frame without a reason clears the previous one, so a later failure cannot show a
  stale sentence). A linked or failed flow drops the code. The handle owns no connection, so the
  terminal client folds frames in its own loop and the desktop app from its worker thread, and a
  client that did not start a flow folds the same frames. Named `LinkFlow` because
  `nuncio_client::Link` is the driver's request/event pair (L5c). Verified: RED reproduced twice
  (the fixture through `next_event`, then the unresolved exports), `cargo fmt --check`, `cargo
  clippy --workspace --all-targets -- -D warnings` over both clients, `cargo test --workspace`
  (25 suites, doc-tests included), `gofmt -l cmd internal` empty, `go vet ./...`, `CGO_ENABLED=0 go
  build ./...` and `CGO_ENABLED=0 go test ./...` green; `packaging/check.sh` green except
  `systemd-analyze --user verify` (needs `/usr/bin/nunciod`, installed in T11). The GGA hook passed
  with advisories only, all of them in code this commit did not touch.
- L13 (review, 2026-10-31, T6) Approved (medium tier, lenses risk, readability and reliability) and
  acknowledged with its authority burned — no repeat of L11 this time, because the three lens
  captures were admitted on the first relay. Six advisory findings, none blocking and none reopening
  the candidate; three are inputs for T7 and are recorded so they are not lost: (a) `LinkQr` keeps
  two representations of one dimension — the frame's `size` and the actual `rows` — and `module()`
  bounds by the rows, so a screen MUST draw through `module()` and never index `rows()` by `size()`
  (risk R1-QR-SHAPE, readability R2-1, reliability R3-001); (b) `reason()` is only displayable while
  `phase()` is `Some(LinkState::Failed)`, since every state frame refreshes it (readability R2-2), so
  T7 shows it in the failed state only; (c) the failure sentence is stored verbatim (risk
  R1-REASON-VERBATIM), so the renderer sanitizes it instead of echoing bytes at a terminal or a
  markup sink. The readability lens also noted the crate root now carries three "link" surfaces
  (`driver::Link`, the `whatsapp:` `link` module, `link_flow`); the names stay, because renaming the
  driver's pair or the URL module is outside this task and T7 consumes `LinkFlow` as documented.
- L14 (evidence, 2026-10-31, T5) `nuncio link --qr` is a thin client of the frames: it dials twice
  (requests and a subscribed events connection), sends `link_start` with the gate's consent, draws
  every `link_qr` with `wa.RenderMatrix` (half blocks, a 4-module quiet zone, validation of size,
  row count, row width and both module characters) and ends on `link_state`: `linked` prints and
  exits 0, `failed` shows the daemon's sentence (falling back to `ipc.LinkFailureReason` when the
  frame carries none) and exits 1. It opens no store and no keyring, so a link from here and one
  from the app leave exactly the same data; `--phone` keeps the direct path and its refusal while
  `nunciod` answers, because no frame carries a pairing code yet (S9's carve-out, written into
  `docs/COMMAND-LINE.md`, the usage text and README). Ctrl-C sends `link_cancel` and reports what
  the daemon did before it answered; a `link_start` whose answer never arrives also cancels the
  flow it may have started, so the next attempt is not refused as busy. The client half adds
  `ipc.Client.Subscribe`, `NextEvent`, `LinkStart` and `LinkCancel`. **Three intended output
  changes:** `--qr` no longer refuses while the daemon answers (it needs it running); its ending no
  longer prints `systemctl --user start nunciod`; and it draws the wire matrix instead of encoding a
  payload, so `--plain-qr`'s art comes from `link_qr` (its shape is unchanged: half blocks, one
  frame per code, blank line between frames). A fourth follows from the first and is stated in the
  docs: on a machine whose store holds a device, `--qr` no longer connects it locally; the daemon
  answers with its own refusal. Verified: `gofmt -l cmd internal` empty, `go vet ./...`,
  `CGO_ENABLED=0 go build ./...`, `CGO_ENABLED=0 go test -count=1 ./...`, `go test -race -count=1
  ./cmd/nuncio/ ./internal/ipc/ ./internal/wa/`, `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` (25 suites) green; `packaging/check.sh`
  green except `systemd-analyze --user verify` (T11). An independent verifier re-ran every gate in a
  scratch copy and probed the built binary against a wire-level fake daemon: no daemon → exit 1 with
  the not-running line and nothing created; waiting → code → linked → exit 0, art, no `systemctl`
  line, no refusal, `$XDG_DATA_HOME` empty; `--plain-qr` lines art and blanks only; `failed` → the
  fixed sentence and exit 1; SIGINT → exactly one `link_cancel`; `--phone` with a daemon up →
  byte-identical refusal to the baseline; `--help` differs only in the four `link` lines. It found
  no blocker and four advisories, two of them real and fixed before the commit
  (`phoneWait`-derived "waiting up to N minutes", cancel on a lost `link_start` answer); the
  remaining two are recorded here: the 60 s `linkEventTimeout` is both the event read bound and the
  `link_start` reply bound, so a keyring unlock slower than a minute fails the command (the flow is
  cancelled first), and `wa.RenderQR` now has no production caller (`qrterminal` stays through T10,
  where the dependency can go with a `go mod tidy`).
- L15 (review, 2026-10-31, T5) Approved (medium tier, lenses risk, resilience, readability and
  reliability) and acknowledged with its authority burned; the four captures were admitted on the
  first relay. No blocker. Six advisory findings, all informational and recorded because two are
  inputs for T7 and two show the same gap: (a) reliability R3 and resilience R4 — after a successful
  `link_start`, an event-stream error or a failed draw returns without `LinkCancel`, leaving the
  daemon's flow running until it ends on its own (a retry then answers busy); self-healing, and the
  fix belongs with whatever retries the flow, so T7 should cancel on every exit path; (b)
  readability R2 `linkEventTimeout` carries two bounds (dial and read) under one name, and the two
  dial sites plus the two decode sites are copy-pasted — inputs for the next touch of
  `linkByQR`; (c) readability R2 `RenderQR` has no production caller now. The risk lens found
  nothing: consent is enforced in the daemon (`LinkStart(false)` → `bad_frame`, never reaches the
  handler), the matrix is validated before it reaches a terminal sink and carries no escapes, and
  `NextEvent` copies the reader's buffer before handing it out.
