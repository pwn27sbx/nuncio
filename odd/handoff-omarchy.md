# nuncio: handoff to the home Omarchy machine

> **Read this first.**
>
> This file is the only context a fresh Claude Code session on the owner's home Arch Linux /
> Omarchy (Hyprland) machine needs. It replaces the office handoffs that stayed on the work Mac
> (`handoff-2026-10-19-*`, `-10-20-office`, `-10-21-office`); everything that still matters from
> them is here, and where a later owner decision changed an earlier one, the later one is written
> and the change is said. Written on 2026-10-22 on the work Mac.
>
> **State in ten lines.**
>
> 1. Branch `feat/m4-omarchy`, head at or after `b9b1df1` (this file is one more commit on top),
>    fully pushed to `origin`. 106 commits over `main` (`ae35fea`).
> 2. Draft PR #1 `feat/m4-omarchy` → `main`: https://github.com/pwn27sbx/nuncio/pull/1 (label
>    `enhancement`). The repository is **public**.
> 3. Last CI run `39822059120` (`b9b1df1`): green on all six jobs.
> 4. M0–M4, T15 fixes, T17 (sealed Signal state), the polish queue and CI are built and tested,
>    almost entirely against fakes. **Nothing has run on this machine since T15 (`9441b6c`).**
> 5. Every "to confirm" decision was decided by the owner on 2026-10-22 (polish L32–L43, t17
>    L11–L13); none is open.
> 6. The home database is still the **plain** pre-T17 one of the spare account linked in M0/T15.
>    **The first start of the new `nunciod` migrates it** (T17 seal, ADR 0001 merge, ADR 0002 names).
> 7. Nothing is installed as a package yet; the systemd unit, D-Bus service files, hotkey and bar
>    widget have never run on Linux.
> 8. The D-Bus tests have never run with a real `dbus-daemon` outside CI; the desktop smoke test
>    has never run with a display.
> 9. Open follow-ups are minor (section 8); none blocks the merge.
> 10. The merge is the owner's, with "Create a merge commit", after T1–T10 pass.
>
> **Tonight, in this order.**
>
> 1. Reply to the owner in neutral Spanish (section 5). Confirm with them what they want to do tonight.
> 2. Section 6.1–6.3: get the code, check the toolchain, run every suite (no real account involved).
> 3. Section 6.4: **back up `~/.local/share/nuncio`** before any new `nunciod` starts. No backup, no
>    start.
> 4. T1 (baseline) and T6 (D-Bus tests): suites and package build, no real account.
> 5. T2 with the **T8 first-start block** (the migration happens at T2's first start, see 6.6),
>    owner present, spare account only.
> 6. T3, T4, then T5 (needs `sudo`: ask first), T7 cross-checks, the rest of T8, T9, T10.
> 7. Every failure: a feature document, a RED test, a fix on this branch, push to
>    `origin/feat/m4-omarchy` (section 6.16).
> 8. When all pass: section 7 (PR ready for review, the owner merges).

---

## Contents

0. Read this first (above)
1. What nuncio is
2. How it was built
3. Decisions and why
4. Incidents and lessons
5. Hard rules
6. Home procedure (6.1–6.16, T1–T10)
7. After T1–T10 pass: the PR, the merge, CI trigger, branch protection
8. Open follow-ups and advisories
9. Environment notes for Linux
10. Where to find more

---

## 1. What nuncio is

A native WhatsApp client for Omarchy, running as a WhatsApp **linked device**. GPL-3.0-only.
Repository: github.com/pwn27sbx/nuncio. The home clone was created in M0 (the M0 plan names
`~/Projects/nuncio`; check with `git -C ~/Projects/nuncio remote -v`, or ask the owner where it is).

### Components

| Part | Where | What it does |
|---|---|---|
| `nunciod` | `cmd/nunciod/` | Go daemon on whatsmeow. Owns the one linked-device session, the SQLite store, the outbox, media downloads; serves clients over `$XDG_RUNTIME_DIR/nuncio/core.sock` (NDJSON). Pure Go (`CGO_ENABLED=0`). Also owns the D-Bus name `io.github.pwn27sbx.nuncio` (single instance, D-Bus activation, `Status`, `UnreadCount` property, `UnreadChanged` signal). |
| `nuncio` | `cmd/nuncio/` | CLI: `link` (with `--consent`), `status`, `chats`, `send` (one recipient, one message, never stdin), `focus`, `retry`, `logout --confirm`, `purge --confirm`. |
| `nuncio-demo` | `cmd/nuncio-demo/` | A fake daemon with invented chats, no WhatsApp, no DB, no keyring; refuses to start while a real `nunciod` answers. For trying the apps. |
| `nuncio-client` | `crates/nuncio-client/` | Shared Rust library: IPC, wire types, view models (chat list, transcript, composer), config, links, notifications rules, theme and font watchers, picture decode rules, the send outbox. |
| `nuncio-tui` | `crates/nuncio-tui/` | ratatui terminal client; inline pictures through ratatui-image 11.1 (kitty, sixel, iTerm2, half blocks; override `NUNCIO_IMAGE_PROTOCOL`). |
| `nuncio-desktop` | `crates/nuncio-desktop/` | GTK 4 + libadwaita app, app id `io.github.pwn27sbx.nuncio.Desktop`: list, transcript, composer, palette (F1/Ctrl+K), settings (Ctrl+,), notifications, background mode, `whatsapp:`/`wa.me` links, pictures and animated stickers, Omarchy theme/font, Hyprland rounding, no title bar. |
| Wire protocol | `proto/wire.md`, `proto/fixtures/` | Defined once; Go and Rust contract suites both read the same fixtures. v1, additive changes only. |
| Store | `internal/store/` | SQLite (pure-Go driver, FTS5), modes 0700/0600, merge of PN/LID chats, wipe, the keyring-held data key, the sealed whatsmeow device store, the T17 migration. |
| WhatsApp layer | `internal/wa/` | whatsmeow session, backoff, events, sends, outbox drain, receipts, app-state, names. |
| Media | `internal/media/` | On-demand downloads, SHA-256 check, cache. |
| IPC | `internal/ipc/` | Framing, wire types, frame fitting (`fit.go`), socket and D-Bus names. |
| Packaging | `packaging/` | `PKGBUILD.in`, `nunciod.service` (hardened `Type=dbus` user unit), D-Bus service files, `.desktop` entries, XDG autostart, icons, `MANIFEST`, `install.sh` (install/uninstall), `package-source.sh`, `check.sh`, `tests/uninstall.sh`. |
| Omarchy glue | `contrib/omarchy/` | `hypr/nuncio.lua` (launch-or-focus hotkey, Omarchy 4 Lua), `bar-widget/` (Quickshell plugin `pwn27sbx.nuncio-unread` reading `UnreadCount` with `busctl --auto-start=no`). |
| CI | `.github/workflows/ci.yml`, `.github/dependabot.yml` | Six jobs (section 3.6). |
| Docs | `README.md`, `SECURITY.md`, `INTEGRATIONS.md`, `CREDITS.md`, `AGENTS.md`, `docs/config.md`, `docs/adr/`, `docs/bump-drill.md` | |
| Plan of record | `odd/tasks/*.md` | One feature document per stage (section 10). |

Data locations (SECURITY.md): `~/.local/share/nuncio/` (`nuncio.db`, `-wal`, `-shm`, `media/`,
`exports/`, migration backups `nuncio.db.pre-t17-<UTC>.bak`); `$XDG_RUNTIME_DIR/nuncio/core.sock`;
`~/.config/nuncio/config.toml`; keyring item `store.data_key` under service `nuncio`.

---

## 2. How it was built

### 2.1 Milestones

- **M0, account-free spikes (on this home machine, 2026-10-18; `main`, 8 commits, tip `ae35fea`).**
  Scaffold and wire contract (`744e368`), daemon skeleton and NDJSON handshake (`6019dc0`), Rust
  client + contract tests + ratatui TUI (`6197fff`), keyring store, GTK theme watcher (probe
  150.25 ms, budget 500 ms), D-Bus activation and single instance, whatsmeow bump drill (5.1 s
  cold), pure-Go SQLite with FTS5. The linking path behind `--consent` (`22f9d30`), then the owner
  linked the **spare** number (`ae35fea`). Owner's L2: the TUI is Rust + ratatui sharing a Rust
  client library with the GTK app; the Go module is only the daemon. (`odd/tasks/nuncio-m0.md`)
- **M1, core daemon (work Mac, 2026-10-19; `feat/m1-core`, 25 commits, tip `87e4d9e`).** Store
  modes, client read timeouts, CLI tests, wire frames (messages/send/focus/events), store with FTS5,
  whatsmeow session ownership, incoming messages and unread, event streaming (bounded queue, drop a
  stalled subscriber), CLI commands, backoff and stops, the offline outbox, logout/purge, media
  downloads, app-state flags, CREDITS, bump drill. (`odd/tasks/nuncio-m1.md`)
- **M2, TUI (work Mac, 2026-10-19; `feat/m2-tui`).** Chat list, transcript, reply/react/search,
  demo daemon, `mark_read`, receipt caps, `event_chat`, `not_confirmed`, multi-line composer.
  Tip `9441b6c` was the T15 run. (`odd/tasks/nuncio-m2.md`)
- **T15, first real-account run (this machine, 2026-10-19, owner at both phones).** 10 real
  messages (6 in, 4 out), spare account, no logout/purge/relink, no WhatsApp warning. Found B1 (one
  grey tick), B2 (dead connection never noticed), B3 (TUI never reconnects), Q2 (PN send split the
  chat), plus names, discoverability and the early-offline gap.
- **T15 fixes and Part 2 (work Mac, 2026-10-20; `feat/m2-tui`, 27 commits total on the branch,
  tip `f6af712`).** `8dcad27` B1+B2, `02b9926` B3, `6e21a68` one chat per contact (ADR 0001),
  `d05ac5c` names (ADR 0002) + WhatsApp-like keys and mouse, `7c81be9` early-offline queue,
  `534963b` thumbnails on the wire, `b931017` inline pictures in the TUI, `f6af712` demo stickers
  and transparency. (`odd/tasks/nuncio-t15-fixes.md`)
- **M3, desktop app (work Mac, 2026-10-20/21; `feat/m3-desktop`, 7 commits, tip `018d66a`).**
  `82b8f00` config/links/notification rules, `f636647` shared view models (TUI byte-identical),
  `5c65ef1` the GTK window, `128454e` notifications/background/links, `ec4f3c8`+`7634c91` palette
  and settings, `018d66a` pictures. (`odd/tasks/nuncio-m3.md`)
- **M4 preparation (work Mac, 2026-10-21; `feat/m4-omarchy`).** `33199b9` packaging, `8619fb5`
  and `0326312` wipe under the unit's sandbox, `95ce335` hotkey and bar widget, `afeb7a2` font,
  rounding, app id rename, `bf858ff` docs, `a028a65` `nuncio link` no longer replies.
  (`odd/tasks/nuncio-m4.md`)
- **Polish (work Mac, 2026-10-21/22; `odd/tasks/nuncio-polish.md`).** G1 group senders
  (`1de3eec`), G2 icon (`0454d99`), G3 live unread count over D-Bus (`faac81e`), G4 advisories
  (`01b60f2`, `b0ba2b2`), log cleanup (`0f07ea7`, `9fceeab`, `eda6f81`, `e6b7d70`), H1 oversized
  replies (`f2648bb`), H2 state publishing (`e3a7652`), H3 docs (`f6cc923`), H4 test sockets
  (`7b8ed7d`), J1 panic isolation (`455d283`), J2 frame fitting (`93a36dc`), J3 adapter errors
  (`3c8e7d2`), K1 shellcheck (`367fd1f`), Q4 drain pacing (`dcdf1d5`), Q6 stop reason
  (`434d69b`), N5 picture cap (`b6739a8`), F3 not confirmed on read timeout (`7081e4a`), M4-U
  uninstall of widget/hotkey (`b72066c`), AS animated stickers (`6990983`).
- **T17, sealed Signal state (work Mac, 2026-10-21; `odd/tasks/nuncio-t17.md`).** `bc89029` ADR
  0003, `9c2a3c1` data key and seal, `8e3c60e` sealed store, `70a5338` migration, `093b149`
  wiring, `e870aac` checklist.
- **CI (2026-10-21, re-done 2026-10-22).** `46eb6c3`, `4a61c17` (first version), then `766f0d2`
  (public-project shape) and `11a8cab` (short socket paths).
- **2026-10-22, the owner's decisions built.** `d0ea3c5` D2 sticker frames at 1x the box;
  `b386678` D3 seal `companion_meta_nonce` (with an upgrade path for DBs sealed before it);
  `766f0d2` D5 CI + Dependabot + toolchain pin + cargo-deny; `11a8cab` `shortTempDir`; `cf6821c`
  D1 25 s send wait off the UI loop (`crates/nuncio-client/src/outbox.rs`, `LateReply`);
  `28d2db1` test fix for a late reply; `a0599df` D4 `stop_reason` `keyring_locked` /
  `device_unreadable` and client texts; `7dab5cc` and `b9b1df1` bus tests: the private
  `dbus-daemon` in its own process group under a watchdog, a zombie counted as stopped.

`feat/m1-core` → `feat/m2-tui` → `feat/m3-desktop` → `feat/m4-omarchy` are stacked; each is
pushed; `feat/m4-omarchy` contains all of them. The PR is only `feat/m4-omarchy` → `main`.
`wip/n6-ratatui-image-12` (`5520769`, ratatui-image 12.0.0-rc.3) exists **only on the work Mac**,
never pushed; this machine will not have it.

### 2.2 The workflow (ODD)

Organic Driven Development, as the gentle-ai orchestrator defines it:

- Every stage has a feature document `odd/tasks/<name>.md`: header, `## Specs` (numbered `S#`,
  the owner's words quoted verbatim), `## Tasks` (ID, specs, route, status with evidence, commit),
  `## Log` (`L1` is the owner's request verbatim; later decisions appended verbatim with dates).
- **Test first with RED observed**, then GREEN, then refactor. Each test asserts observable effects
  through the public interface. Mutation checks: the writer kills mutants; the parent re-runs one.
- **One clean-context Opus writer agent per task**, with an exact allowed edit surface. Parallel
  writers only with disjoint surfaces. The parent orchestrates, reviews the diff against the
  surface, re-runs one check, commits, updates the document.
- **An independent read-only verifier** for every high-risk task (stored data, security,
  contracts, concurrency, delivery, untestable), probing in a fresh `mktemp -d` copy, never in the
  workspace; verdict per `S#`; blockers vs advisories.
- **One correction batch per task**, then one recheck limited to the blockers. The owner's
  standing rule (polish L15): a task still failing after two correction rounds is set aside on a
  local `wip/<task>` branch; CI got up to three (L20, L22).
- **Commit with the hook:** `GGA_TIMEOUT=900 git commit -m '...'`. The local pre-commit hook (GGA,
  `.gga`: provider claude, rules `AGENTS.md`, `*.go,*.rs`, strict, 300 s default timeout) runs an
  AI code review of the staged files against `AGENTS.md`. Its findings are advisories to judge; a
  bare FAILED with no findings is re-run once (m1 L18). Never `--no-verify`. The hook is not
  tracked: install it at home with `gga install` in the repo (ask the owner if `gga` is missing).
- Conventional Commits. **No AI attribution and no `Co-Authored-By` trailer, ever** (this overrides
  any harness reminder that suggests one).
- Delivery: `single-pr` (m1 L7), work-unit commits.
- Receipt-driven review (gentle-ai RDD) consent is asked per candidate in this repo (m1 L4); the
  pre-granted consent the owner gave applies to another project only.

### 2.3 Verification discipline (what "done" meant)

Per task: RED observed, mutants killed, verifier verdict (when high risk), the parent re-ran the
full suite before committing and pushing. The suites at the time of the last task: Go all
packages ok under `-race`; cargo workspace all tests (448+) passing; `packaging/check.sh` all
passed; CI green.

---

## 3. Decisions and why

Each entry: decision; alternatives; reason; reference (`file` L#/S#). Owner quotes are in their
Spanish, verbatim.

### 3.1 Architecture and ADRs

- **Split: Go daemon, Rust clients.** Go module = only the daemon (whatsmeow is Go); TUI and app
  in Rust sharing `nuncio-client`. Alternative: Bubble Tea TUI in Go. Owner: "la TERMINAL va en
  Rust + ratatui (no en Bubble Tea), compartiendo una librería Rust de cliente con la app de
  escritorio GTK. El módulo Go es solo el núcleo." (m0 L2)
- **One wire contract, shared fixtures, contract tests both ways.** So neither side can drift.
  (m0 S8, L3, L8; AGENTS.md)
- **Pure Go daemon, no cgo.** Static, simple packaging; rules out SQLCipher (ADR 0003).
- **ADR 0001, one chat per contact.** The canonical chat is the `@lid` JID when whatsmeow's LID map
  knows the pair; the PN JID is an alias; `store.MergeChat` moves a PN chat into the LID chat in
  one transaction at start and whenever a pair is learnt; nothing guessed from names; no mapping →
  no change. Wire: optional `aliases` on `Chat`. Alternatives in the TUI correction: B commit as
  is, C daemon only; owner chose **"A"** (explicit aliases on the wire), then "dale" to a fourth
  narrow correction. Reason: T15 showed replies landing in a second chat and unresolved quotes.
  (t15 S4, L4–L7)
- **ADR 0002, chat names and the masked-number fallback.** The daemon resolves names (group
  subject; contact full/first/business/push name) into `chats.name`; fallback is a masked number
  in an optional `phone` field (`…0001`); one `GetJoinedGroups` per start (ban-risk bound); group
  message `sender` by the same priority, capped at 64 code points. Rejected: resolve at serve
  time; masked number in `name`; full number to clients. (t15 P2b; polish G1, L2)
- **ADR 0003, where the Signal state lives (T17).** Option 1 (owner, polish S4): a 32-byte data
  key in the Secret Service (`store.data_key`, service `nuncio`, base64), AES-256-GCM from the Go
  stdlib, format `0x01|nonce|ct|tag`, AAD = `"nuncio/seal/v1"` + table + column + own JID + primary
  key; a wrapper over whatsmeow's `*sqlstore.SQLStore` (not a SQL driver wrapper: length CHECKs,
  in-SQL PN→LID copies and in-store pre-key generation break it); sealed columns per t17 S6;
  messages stay plain. Rejected: encrypted file into tmpfs (option 2; the owner rejected it), SQL
  driver wrapper, SQLCipher (cgo), one keyring item per secret. Migration: backup by
  `VACUUM INTO` (`nuncio.db.pre-t17-<UTC>.bak`, 0600, verified), `BEGIN IMMEDIATE`, key re-read
  before commit, then `secure_delete` + checkpoint + `VACUUM` + checkpoint, a durable
  cleanup-pending mark. Only `nuncio link` (or the migration of a not-yet-sealed DB) creates the
  key, create-if-absent with read-back. (t17 S1–S6, L2–L10; ADR 0003)
  - **Contradiction to know:** ADR 0003 point 11 still says the locked keyring shows "no new wire
    field ... no `stop_reason`". That was the U5 state; D4 (`a0599df`, t17 L12, polish L35) added
    `stop_reason` `keyring_locked` / `device_unreadable`. `proto/wire.md` and `SECURITY.md` are
    current; the ADR paragraph is stale (a docs follow-up, section 8).

### 3.2 Ban-risk rules (AGENTS.md, SECURITY.md)

- **No automation surface:** one message, one recipient per CLI invocation, no stdin, no batch.
  The apps send only on Enter; links only fill the composer (m3 S4).
- **No reconnect storms:** whatsmeow's `EnableAutoReconnect` off; nuncio's own backoff: base 2 s,
  ceiling 5 min, stable after 1 min, equal jitter in [s/2, s). Full jitter rejected (near-zero
  delays). Stop for good on logout, temporary ban, stream replaced, client outdated, refused
  connects (400/409/415/418/501). (m1 T10a, T10, verifier B2)
- **No presence:** never "available"; structural (no `SendPresence` method on `wa.Client`). B1
  uses `SetForceActiveDeliveryReceipts(true)` for two grey ticks; verified in whatsmeow that it only
  picks the receipt type, no presence. (m1 S10; t15 S1, L2)
- **Read receipts only on an explicit view**, as WhatsApp does it: opening a chat marks it read
  in nuncio and on WhatsApp, respecting the user's privacy setting (`TryFetchPrivacySettings`
  first, because `GetPrivacySettings` returns empty on a failed fetch). Owner: "B porque es un
  cliente de whatsapp si leo el mensaje en un dispositivo o lugar se lee en todos tal cual
  funcioan whatsapp ahora". At most 50 ids per receipt (assumption, Q9 "keep"). (m2 S8, L11, S9)
- **No silent retry loops.** Offline queue: queue offline, one drain on reconnect, in order, no
  timed retries; failures stay `not_sent`/`not_confirmed` with a manual retry under the same id.
  Owner: "hazlo, si necesitas algo de mi para pruebas o asi me avisas". (m1 S11, L23)
- **Early-offline queue:** a send whose acknowledgement is lost to a dropped connection becomes
  `queued` (same id, sent once on reconnect). Owner: "si hazlo asi porque debe funcionar como
  esperas que whatsapp funcione". (t15 S9, P2c)
- **Drain pacing 1 s (Q4).** `drainPace = time.Second` between drained sends; ended at once by a
  stop; live sends never paced. Before accepting, the owner asked how WhatsApp itself paces
  messages; what was found: normal sends are never paced; the 1 s applies only to the offline
  queue drained on reconnect, and it is a guess, not a documented WhatsApp limit. Owner: "seguimos
  tu recomendacion la opcion A": keep 1 s, and a live send during a pause goes at once after the
  older queued ones. (polish S4, L25, **L36**)
- **`nuncio link` never sends.** The M0 check that auto-replied to a message survived to M4; owner
  "si" to removing it (`a028a65`). (m4 L5, L6; earlier restriction m1 L12)
- **Consent gates:** `link` needs `--consent`; `logout`/`purge` need `--confirm`; nothing happens
  before the gate. (AGENTS.md; m0 L10; m1 S12)

### 3.3 Session, wire and client behaviour

- **B2 dead connection:** on keepalive failure go `offline` and reconnect through nuncio's
  backoff. A send can end not confirmed for up to 3 minutes (whatsmeow `KeepAliveMaxFailTime`),
  now in help and READMEs (H3). (t15 S2, L2)
- **B3 TUI reconnect:** capped backoff 0.5 s → 5 s, draft/selection kept, resubscribe. (t15 S3)
- **Key scheme (TUI):** Tab switches panes, Enter opens a chat straight into the composer, Esc
  back, mouse click and wheel. Owner: "...con el enter no puede abrirse y directamente pasar a
  escribir porque pues es el comportamiento esperado y como ya funciona whatsapp y tambien que sea
  compatible con mouse...". (t15 S8)
- **Inline pictures:** research first ("hay que investigar a fondo muy a fondo"), then
  ratatui-image (MIT) with thumbnail-first from the stored `JPEGThumbnail`, full picture only on a
  key, same reserved box. Stickers moved into the same slices ("genial entonces como dices subelo
  a la parte 2"). (t15 S10, L14, L15)
- **Shared view models before the GTK UI.** Owner: "si sigamos el camino del plan porque es lo que
  se verifico y es lo mejor". (m3 S5)
- **Pictures in the app:** "si me parecen bien tus recomendaciones": row label "Photo"/"Sticker" +
  caption; Enter shows the full picture; a picture with no size resizes once. (m3 L10)
- **App id rename** to `io.github.pwn27sbx.nuncio.Desktop`: the old id equalled the daemon's bus
  name, so they would fight for it on Linux. Orchestrator decision, unchallenged. (m4 L2)
- **Q6 stop reason:** optional `stop_reason` (`logged_out`, `temporary_ban`, `stream_replaced`,
  `client_outdated`, `connect_rejected`, `other`) on `event_connection` and `status`; no ban expiry
  (no numbers), generic `other`, logout shows `logged_out`, a second stop keeps the first reason.
  Owner: "los confirmo". (polish L25, **L37**)
- **D4 locked keyring:** `stop_reason` `keyring_locked` ("keyring locked: unlock it to connect")
  and `device_unreadable` ("the stored key does not open this device: see SECURITY.md"); old
  clients read unknown values as `other`. Owner answer "A". (t17 L10, **L12**; polish **L35**)
- **F3 → D1 send wait 25 s off the UI loop.** Problem: Rust clients waited 5 s while nunciod's
  `sendTimeout` is 20 s and the Go CLI waits 30 s, so 5–20 s acks showed not confirmed; a longer
  wait would have frozen the TUI's synchronous loop. Owner answer **"B"**: the clients wait 25 s
  (`SEND_WAIT`, `crates/nuncio-client/src/driver.rs`) in a separate outbox thread with its own
  connection (`outbox.rs`); the message shows sending until the reply; late replies (`LateReply`)
  only mark their own send; the daemon's `not_confirmed` now reaches both UIs. (polish L26, **L32**,
  D1, L46, L48)
- **J1/J2/H1/H2:** a panic closes only its connection; oversized replies keep the newest whole
  items and cut the newest alone on a code point; state publishing is single-flight and a stop is
  never overwritten. (polish S3, S4)
- **Logs carry no ids, JIDs, paths, SQLite text or client names**, only fixed text + error type.
  Owner "si" (L5) and "hazlo" (L8). (polish L5–L8)

### 3.4 Pictures and stickers

- **N5 full-picture cap 96 MiB** (`FULL_BOUNDS.alloc`, `crates/nuncio-client/src/pictures.rs`); the
  header pre-check charges 12 bytes/pixel for any WebP (animated canvases peak at ~3x), 4 for
  others. Consequence: a still WebP over ~8.4 Mpx, JPEG/PNG over ~25 Mpx and 16-bit PNG over ~12.6
  Mpx are refused. Alternative: read the VP8X animation bit. Owner: "A" (keep). (polish L17, L26,
  **L38**)
- **Animated stickers (AS):** desktop only (a TUI frame would be a re-encode sent every 100 ms);
  only the fetched full picture animates; canvas ≤ 1024 px (`ANIMATION_SIDE`); delay < 10 ms →
  100 ms; loops forever; GTK tick callback, off when `gtk-enable-animations` is off;
  `MAX_FRAMES = 1000`, `FRAME_MIN_BYTES = 4 KiB` charged per frame (verifier B1: 1x1 × 1.45M frames
  → 524 MB RSS), `ANIMATION_BYTES = 16 MiB` (`crates/nuncio-desktop/src/pictures.rs`). (polish L29,
  L31)
- **D2 frames at 1x the box:** a 512 px sticker kept 320x320 frames (400 KiB each) so > 40 frames
  stayed still; owner answer **"B"**: frames at the box size (160x160, 100 KiB), so 163 fit in
  16 MiB. Stills, thumbnails and the first-frame fallback keep 2x. (**L33**, D2)
- **N6 ratatui-image 12:** first parked (no stable 12.x; L27); then the owner chose 12.0.0-rc.3
  for its tmux sixel fixes ("si uso tmux entonces usemos la version mas reciente de la version 12",
  **L39**); then re-decided for a public release: "si volvemos a la A mejor": stay on **11.1**
  until 12.0.0 stable, keep `wip/n6-ratatui-image-12` (Mac only) for then. **L43 replaces L39.**
  Notes for that day in L27 (stdin workaround #202 can be simplified; kitty shm probing on Ghostty
  and foot+herdr to check).

### 3.5 T17 decisions (all confirmed)

- Event and retry buffer payloads sealed too; the migration never deletes its plain backup and
  says it holds keys, the wipe (logout/purge) deletes it; keyring locked/absent at start → daemon
  stays up, never connects or writes, retries on the backoff; a non-keyring migration failure
  waits for a restart (avoid one plain backup per retry); logout through the daemon with a locked
  keyring is refused and suggests purge; L6 wrapper choices (`MigratePNToLID` clears its done mark
  on failure, `DeleteDevice` deletes privacy tokens, `PutDevice` requires ID and Account,
  `nuncio_device(jid, sealed, companion_meta_nonce)`). Owner: "lo confirmo". (t17 L3, L6, L10,
  **L13**; polish L41)
- **`companion_meta_nonce` sealed** (whatsmeow sends it to WhatsApp as the `Companion_User_Secret`
  header, so it is a secret). Owner "A": seal it before the first real migration, which then
  covers it. Built in D3 with an upgrade path for DBs sealed before it. (t17 L7, **L11**; polish
  **L34**, D3)
- Migration on the real DB only at home, backup first, never the real DB or keyring on the Mac:
  "Sí, con esas reglas". (polish L19 = t17 S3)

### 3.6 CI, toolchain and delivery

- **CI for a public project (D5).** First plan (L20, L30): CI last, push to every branch, plain
  Ubuntu packaging container, stable Rust unpinned. Re-evaluated because the repository is public;
  owner: "si con todo eso" (**L42**). Result in `.github/workflows/ci.yml`: triggers push to `main`
  and `feat/m4-omarchy` (the branch only until the workflow is on `main`), every `pull_request`,
  `workflow_dispatch`; `contents: read`; actions pinned by SHA; no secrets. Jobs:
  1. **Go** (gofmt, vet, `CGO_ENABLED=0` build, `go test -race -count=1 -timeout 900s ./...` with
     `dbus-daemon`, govulncheck v1.9.0 on stable Go);
  2. **Rust** (pinned 1.99.1: fmt, clippy `-D warnings`, `cargo test --workspace --no-fail-fast`;
     the desktop smoke test skips without a display);
  3. **Rust MSRV** (`cargo check --locked --workspace --all-targets` with `rust-version` 1.92);
  4. **cargo-deny** 0.20.2, SHA-256 pinned (`deny.toml`: GPL-3.0-compatible licences, advisories,
     unmaintained/yanked denied, crates.io only);
  5. **Arch Linux package** in `archlinux:base` pinned by digest: `package-source.sh`, `makepkg`
     with `check()` as a non-root user, `namcap` (an `E:` fails), `pacman -U`, `pacman -Qlq`
     equals `MANIFEST`, `systemd-analyze --user verify`, `check.sh`;
  6. **Packaging** on Ubuntu with ShellCheck (on Arch it pulls in GHC).
  Dependabot updates the actions weekly; Cargo/Go updates are by hand, flagged by cargo-deny and
  govulncheck; the Arch image digest is bumped by hand.
- **Rust pinned in `rust-toolchain.toml` to 1.99.1** (rustfmt, clippy, minimal profile) so a new
  clippy lint cannot turn CI red with no code change (the L30 advisory); **MSRV 1.92** in
  `Cargo.toml` `[workspace.package] rust-version`, checked in CI. Plain cargo without rustup (Arch's
  `rust` package, used by `makepkg`) ignores the pin. Go minimum: `go 1.26.0` in `go.mod`.
- **One PR, merge commit.** Owner: "no necesitamos PRs apilados o encadenados es nuestro proyecto
  propio nadie mas que nosotros lo revisa..." (m1 L7). The PR body explains the size exception:
  the whole first release line as reviewed work units; re-slicing finished history adds no review
  value; review commit by commit; **merge with a merge commit to keep the work units**. Draft until
  T1–T10 pass.
- **Releases:** none. Release steps exist (`packaging/README.md`, "How a release would be made")
  and need the owner's explicit OK.

### 3.7 Standing approvals (polish L13–L22, given for the office night of 2026-10-21)

- **L13 (still valid):** push to `origin/feat/m4-omarchy` without asking, after each task verified
  with the suite green; never another branch, never `main`, no merge, no PR creation.
- L15 a task failing after two correction rounds goes to a local `wip/<task>` branch; L16 an
  uncovered design decision takes the most conservative option and is logged "to confirm"; L17
  dependencies only when needed, GPL-3.0-compatible, in CREDITS, with a verifier; L19 T17 migration
  rules; L20/L22 CI up to three correction rounds. **Ask the owner once whether L15–L17 still apply
  at home** before relying on them. L14 (`caffeinate`) and L21 (Homebrew) were Mac-only.

---

## 4. Incidents and lessons

| Incident | What happened | Rule since |
|---|---|---|
| `nuncio link` auto-reply | The M0 check (wait 10 min, reply to a direct message) survived into M4 (m4 L5). | Removed (`a028a65`); a link never sends. |
| App id collision | The GTK app id equalled the daemon's bus name (m4 L2). | Renamed before anything ran on Linux; `check.sh` checks it. |
| Deleted `/tmp/nuncio-*` | A writer deleted temp folders including the owner's `/tmp/nuncio-demo` (m3 L6). | **Never delete files you did not create.** |
| `rm -rf /tmp/TestProbe*` | The J2 verifier removed 10 other verifiers' leftovers (polish L24). | Handoffs name the exact paths an agent may delete. |
| Glob delete in a shared scratchpad | A D5 writer's `rm -f $SCRATCH/run*.sh` deleted another writer's helper (L44). | **Unique per-writer scratch names; never glob-delete.** |
| rustup silent install | `rustup show active-toolchain` before exporting PATH installed toolchain 1.99.1 (~470 MB) on the Mac (L44). | Expect rustup to install the pinned toolchain on first use; say so before it happens. |
| 112 orphaned `nunciod` | The D-Bus activation tests left their activated daemon running (PPID 1), one pair per run; a `-timeout` panic also skipped cleanups (L49). | Fixed in `7dab5cc` (process group, watchdog, regression test `TestPrivateBusLeavesNoActivatedDaemonBehind`). **After test runs on Linux: `pgrep -a nunciod; pgrep -a dbus-daemon`.** |
| Zombies in Arch CI | Killed bus processes became zombies (container PID 1 does not reap); `kill(pid, 0)` succeeds on a zombie (L50). | `b9b1df1`: state `Z` counts as gone (`TestAZombieCountsAsGoneAndARunningProcessDoesNot`). |
| Socket path too long | `makepkg`'s deep `TMPDIR` pushed test sockets past `sun_path` (Linux 107, macOS 103 usable bytes; `dbus-daemon` 99) (D5). | `11a8cab`: tests use `shortTempDir`. |
| apt 403 in CI | The runner's packages.microsoft.com source answered 403 before any cargo command. | Rerun the job; not a code problem. |
| ext4 inode reuse | The stale-socket test passed falsely on Linux CI (`4a61c17`). | Fixed; Linux behaves differently from macOS: run the suites here. |
| FSEvents panic | A theme read panicking inside notify's callback aborted the process on macOS (m4 L3). | The read runs on its own thread. |
| herdr "image swap" | Blamed on herdr; the demo served one WebP for every sticker (t15 L19). | Reproduce before blaming a tool. |
| Writer ran `install.sh --uninstall` with the real HOME | On the Mac; the guard refused before reading anything (L28). | Probes run with a temp HOME only. |
| A commit after a red run | `b931017` committed after one failing (flaky) test. | Rerun the full suite before committing. |

No commit carries AI attribution; no real-account action, real database or real keyring was
touched from the Mac.

---

## 5. Hard rules (they bind this session)

1. **Talk to the owner (Sebastián) in neutral Spanish, never voseo** («puedes», «mira», «dime»;
   never «podés», «mirá», «decime»), also after a bare command or a task notice. Code, commits,
   docs and this kind of handoff are in **English**.
2. **No push without the owner's explicit OK naming the remote and the branch.** The only standing
   OK: `origin/feat/m4-omarchy` after a verified task with the suite green (L13). **Never push to
   `main`. Never merge** (the merge is the owner's, "Create a merge commit", after T1–T10). Never
   create a release, a tag, an issue, a PR or anything else on GitHub without their OK. `gh pr ready`
   and branch protection are GitHub actions: owner's OK first.
3. **Real WhatsApp account actions only as the checklist says, with the owner present and OK.**
   Only the **spare** account (the one linked in M0 and used in T15). **Never link, send from or
   touch the owner's main number** (the one with their two-factor authentication); nothing in this
   checklist needs it. No `logout`, `purge`, relink or soak without their explicit OK. Keep the real
   message budget small, about 10, as in T15. One linked client; never link a second device.
4. **Back up `~/.local/share/nuncio` before the first start of the new `nunciod`** (it is the first
   real T17 migration; section 6.4). **Never copy that directory anywhere else** (no other machine,
   no repo, no report, no chat, no issue). Logs and reports never contain message text, full
   numbers (mask to the last 4 digits at most), contact names, key material, or the keyring
   contents (never print a `secret-tool` secret).
5. **No `sudo` without the owner's OK.** T5 (install) needs it: ask first. Installing packages
   with pacman needs it too.
6. **Never delete files you did not create; never kill processes you did not start;** close every
   server and browser tab you open; never glob-delete. Before stopping the owner's running
   `nunciod`, ask.
7. **Never commit `.atl/` or `.DS_Store`.** No AI attribution, no `Co-Authored-By`.
8. Tests never connect to WhatsApp and never touch the real keyring. `NUNCIO_KEYRING_TESTS=1`
   (`internal/store/device_test.go`) runs one test against the owner's real Secret Service: the
   rules forbid it on the Mac; here, only with the owner's OK, and it is not part of the checklist.
9. Every failure found at home goes into a feature document before it is fixed (section 6.16).

---

## 6. Home procedure

Run each command from the repository root unless it says otherwise. `$REPO` below means that root.

### 6.1 Get the code

```sh
cd "$REPO"                                   # the home clone (see section 1)
git status --short                           # expect only untracked .atl/ (never commit it)
git fetch origin
git switch feat/m4-omarchy 2>/dev/null || git switch -c feat/m4-omarchy origin/feat/m4-omarchy
git pull --ff-only
git log --oneline -1                         # this handoff's commit, on top of b9b1df1
gh pr view 1 --json state,isDraft,headRefOid # OPEN, draft, same head
gh run list --branch feat/m4-omarchy --limit 3   # latest CI green (rerun a job on an apt 403)
```

If `git switch` reports local changes, stop and ask the owner; do not stash or discard them.

### 6.2 Toolchain and tools (Arch)

What the repo needs (README "From source", `packaging/README.md`, CI):

| Need | Version | Check | Install (needs sudo: **ask the owner**) |
|---|---|---|---|
| Go | ≥ 1.26 (go.mod); home had 1.27.2 at T15 | `go version` | `sudo pacman -S --needed go` |
| Rust via rustup | pinned 1.99.1 (`rust-toolchain.toml`) | `rustup show active-toolchain` | `rustup toolchain install` (no sudo; downloads ~0.5 GB: tell the owner first) |
| Rust MSRV (optional) | 1.92 | | `rustup toolchain install 1.92 --profile minimal` |
| Build tools | | `pacman -Q base-devel git pkgconf` | `sudo pacman -S --needed base-devel git pkgconf` |
| GTK | gtk4, libadwaita (needs GTK ≥ 4.12, libadwaita ≥ 1.4) | `pkg-config --modversion gtk4 libadwaita-1` | `sudo pacman -S --needed gtk4 libadwaita` |
| `dbus-daemon` (D-Bus tests) | | `command -v dbus-daemon` | Arch uses dbus-broker for the session; the `dbus-daemon` binary comes from the `dbus` package. Check `pacman -Qo /usr/bin/dbus-daemon`; install only with OK |
| Packaging checks | namcap, desktop-file-utils, python, systemd (`systemd-analyze`) | `command -v namcap desktop-file-validate python3 systemd-analyze` | `sudo pacman -S --needed namcap desktop-file-utils python` |
| ShellCheck | | `command -v shellcheck` | `sudo pacman -S shellcheck` pulls the GHC runtime; ask; CI already runs it |
| T8 probe | sqlite3, python3, libsecret (`secret-tool`) | `command -v sqlite3 secret-tool` | `sudo pacman -S --needed sqlite libsecret` |
| T5/T10 | jq, qmllint (Qt 6 declarative tools), Quickshell, hyprctl, busctl | `command -v jq qmllint qs hyprctl busctl` | Omarchy ships most; ask before installing |
| Terminals for T3 | Ghostty, foot, alacritty, herdr | | owner's choice |
| GGA hook | | `command -v gga && ls .git/hooks/pre-commit` | `gga install` (ask if `gga` is missing) |

Rust with rustup: `cargo` must be on `PATH` (`export PATH="$HOME/.cargo/bin:$PATH"` in
non-interactive shells if needed). Running any cargo or rustup command in the repo makes rustup
install 1.99.1 if it is missing (incident L44): say so to the owner before the first run.

### 6.3 The full suite on Linux (no real account; any time)

```sh
# Go
test -z "$(gofmt -l .)" && echo gofmt-ok
go vet ./...
CGO_ENABLED=0 go build ./...
TMPDIR=/tmp go test -race -count=1 -timeout 900s ./...
TMPDIR=/tmp go test -count=1 -v -run 'DBus|PrivateBus|Zombie' ./cmd/nunciod \
  | grep -E '^(=== RUN|--- (PASS|FAIL|SKIP))'
go run golang.org/x/vuln/cmd/govulncheck@v1.9.0 ./...      # network; optional, CI runs it

# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo test -p nuncio-desktop --test smoke -- --nocapture  # under Hyprland: must NOT print "smoke: skipped"
RUSTUP_TOOLCHAIN=1.92 cargo check --locked --workspace --all-targets   # optional MSRV

# Packaging
bash packaging/check.sh       # also runs shellcheck, desktop-file-validate, systemd-analyze when present
command -v shellcheck >/dev/null && shellcheck packaging/*.sh

# Leftovers: the tests must leave nothing behind
pgrep -a nunciod; pgrep -a dbus-daemon
```

Pass: every command exits 0; the `-v` D-Bus run shows `--- PASS` and **no `--- SKIP`** for
`TestDBusUnreadCountFollowsTheStoreWithOneSignalOfEachKind`,
`TestDBusIntrospectionListsUnreadCountAndUnreadChanged`
(`cmd/nunciod/dbus_unread_test.go`), `TestDBusActivationStartsDaemonInUnderOneSecond`,
`TestDBusSingleInstanceSecondDaemonExitsZero`, `TestCLIActivatesDaemonOverDBus`
(`cmd/nunciod/main_test.go`), `TestPrivateBusLeavesNoActivatedDaemonBehind`
(`cmd/nunciod/bus_leak_test.go`); `TestAZombieCountsAsGoneAndARunningProcessDoesNot`
(`cmd/nunciod/procstate_test.go`) passes. After the runs, `pgrep` shows no test `nunciod` (a test
one runs from a Go build cache path or `/tmp`, with a temp `HOME`) and no test `dbus-daemon` (its
`--config-file`/`--address` point into a temp dir). The owner's own `nunciod` (if running) is not
a leftover. A leftover test process is one this session started: kill only those, after checking
its command line, and record it as a failure of the fix.

Known flaky tests (rerun before blaming a change; a flake on Linux inotify is new and worth a
note): `theme::watch_tests::a_theme_switch_is_noticed_within_500ms`
(`crates/nuncio-client/src/theme.rs`), `font::tests::an_unchanged_font_and_unrelated_files_are_not_sent`
(`crates/nuncio-client/src/font.rs`), possibly `TestAWrappedReactionFailureIsLoggedByItsCause`
(`internal/wa/adapter_error_test.go`).

### 6.4 The backup (before the first start of the new `nunciod`)

The new daemon rewrites stored data at its first start: whatsmeow schema upgrade, the T17
migration (seal), the ADR 0001 merge of split chats, the ADR 0002 name sync. Owner present.

```sh
data="${XDG_DATA_HOME:-$HOME/.local/share}/nuncio"
ls -la "$data"                                  # nuncio.db, maybe -wal/-shm, media/
pgrep -a nunciod                                # ask the owner before stopping a running one
# (owner OK) stop it the way it was started (T15 ran debug builds by hand; no unit is installed yet)
stamp=$(date -u +%Y%m%dT%H%M%SZ)
backup="$HOME/.local/share/nuncio-backup-$stamp"
mkdir -m 700 "$backup"
cp -p "$data"/nuncio.db* "$backup"/             # nuncio.db, -wal, -shm if present
ls -la "$backup"                                # every file mode 600, dir 700
sha256sum "$backup"/nuncio.db* | sed "s|$HOME|~|"   # record only that hashes exist, not paths in reports
```

The backup stays on this disk, private (0700/0600), and goes nowhere else. It holds the device's
plain keys: tell the owner, and delete it only when they say so (it is their file then).

### 6.5 Build the binaries for the manual runs (T2–T4, T8 before T5 installs the package)

```sh
run="$HOME/.cache/nuncio-home-run"; mkdir -p -m 700 "$run"
CGO_ENABLED=0 go build -o "$run/" ./cmd/nunciod ./cmd/nuncio
cargo build --release -p nuncio-tui -p nuncio-desktop   # binaries in target/release/
```

Run the daemon in the foreground in its own terminal, logging to a file outside the repo:

```sh
"$run/nunciod" 2>&1 | tee -a "$run/nunciod.log"
```

Before quoting any log line in a report, read it: it must have no ids, numbers, names, paths or
key bytes (if one has, that is a finding).

### T1. Baseline (no account)

- Goal: everything builds and passes on Linux; the package builds.
- Commands: 6.3, then:
  ```sh
  packaging/package-source.sh                 # writes .local/package/{nuncio-<version>.tar.gz,PKGBUILD}
  cd .local/package
  makepkg --cleanbuild --force                # build + check(), no install (add --nodeps with rustup Rust)
  namcap PKGBUILD nuncio-*.pkg.tar.zst        # an " E: " line is a failure
  tar -xOf nuncio-*.pkg.tar.zst usr/bin/nunciod | strings | grep -c '/home/'   # must print 0
  for b in nuncio nuncio-tui nuncio-desktop; do tar -xOf nuncio-*.pkg.tar.zst usr/bin/$b | strings | grep -c '/home/'; done
  cd "$REPO"
  ```
- Pass: all green, D-Bus tests PASS (not SKIP), `makepkg` incl. `check()` ok, `namcap` no `E:`,
  zero `/home/` strings.
- Record: tool versions (`go version`, `rustc --version`, `pkg-config --modversion gtk4
  libadwaita-1`), counts of passed tests, any flake.

### T6. The real-bus D-Bus tests (no account)

Covered by 6.3's `-v` line. These are their first runs outside CI with `-v`. Pass and failure
handling as in 6.3. Do T6 together with T1.

### 6.6 Ordering note: T2's first start IS the first real migration

The office checklist put T8 (first real T17 migration) after T2, but `nunciod` at HEAD migrates a
plain database **as soon as it opens the stored device**, which is T2 step 1. So run the **T8
first-start block** (T8 steps 1–3 below) at T2 step 1, then continue T2, and do the rest of T8 in
its place. This is a correction of the office handoff, not a new decision.

### T2. T15 re-check with the spare account (owner present and OK)

Preconditions: 6.4 backup done; owner present with both phones (the spare account's phone and the
test contact's phone); login keyring unlocked; old daemon stopped (with OK).

1. **First start (with the T8 first-start block).** Start `"$run/nunciod"` (6.5); watch its log.
   Expect at start: one line about the migration and its backup (no ids, paths, names or key
   bytes). Then check, with `"$run/nuncio" status` / `chats` and the TUI
   (`target/release/nuncio-tui`):
   - **one** chat for the test contact (ADR 0001), with all 10 T15 messages, the 4 `outbox` rows
     `sent`, the reaction and the mute row (T15 final store: 2 chats, 10 messages, 4 outbox rows,
     1 reaction, 1 mute row);
   - the old `> (a message not loaded)` quote now resolves;
   - `chats` lists the PN JID in that chat's `aliases` (do not paste the JID into reports);
   - chat names, not LID digits: the contact's name, else a masked `…NNNN`, else a placeholder;
     group subjects after the first connection.
   Then do **T8 steps 1–3** (below) before any send.
2. **B1, two grey ticks without "online":** with the spare phone's WhatsApp closed, send to nuncio
   from the other phone: two grey ticks; nuncio never shows "online"; no blue tick unless the chat
   is opened in nuncio with read receipts on.
3. **B2, a silent drop:** a network change, or NAT idle > 3 min, until the socket dies without
   FIN/RST. Within about 3 min `nuncio status` says `offline`, then `linked`; phone messages sent
   meanwhile arrive.
4. **B3, restart `nunciod` with the TUI open:** header `reconnecting…` → `reconnected to nunciod`;
   draft, open chat, selection kept; a send during the gap shows `not sent`, `R` retries it.
5. **Q2, a send to `<pn>@s.whatsapp.net`:** still one (`@lid`) chat; the reply lands there; a quote
   of the sent message resolves.
6. **P2c, airplane mode mid-send:** the TUI shows `queued`; on reconnect it is sent once.
   **Q3:** on the recipient's phone it appears **once** (whatsmeow's `retryFrame` can resend up to
   3 times under one id on a quick reconnect).
7. **`mark_read`:** opening a chat clears unread; with read receipts on the sender sees blue
   ticks; with them off, `read-self` and no blue.
8. **Quoted media:** a reply quoting a photo and a document renders right on the phone and in the
   TUI.
9. **Group senders (G1):** in a group, each incoming message shows its author's name (or `…NNNN`),
   never LID digits.
10. **Send wait (D1):** a send shows "sending" until its reply; the screen never freezes.
11. **`nuncio link`** only if the owner OKs a relink (normally skip): it reports and exits, sends
    nothing.

Budget: about 10 real messages for T2 + T8 + T9 together. Record per step: pass/fail, what was
seen, no text, no numbers, no names.

### T3. Terminal matrix with pictures (owner present for real chats; or the demo daemon)

Ghostty (kitty), foot (sixel), alacritty (half blocks), herdr (kitty re-rendered; also foot
inside herdr, which may announce kitty and show nothing → try `NUNCIO_IMAGE_PROTOCOL`). In each:
thumbnail at once, `o` the full picture in the same box, no layout jump, nothing downloaded
without a key, the footer names the protocol; stickers static and animated show the first frame
and `▶`; transparency blends with the theme in half blocks; a no-thumbnail sticker labelled once;
80x24 and 240x60; emoji/ZWJ and RTL text do not corrupt the frame (ZWJ/VS16/skin tones are
dropped when drawing only: known trade-off); 200 messages scroll interactively.
Without the real account: `mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo
go run ./cmd/nuncio-demo`, then `XDG_RUNTIME_DIR=/tmp/nuncio-demo target/release/nuncio-tui`
(the demo refuses to start while a real `nunciod` answers; stop the demo at the end; remove
`/tmp/nuncio-demo` only if you created it).

### T4. The GTK app on Hyprland (M3 acceptance)

Plan wording (m3 S1): "Orca reads the chat list and transcript; a live theme switch leaves no
stale colours; a notification click focuses the correct conversation from a cold start; cold start
< 1.5 s; no title bar under Hyprland; stable memory over a day."

1. Orca reads the chat list, the transcript and picture bubbles ("Photo"/"Sticker" + caption).
2. No title bar (`[app] title_bar = auto`); corners follow
   `hyprctl -j getoption decoration:rounding`.
3. Live theme: switch the Omarchy theme (and font) with the app open: no stale colours; the font
   follows `fc-match monospace`.
4. Notification click from a cold start: Ctrl+Q, receive a message, click the notification: the
   app starts on that chat. Also with the window hidden in background.
5. Cold start < 1.5 s (launch to a populated list).
6. Memory: `ps -o rss= -p $(pgrep -f nuncio-desktop)` at start and after ~24 h with pictures viewed
   (overlaps T19 soak: owner's OK).
7. Links: `xdg-open 'whatsapp://send?phone=<fake digits>&text=hola'` opens the chat (or a new empty
   one) with the draft, **never sends** (GFile URI normalisation of D-Bus `Open` unverified). Use a
   fake number, never a real one, or the test contact with the owner's OK.
8. Palette (F1 / Ctrl+K) and settings (Ctrl+,) writing `~/.config/nuncio/config.toml` mode 0600
   (dir 0700). Back up the owner's config first if one exists (`cp -p`).
9. **Locked keyring notice (D4):** with the keyring locked (T8 step 5), the header/subtitle shows
   `keyring locked: unlock it to connect`.
10. Send wait (D1): a send shows sending until the reply; the window stays responsive.

### T5. M4 install (owner's OK first: `sudo`, packages, enabled units)

Follow `packaging/README.md` "What to test at home" and `contrib/omarchy/README.md` "Verify at
home". Before: stop the manual `nunciod` from 6.5.

1. Install: `cd .local/package && makepkg --cleanbuild --force -si` (or `sha256sum
   nuncio-*.pkg.tar.zst > SHA256SUMS && bash "$REPO/packaging/install.sh" --dir .`). Then
   `diff <(grep -vE '^[[:space:]]*(#|$)' "$REPO/packaging/MANIFEST" | sort) <(pacman -Qlq nuncio | grep -v '/$' | sort)`
   prints nothing.
2. `systemctl --user enable --now nunciod.service`; `systemctl --user status nunciod` active;
   `busctl --user status io.github.pwn27sbx.nuncio`; `systemd-analyze --user security nunciod`
   (record the score; SECURITY.md says it is still to run on Linux). Skip the doc's relink step
   unless the owner wants it: the keyring is exercised by T8.
3. `systemctl --user kill -s SIGKILL nunciod`: it comes back after 30 s, not sooner
   (`journalctl --user -u nunciod`). If it refuses to start (StartLimitBurst=4 per 30 min counts
   manual and D-Bus starts): `systemctl --user reset-failed nunciod`.
4. D-Bus activation: `systemctl --user stop nunciod`, then `nuncio chats` starts it.
5. With the app running: `busctl --user list | grep pwn27sbx` shows `io.github.pwn27sbx.nuncio` and
   `io.github.pwn27sbx.nuncio.Desktop`; `gapplication launch io.github.pwn27sbx.nuncio.Desktop`
   works; the launcher shows "nuncio" and "nuncio (terminal)"; the icon shows and the symbolic one
   recolours.
6. Autostart: log out and in: the daemon and the app start, the app with no window (Omarchy's uwsm
   runs `xdg-desktop-autostart.target`: to verify).
7. Hotkey (changes the owner's Hyprland config: OK, and `cp -p ~/.config/hypr/bindings.lua
   ~/.config/hypr/bindings.lua.before-nuncio` first):
   ```sh
   cp contrib/omarchy/hypr/nuncio.lua ~/.config/hypr/nuncio.lua
   echo 'require("hypr.nuncio")' >> ~/.config/hypr/bindings.lua
   hyprctl reload
   omarchy menu keybindings --print | grep -i nuncio
   hyprctl clients -j | jq -r '.[].class'      # lists io.github.pwn27sbx.nuncio.Desktop with the app open
   ```
   It replaces Omarchy's WhatsApp web-app key. Pass: starts the app; focuses it on another
   workspace without a second window; brings it back from background.
8. Bar widget:
   ```sh
   dest=~/.config/omarchy/plugins/pwn27sbx.nuncio-unread
   mkdir -p "$dest"
   cp contrib/omarchy/bar-widget/manifest.json contrib/omarchy/bar-widget/BarWidget.qml "$dest"/
   omarchy plugin validate "$dest"
   qmllint -I "$OMARCHY_PATH/shell" "$dest/BarWidget.qml"
   omarchy-shell shell rescanPlugins
   omarchy plugin enable pwn27sbx.nuncio-unread --section right
   busctl --user --auto-start=no get-property io.github.pwn27sbx.nuncio /io/github/pwn27sbx/nuncio io.github.pwn27sbx.nuncio UnreadCount   # i <n>
   busctl --user introspect io.github.pwn27sbx.nuncio /io/github/pwn27sbx/nuncio   # UnreadCount, UnreadChanged
   busctl --user monitor --match "type='signal',path='/io/github/pwn27sbx/nuncio'" # runs without error (Ctrl+C)
   qs log -p "$OMARCHY_PATH/shell" --tail 100   # widget errors
   ```
   Pass: the badge shows the count (99+ above 99); a new message raises it and reading the chat
   lowers it **within a second, live**; at 0 it hides; stopping nunciod hides it at once
   (`NameOwnerChanged` through dbus-broker), starting it shows it again; `get-property` fails with
   the daemon stopped and **does not start it**; click = launch-or-focus.
9. Uninstall (first real run on Arch; owner's OK; reinstall afterwards if more tests need it):
   `omarchy plugin disable pwn27sbx.nuncio-unread`, then `bash packaging/install.sh --uninstall`
   (it disables the unit before `pacman -R`). Pass: no `MANIFEST` path remains; no unit enabled;
   the widget dir is removed only if it holds exactly the shipped files; `nuncio.lua` only if
   unchanged; only the exact `require("hypr.nuncio")` line leaves `bindings.lua`, after a backup
   `bindings.lua.nuncio-backup`; the mode of `bindings.lua` is kept; a hand-edited copy stays with
   a notice; a symlinked path is left with a notice; `hyprctl reload` has no error. Uninstall does
   **not** delete `~/.local/share/nuncio`.

### T7. What the Mac could not check (cross-reference; tick each while doing the others)

| Item | Where it is checked |
|---|---|
| dbus-broker delivers `NameOwnerChanged` to `busctl monitor` | T5.8 |
| `BarWidget.qml` lint with Quickshell | T5.8, T10.1 |
| real-bus D-Bus tests | T6 |
| `desktop-file-validate`, `systemd-analyze` inside `check.sh` | 6.3 (with both installed) |
| `namcap`, `shellcheck`, `makepkg check()` | T1 |
| `systemd-analyze --user security nunciod` score | T5.2 |
| wipe under `ProtectSystem=strict` (bind-mounted data dir) | only with a real logout/purge (T16, owner's OK) |
| symbolic icon recolouring | T5.5 |
| GFile URI normalisation of D-Bus `Open` | T4.7 |
| Omarchy bar types for third-party plugins; WhatsApp glyph in the bar font | T5.8 |
| a second app start presents the running window | T5.7 |
| foot inside herdr | T3 |
| `nuncio-desktop` builds and runs on Arch | T1, T4 |
| `cmd/nunciod/main.go` start log line with a real device | T2.1 log: no ids, paths, names |
| T17 on a real DB and Secret Service | T8 |
| go-keyring's Unlock prompt (no timeout on Linux) | T8.6 |
| animated sticker playback (mapped window) | T9 |
| uninstall of widget/hotkey on Arch | T5.9 |
| desktop smoke test with a display | 6.3, T10.2 |
| ratatui-image 12 kitty shm probing | later, on 12.0.0 (N6) |

### T8. T17 sealed store: the first real migration (owner's OK; spare account)

Never run on the Mac against a real database or keyring (t17 S3).

**First-start block (do it at T2 step 1, 6.6):**

1. Backup done (6.4); the migration also writes `nuncio.db.pre-t17-<UTC>.bak` in the data dir.
2. Login keyring unlocked. Start nunciod and watch the log: one line about the migration and the
   backup, and no ids, paths, names or key bytes. A line warning that a free-page cleanup is still
   pending is possible (a restart retries it). Check the keyring item **without printing secrets**:
   ```sh
   secret-tool search --all service nuncio 2>/dev/null | grep -E '^(label|attribute\.)'
   ```
   It must list an item for `store.data_key` (legacy `device.*` items from M0 may also show).
   Never paste a `secret = ` line anywhere (the filter drops it).
3. **Pass check (S2): no key byte, and not the companion meta nonce, in any DB file.** Stop nunciod
   first. Write the probe to a fresh scratch dir outside the repo (it prints counts only, never a
   value):
   ```sh
   probe=$(mktemp -d)
   cat > "$probe/t17_probe.py" <<'EOF'
   import base64, os, sqlite3, sys
   bak, targets = sys.argv[1], sys.argv[2:]
   con = sqlite3.connect(f"file:{bak}?mode=ro", uri=True)
   secrets = {}
   def add(kind, value):
       if value is None:
           return
       if isinstance(value, str):
           value = value.encode()
       value = bytes(value)
       if len(value) >= 12 and any(value):
           secrets.setdefault(kind, []).append(value)
   cols = ("noise_key", "identity_key", "signed_pre_key", "adv_key", "companion_meta_nonce")
   for row in con.execute(f"SELECT {', '.join(cols)} FROM whatsmeow_device"):
       for kind, value in zip(cols, row):
           add(kind, value)
   for (value,) in con.execute("SELECT key FROM whatsmeow_pre_keys LIMIT 50"):
       add("pre_key", value)
   for table, col in (("whatsmeow_sessions", "session"), ("whatsmeow_sender_keys", "sender_key"),
                      ("whatsmeow_app_state_sync_keys", "key_data")):
       try:
           for (value,) in con.execute(f"SELECT {col} FROM {table} LIMIT 20"):
               add(table, value)
       except sqlite3.Error:
           pass
   con.close()
   def forms(v):
       b64 = base64.b64encode(v)
       return {"raw": v, "hex": v.hex().encode(), "HEX": v.hex().upper().encode(),
               "b64": b64, "b64-nopad": b64.rstrip(b"="),
               "b64url": base64.urlsafe_b64encode(v).rstrip(b"=")}
   print("secrets loaded:", {k: len(v) for k, v in secrets.items()})
   total = 0
   for path in targets:
       name = os.path.basename(path)
       if not os.path.exists(path):
           print(name, "absent")
           continue
       data = open(path, "rb").read()
       hits = {}
       for kind, values in secrets.items():
           for v in values:
               for form, f in forms(v).items():
                   if f in data:
                       hits[f"{kind}/{form}"] = hits.get(f"{kind}/{form}", 0) + 1
       total += sum(hits.values())
       print(name, "matches:", sum(hits.values()), hits)
   print("TOTAL", total)
   sys.exit(1 if total else 0)
   EOF
   data="${XDG_DATA_HOME:-$HOME/.local/share}/nuncio"
   bak=$(ls "$data"/nuncio.db.pre-t17-*.bak | head -1)
   python3 -I "$probe/t17_probe.py" "$bak" "$data/nuncio.db" "$data/nuncio.db-wal" "$data/nuncio.db-shm"
   rm -f "$probe/t17_probe.py" && rmdir "$probe"     # only what this step created
   ```
   Pass: `secrets loaded` shows non-zero counts for the device keys, `companion_meta_nonce` and
   `pre_key` (if `companion_meta_nonce` is absent, it was empty in the plain DB: note it), and
   `TOTAL 0`, exit 0. Any match is a **blocker**: stop, keep everything, tell the owner, open the
   feature document. Optionally also confirm with sqlite3 that the stub row is zeroed:
   `sqlite3 "file:$data/nuncio.db?mode=ro" "select length(noise_key), hex(substr(noise_key,1,4)), length(companion_meta_nonce) from whatsmeow_device"`
   (expect zeros / empty; prints at most 4 bytes, all zero).

**Later steps (in T8's place, after T2–T5):**

4. Send and receive one message with the **spare** account (T2 rules): Signal sessions work after
   the migration.
5. **Keyring stopped:** with the owner's OK (it affects their other apps), lock the login keyring
   (e.g. Seahorse, or for gnome-keyring
   `busctl --user call org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service Lock ao 1 /org/freedesktop/secrets/collection/login`;
   verify the collection path with the owner). Record `sha256sum` and `stat -c '%n %Y %s'` of
   `nuncio.db*`, restart nunciod: clients show `offline` with `keyring locked: unlock it to connect`
   (T4.9; `nuncio status` shows offline), the log gives the reason with no key bytes, and the hashes
   and mtimes are unchanged after a minute. Logout through the daemon is refused and suggests purge
   (only observe the refusal; no purge). Unlock: the daemon reconnects by itself on the backoff.
6. go-keyring's Unlock prompt has no timeout on Linux (t17 L10): with the prompt open,
   `nuncio status` still answers (offline).
7. `nuncio link --qr --consent` while nunciod runs must refuse **before touching the database**
   (compare hashes; never let it reach a QR without the owner's explicit wish to relink).
8. Once the owner is happy: delete the `nuncio.db.pre-t17-*.bak` file(s) only on their word (they
   hold the keys in plain), or leave them for `logout`/`purge`. The 6.4 backup is theirs too.

### T9. Animated stickers in the GTK app (Hyprland)

1. A chat with an animated sticker: fetch its full picture (click / Enter / `o`): it plays and
   loops; the thumbnail before the fetch stays still.
2. Animations off: `gtk-enable-animations=0` under `[Settings]` in
   `~/.config/gtk-4.0/settings.ini` (back up the owner's file first; restore it after), restart the
   app: the sticker stays on its first frame; back on, it plays the next time its bubble is drawn.
3. A 512 px sticker with up to **163 frames now plays** (D2); one over the 16 MiB cap stays still
   on its first frame with no error and no memory jump (`ps -o rss=`).
4. The TUI still shows the first frame and `▶`.

### T10. Lint and the checks CI skips

1. `qmllint -I "$OMARCHY_PATH/shell" contrib/omarchy/bar-widget/BarWidget.qml` with Quickshell
   present. Without it (Mac, L23): 13 import + 2 unresolved-type warnings, 9 unqualified (e.g. :95
   `root.refresh()`, :125 `parent` in nested components; suggests
   `pragma ComponentBehavior: Bound`), 2 missing-property (:111 `monitorProc.running`), 1
   inheritance-cycle, 1 type-instantiated-recursively. Fix only what remains with Quickshell
   present, and only after the widget ran in T5.8 (earlier QML warnings were also seen at home).
2. `cargo test -p nuncio-desktop --test smoke -- --nocapture` under Hyprland: runs, does not print
   `smoke: skipped, GTK cannot start here`.
3. `systemd-analyze --user verify /usr/lib/systemd/user/nunciod.service` and `bash
   packaging/check.sh` with the package installed (CI's Arch job runs this in a container; here it
   is the real session).

### Optional, owner's OK only

- **T16 real logout:** `nuncio logout --confirm`: WhatsApp unlinks first; then the keyring items,
  `nuncio.db*`, the `pre-t17` backups, media and exports go; the daemon stops and stays stopped;
  the emptied data dir may stay under the sandbox. Irreversible for the link: the owner decides.
- **T19 24 h soak** (and T4.6 memory).

### 6.16 When something fails

1. Stop that step; keep state; tell the owner in Spanish what failed (no text, numbers, names).
2. Create or extend a feature document `odd/tasks/nuncio-home.md` (header; `## Specs` with the
   failing checklist item quoted; `## Tasks`; `## Log` with `L1` = the owner's request verbatim and
   the observed failure). Reproduce first.
3. RED test that fails for the observed reason; fix; GREEN; mutants; for a high-risk change an
   independent read-only verifier in a `mktemp -d` copy; one correction batch.
4. In a long session, delegate the writing to a clean Opus writer per task with an exact allowed
   edit surface (owner's global rule); you orchestrate and commit.
5. Full suite (6.3) green, then `GGA_TIMEOUT=900 git commit` (Conventional Commit, no
   attribution), then `git push origin feat/m4-omarchy` (standing OK L13), then watch CI
   (`gh run watch` or `gh run list --branch feat/m4-omarchy`).
6. Re-run the failed checklist step.

---

## 7. After T1–T10 pass

1. Tell the owner the results (Spanish), with the list of anything skipped and why.
2. With their OK: `gh pr ready 1`, and update the PR body if the home run changed anything (their OK
   for the edit too). The PR body says: draft until T1–T10; merge with a merge commit.
3. **The owner merges** in the GitHub UI with **"Create a merge commit"** (not squash, not rebase:
   it keeps the work units). Never merge yourself.
4. Only when the owner says so, two follow-ups:
   - **Remove the branch push trigger.** On a new branch from the merged `main` (e.g.
     `ci/main-only`): in `.github/workflows/ci.yml` change `branches: [main, feat/m4-omarchy]` to
     `branches: [main]` and delete the three comment lines above it that explain the temporary
     branch. Commit `ci: run on pushes to main only`. Push that branch and open a PR only with their
     OK naming it; they merge.
   - **Branch protection for `main`**, requiring the six checks. Check names are the job names:
     - `Go (gofmt, vet, pure-Go build, race tests, govulncheck)`
     - `Rust (fmt, clippy, tests)`
     - `Rust MSRV (cargo check with the workspace rust-version)`
     - `cargo-deny (licences, advisories, bans, sources)`
     - `Arch Linux package (makepkg, namcap, systemd-analyze, check.sh)`
     - `Packaging (packaging/check.sh with ShellCheck)`

     In the UI: Settings → Branches → Add branch protection rule → `main` → "Require status checks
     to pass before merging", "Require branches to be up to date", add the six names. Or with
     `gh` (owner's OK; they are the only reviewer, so no required approvals):
     ```sh
     gh api -X PUT repos/pwn27sbx/nuncio/branches/main/protection --input - <<'EOF'
     {
       "required_status_checks": {
         "strict": true,
         "contexts": [
           "Go (gofmt, vet, pure-Go build, race tests, govulncheck)",
           "Rust (fmt, clippy, tests)",
           "Rust MSRV (cargo check with the workspace rust-version)",
           "cargo-deny (licences, advisories, bans, sources)",
           "Arch Linux package (makepkg, namcap, systemd-analyze, check.sh)",
           "Packaging (packaging/check.sh with ShellCheck)"
         ]
       },
       "enforce_admins": false,
       "required_pull_request_reviews": null,
       "restrictions": null,
       "allow_force_pushes": false,
       "allow_deletions": false
     }
     EOF
     gh api repos/pwn27sbx/nuncio/branches/main/protection --jq '.required_status_checks.contexts'
     ```
     If the PR that removes the branch trigger is still open, its checks must show the same names
     first (they do: the job names do not change).
5. A release (tag, `.pkg.tar.zst`, `SHA256SUMS`) only with the owner's explicit OK
   (`packaging/README.md`, "How a release would be made").

---

## 8. Open follow-ups and advisories

**Priority 1 (small, worth doing after the merge, owner's call):**

- The TUI's first connection loss of a session reconnects with **no backoff** (pre-existing:
  `Daemon::connect` leaves Reconnect scheduled; reproduced at baseline). Also re-pin the F3 footer
  test in `crates/nuncio-tui/tests/reconnect.rs` once fixed. (polish L46)
- A **one-time `SQLITE_BUSY`** when two processes first open a database sealed before D3 (the
  nonce column added outside the write lock; 30/60 in a probe; nothing partial; the daemon retries,
  link refuses while the daemon answers). Only test DBs were sealed before D3, so the real home DB
  is not affected. (polish L45)
- ADR 0003 point 11 is stale about `stop_reason` (section 3.1).
- The desktop README sentence on a send waiting during a lost connection should say only the one in
  flight is answered, queued ones are not sent. (polish L46)

**Priority 2 (when 12.0.0 ships):** ratatui-image 11.1 → 12.0.0 (L43, L27 notes).

**Priority 3 (accepted or unrealistic today; keep in mind):**

- D-Bus/bar: a failed recount keeps the last value until the next change; the notifier is not
  awaited at shutdown (G3, L3). QML warnings (T10.1; also seen at home earlier).
- Drain: `nuncio status` CLI does not print `stop_reason`; a status read during `stop()` can
  briefly show linked with a reason (L25). After the device opens, a harmless duplicate offline
  frame; a pre-D4 client shows "WhatsApp stopped the session" for a locked keyring (L47).
- T17: a lost migration race can leave a second plain backup (the wipe deletes it); a narrow window
  between keyring probe and the migration's key write; `store.Open` `SQLITE_BUSY_RECOVERY` when
  processes open the file together (baseline); waiter past 30 s busy timeout fails "database is
  locked" (writes nothing) (t17 L9, L10).
- Frames: no paging cursor; a cut text has no wire marker; an oversized non-message event is not
  sent (H1 L10, J2 L24).
- J1 (L18): out.mu without defer in subscribe; two log lines for an event-write panic; Rust client
  notices the close only on its next request; a panic mid-wipe leaves a half wipe; no server write
  deadlines.
- Pictures: 16-bit PNG over the cap says "Memory limit exceeded"; desktop README omits the WebP
  rule (L26); N4 a corrupt thumbnail shows its label twice; I1 thumbnail budget not monotonic;
  reopening a chat re-decodes evicted pictures; queued decodes are not cancelled (m3 L11).
- AS: re-enabling animations plays a sticker only when its bubble is next drawn; rows scrolled out
  of view are not paused; demo stickers do not animate (L31).
- M4-U: `mv` does not keep hardlinks/ACLs/xattrs of `bindings.lua`; a failed rewrite leaves its
  backup (L28). M4: StartLimitBurst counts manual and D-Bus starts (documented).
- D3b/D3c (m3 L8, L9): a link without text or a notification click retargets a draft written for
  another chat; two links at cold start keep only the last draft; `app.open-chat` accepts any
  string (never sends); Quit with a pending request waits for the 5 s read timeout; a
  sub-millisecond re-read/rename race can lose a hand edit of the config; palette ranking has no
  accent folding.
- Earlier: A2 a theoretical second reconnect (t15 L2); legacy cache files adopted without
  re-checking SHA-256; a `GetJoinedGroups` failure not retried until restart (ban-risk design);
  `locate_send` HashMap nondeterminism with a duplicated client_id (m3 L3); notification body
  markup unchecked (polish L2).
- Open questions kept as they are: Q1 media decryption delegated to whatsmeow (accept); Q3 dedupe
  of a same-id resend (checked in T2.6); Q7 no second device; Q9 50-id cap; Q10 refuse react/reply
  on my queued send; Q11 partial receipt failure; N1–N3 assumptions (`notified` set 1024, archived
  unmuted chats notify, link text cap 65 536).

---

## 9. Environment notes for Linux

- **`TMPDIR=/tmp`** for Go tests is the documented runner (macOS's temp path is too long for a Unix
  socket; harmless on Linux). Socket path limits: Linux 107 usable bytes, macOS 103, `dbus-daemon`
  99; tests use `shortTempDir` (`11a8cab`), so even makepkg's deep `TMPDIR` works.
- **D-Bus tests** start a private `dbus-daemon` and skip when it is not on `PATH`; on the Mac they
  never ran. They must PASS here (6.3).
- **Process checks:** `kill(pid, 0)` succeeds on a zombie, so the tests read process state
  (`/proc/<pid>/stat`). After any test run: `pgrep -a nunciod; pgrep -a dbus-daemon`.
- **Keyring tests:** the only real-keyring test needs `NUNCIO_KEYRING_TESTS=1` and skips otherwise
  ("set NUNCIO_KEYRING_TESTS=1 to exercise the owner's real Secret Service"); every other store test
  uses a fake keyring. Never set it without the owner's OK.
- **Desktop smoke test** skips without a display (also in CI); run it under Hyprland.
- **Flaky:** theme watcher 500 ms test, font "unchanged" test, possibly
  `TestAWrappedReactionFailureIsLoggedByItsCause`. A desktop worker test flaked after D1 and was
  fixed in `28d2db1` (L48).
- **Rust:** rustup reads `rust-toolchain.toml` and installs 1.99.1 on first use; Arch's `rust`
  package ignores it (makepkg uses it; with rustup Rust add `--nodeps`).
- **makepkg** builds offline after `prepare()` (`go mod download`, `cargo fetch --locked`); the
  tarball is `git archive HEAD`, so packaging changes must be committed before `package-source.sh`.
- **Demo daemon:** `mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo go run
  ./cmd/nuncio-demo`; it refuses to start while a real `nunciod` answers.
- **Old leftovers** from earlier runs may exist (`/tmp/ntui-*`, `/tmp/ntal-*`); remove only those
  this session created.
- **GGA** is local: `gga install`; commits with `GGA_TIMEOUT=900 git commit`.
- **Custom `XDG_DATA_HOME`** needs a drop-in for the unit (a unit cannot read the variable).
- **`.atl/`** is an untracked cache; never commit it. gentle-ai native assess needs
  `--untracked-scope=exclude` because of it (m1 L6).

---

## 10. Where to find more

| File | Holds |
|---|---|
| `odd/tasks/nuncio-m0.md` | M0 spikes on this machine; owner L1–L2 (Rust TUI, Go core); consent gate; the S7 timing. |
| `odd/tasks/nuncio-m1.md` | M1 core: T1–T14, specs S1–S16 (ban-risk acceptance), L1–L24 (single-pr L7, GGA setup L8/L11, outbox L23, verifier advisories). |
| `odd/tasks/nuncio-m2.md` | M2 TUI: U1–U15, read receipts decision S8, demo daemon, L1–L19. |
| `odd/tasks/nuncio-t15-fixes.md` | T15 fixes B1–B3, Q2 (ADR 0001, option A), keys, early-offline queue, image research, I1–I3, L1–L20. |
| `odd/tasks/nuncio-m3.md` | Desktop app D1–D3d, L1–L11 (picture choices L10). |
| `odd/tasks/nuncio-m4.md` | Packaging, hotkey/widget, font/rounding, docs, K1–K4, L1–L6 (app id L2, link reply L6). |
| `odd/tasks/nuncio-polish.md` | G1–G4, H1–H4, J1–J4, K1, Q4, Q6, N5, F3, N6, M4-U, T17, AS, CI, D1–D5; L1–L50 (standing approvals L13–L22; 2026-10-22 decisions L32–L43; incidents L44, L49, L50). |
| `odd/tasks/nuncio-t17.md` | The sealed store U1–U6, S1–S6 (what is sealed), L1–L14 (decisions L11–L13). |
| `docs/adr/0001-one-chat-per-contact.md` | PN/LID canonical chat and merge rules. |
| `docs/adr/0002-chat-names.md` | Names, masked phone, group `sender`. |
| `docs/adr/0003-where-the-signal-state-lives.md` | Data key, seal, wrapper, failure modes, migration, wipe (point 11 stale, section 3.1). |
| `SECURITY.md` | What is stored where, locked keyring texts, logs, ban-risk rules, installer, unit sandbox, wipe, reporting. |
| `proto/wire.md` | The wire contract, `stop_reason` table, corners. |
| `packaging/README.md` | Build, install, uninstall, release steps, decisions, "What to test at home". |
| `contrib/omarchy/README.md` | Hotkey and widget install, "Verify at home", assumptions. |
| `INTEGRATIONS.md`, `docs/config.md` | Omarchy/Hyprland/D-Bus/systemd details; the config file. |
| `AGENTS.md` | Review rules the GGA hook applies. |
| `README.md` | Overview, install, first run, CLI, development. |
| `.github/workflows/ci.yml`, `deny.toml`, `rust-toolchain.toml` | CI, dependency policy, toolchain pin. |
