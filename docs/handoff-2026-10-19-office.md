# nuncio: handoff from the work Mac to the home machine (2026-10-19)

For the Claude Code session on the owner's home Linux machine (Arch/Omarchy), starting with **no
context** from today. Read this top to bottom before touching anything. Sources it was written from:
the office report `~/Documents/nuncio-office-report-2026-10-19.md` (work Mac only; it covers up to
`b8b48aa`), `odd/tasks/nuncio-m1.md`, `odd/tasks/nuncio-m2.md` (the full evidence, Specs and Logs)
and the original office handoff (kept outside the repo because it names the test number).

---

## 1. What this is and how to start

nuncio is a native WhatsApp client for Omarchy: a Go daemon `nunciod` owns the one linked-device
session (whatsmeow); a ratatui TUI and a GTK 4 app are thin clients over a Unix-socket NDJSON
protocol (`proto/wire.md`). M0 passed on 2026-10-18 at home. Today the work Mac did the Mac-safe
part of M1 and almost all of M2 (TUI), against fakes only. **Nothing touched the real account.**

| Ref | Sha | Meaning |
|---|---|---|
| `origin/main` | `ae35fea` | untouched since M0 |
| `origin/feat/m1-core` | `87e4d9e` | M1 office work (T1–T14), branched from `ae35fea` |
| `origin/feat/m2-tui` | `7643b57` (tip) | **everything**: all of `feat/m1-core` plus M2 (U1–U15) |

- **No PR is open.** Delivery is one single PR, no stacking (owner decision, section 5). When to
  open it is an open question (section 6).
- Work continues on `feat/m2-tui`. `feat/m1-core` is only a historical marker.

First commands at home (repo lives at `~/Projects/nuncio` there):

```sh
cd ~/Projects/nuncio
git status --short                 # expect only: ?? .atl/   (never commit it)
git fetch
git switch feat/m2-tui             # tracks origin/feat/m2-tui
git log --oneline -1               # 7643b57 docs(odd): record the follow-up round (or later)

gofmt -l .                         # must print nothing
go vet ./...
go test -count=1 ./...             # D-Bus tests should RUN here (they skipped on the Mac)
go test -race -count=1 ./...
CGO_ENABLED=0 go build ./...
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings    # includes nuncio-desktop on Linux
cargo test --workspace                                    # includes nuncio-desktop on Linux
```

Expected from the Mac's last run (on `9b91c36`, excluding `nuncio-desktop`): all Go packages ok,
`cargo test` 110 passed / 0 failed. `nuncio-desktop` has **never been built since M0**; a failure
there is new information, not a regression from today. Record your own baseline first.

Try the TUI without WhatsApp, with the fake daemon `cmd/nuncio-demo` (invented chats with fake `555`
numbers, no network, no DB, no keyring; it refuses to start while a real `nunciod` answers):

```sh
# terminal 1
mkdir -m 700 -p /tmp/nuncio-demo && XDG_RUNTIME_DIR=/tmp/nuncio-demo go run ./cmd/nuncio-demo
# terminal 2
XDG_RUNTIME_DIR=/tmp/nuncio-demo cargo run -p nuncio-tui
```

What the demo answers and every key binding: `crates/nuncio-tui/README.md`. `Ctrl-C` stops the demo
and removes its socket. Do **not** run it while the real `nunciod` is up on the default runtime dir.

---

## 2. Hard rules still in force

From the original handoff and the owner. Not negotiable.

| Rule | Detail |
|---|---|
| No push without explicit OK | Every push needs the owner's OK **naming the remote and the branch**. Same for opening the PR or creating anything on GitHub. |
| No account actions without explicit OK | Never `nuncio link`, `logout`, `purge`, unlink from the phone, or **any real send** without an explicit OK for **that specific action** (a general "go on" is not enough). |
| Never the owner's main number | Not in code, tests, fixtures, logs, docs, or messages. Only the spare test number, which is linked at home; it is never written in the repo either. |
| No secrets in commits | No DB files, keyring dumps, tokens, session keys, or full phone numbers in commits, fixtures or logs. Never copy `~/.local/share/nuncio/` anywhere. |
| Logs | Never contain message text, full numbers or key material (numbers are masked to the last 4 digits). |
| No AI attribution | **No `Co-Authored-By`, no "Generated with", no AI trailer, ever**, in commits, PR bodies or anywhere. Overrides any tooling default. Check: `git log --format='%H%n%B---' ae35fea..HEAD \| grep -i 'co-authored\|claude\|generated with'` must print nothing. |
| `.atl/` | Skill-registry cache. Never commit it. |
| Language | Reply to the owner in **Spanish, neutral, no voseo** ("puedes", "elige", never "podés", "elegí"). Code, commits, docs, ODD files stay **English**. |
| ODD | Feature documents live under `odd/tasks/<feature>.md` (Specs quoting the owner verbatim, Tasks, Log). Tests first with RED observed; mutants killed; help text and docs updated with every command change. |
| GGA | Pre-commit review by GGA with the rules in `AGENTS.md`. The hook is local (`gga install`), not tracked: install it at home. No `--no-verify`. |
| Independent verifier | High-risk changes (stored data, security/keyring, contracts others consume, concurrency, delivery, untested paths) get an independent read-only verifier after their own checks. |
| Ban risk | Real runs: a handful of messages, never loops, no automation or bulk send, one linked client at a time, stop reconnecting after a ban/logout. |
| No `sudo` | If something needs it, stop and ask. |
| Do not reopen M0 decisions | GPL-3.0-only, Go+whatsmeow daemon, Rust workspace, socket+thin D-Bus, no SQLCipher, no calls, no automation surface. Disagreements become open questions. |

---

## 3. What was done today

Every code change: RED observed first, mutation-checked, GGA-reviewed. Full evidence per task is in
the two ODD files.

### M1, branch `feat/m1-core` (`ae35fea..87e4d9e`, 25 commits)

| Task | Commit(s) | What |
|---|---|---|
| T0 | `8b424bc` | ODD tracking file and day-start baseline |
| T1 | `ae22593` | `nuncio.db` and its WAL/SHM created `0600` in a `0700` dir; an existing `0644` file is tightened |
| T2 | `39066e6` | Go and Rust IPC clients time out on a wedged daemon |
| T3 | `baa5067` | CLI tests: status, usage errors, the `link` consent gate creates nothing |
| T4 | `c01a81c` | Wire v1: `messages`, `send`, `subscribe`, `focus` and event frames; 15 fixtures; corners table |
| T5 | `78336ac` | SQLite store for chats and messages with an FTS5 index; idempotent inserts |
| T6 | `6fea0c1`, `5fc9a89`, `081147c` | `nunciod` owns the whatsmeow session; logout → unlinked, ban/stream replaced/connect failure → offline; `nuncio link`'s test auto-reply only to direct chats |
| (GGA) | `f919e34`, `a568419` | `AGENTS.md` review rules + `.gga`; GGA fails only on new or changed lines |
| T7 | `5aae543` | Incoming text stored, unread tracked, `chats`/`messages` served from the store; no automatic receipts |
| T8 | `b7aca45` | Event stream to subscribers; a stalled subscriber is dropped (64-frame queue). D-Bus `UnreadChanged` NOT done |
| T9 | `4d78834` | `nuncio chats`, `send <jid> "<text>"`, `focus`; honest sent/queued/not sent/not confirmed |
| T10 | `4b87268`, `7f89644` | Capped jittered backoff (2 s base, 5 min cap); stops for good on logout/ban/stream replaced/outdated/connect failure |
| T11 | `87e4d9e` | Offline outbox: queue offline, one in-order drain on reconnect, same WhatsApp id, failures visible, `nuncio retry <client_id>` |
| T12 | `5df5676`, `bbd901f` | `logout --confirm` and `purge --confirm` leave no DB, media, exports, keyring items or socket |
| T13 | `920c0cf`, `6c99bc2` | `download_media`: on-demand download via whatsmeow, SHA-256 checked, `0600`/`0700` cache |
| T14 | `0f4f883`, `c381607`, `982316f` | App-state sync for pin/archive/mute; `CREDITS.md`; bump drill re-run (`docs/bump-drill.md`) |
| (docs) | `25690d1` | `proto/wire.md` corners table and chat JID definition |
| T15–T19 | none | Need the home machine and/or the real account (section 7) |

### M2, branch `feat/m2-tui` (`87e4d9e..7643b57`, 18 commits)

| Task | Commit(s) | What |
|---|---|---|
| U1 | `1ff7ec1` | TUI chat list, conversation view, composer, keyboard only, shared `ThemeWatcher` palette |
| U2 | `37f406b`, `bc65acf` | Wire + daemon: `search`, `send.reply_to`, `react`; reactions of an unknown sender refused; no JIDs in reaction errors |
| U3 | `d866f0d` | TUI: reply, react, retry, search, open media externally. **Inline images left out** (L6) |
| U4 | `a092d53`, `d7d3c3a` | ODD records and pushes |
| U5 | `5b5ccb4` | `cmd/nuncio-demo`, a fake daemon to try the TUI by hand |
| U6 | `d5e5dda` | Manual-test fixes: local times, whole preview times, ZWJ/FE0F/skin tones stripped at render, no react/reply on unsent messages |
| U7 | `5a53d30` | Daemon `mark_read`: receipts on WhatsApp respecting the read-receipt privacy setting (`TryFetchPrivacySettings`) |
| U8 | `ce876dd` | TUI and demo send `mark_read` on open and on a new message in the open chat |
| U9 | `4fe21e2` | At most 50 ids per read receipt; failed reconnect attempts logged |
| U10 | `b8b48aa` | `event_chat` for app-state pin/archive/mute; `not_confirmed` error code; T2 test temp dir cleaned |
| U11 | `0d826a3` | nuncio-demo data race fixed; a `download_media` racing a wipe never recreates the data dir |
| U12 | none | Office report updated with M2 (outside the repo) |
| U13 | `f91183f` | Early reactions kept; quoted media replies record `reply_to`; one message's reactions only; emoji/punctuation-only search falls back to a substring match |
| U14 | `9b91c36` | Reused `client_id` answers the stored send; quoting media carries kind/caption/mime; my LID quoted in `@lid` chats; `mark_read` serialised within its 4 s bound; clearer cache errors; 0700 re-applied on cache hit |
| U15 | `d0bea42` | Multi-line composer (`Alt-Enter`/`Ctrl-J`, grapheme cursor, grows to 5 rows); unknown daemon ops parse as `DaemonFrame::Unknown`; jid tie-break in chat sort |
| (docs) | `6fc40cd`, `7643b57` | ODD records of the follow-up round |

Independent verifiers ran on T8–T13, U2, U7, U10, U11, U13, U14. Every blocker got one correction
batch and a recheck limited to it (all FIXED). Remaining advisories: section 8.

---

## 4. Wire protocol additions today

**Source of truth: `proto/wire.md` + `proto/fixtures/`** (both contract suites, Go and Rust, read
every fixture). All additive; `v` stays 1. Read the "M1 corners" and "M2 corners" tables there
before changing any op.

| Kind | Added in M1 | Added in M2 |
|---|---|---|
| client → daemon ops | `messages`, `send` (`jid`, `text`, `client_id`), `retry` (`client_id`), `subscribe`, `focus`, `wipe` (`unlink`), `download_media` (`jid`, `id`) | `search` (`jid`, `query`, `limit`) → `messages`; `react` (`jid`, `id`, `emoji`; empty removes); `mark_read` (`jid`); `send.reply_to` (optional) |
| daemon → client replies | `messages`, `sent` (`state` `sent`/`queued`), `send_failed`, `subscribed`, `focused` (`delivered`), `wiped` (`unlinked`), `media_ready` (`path`, `mime`) | `reacted`, `marked_read` (`unread`) |
| events | `event_message` (also on state changes of my sends), `event_connection`, `event_unread`, `event_focus` | `event_chat` (whole Chat, after app-state pin/archive/mute) |
| Chat fields | `muted` | — |
| Message fields | `kind`, `media`, `state` (`queued`/`not_sent`), `client_id` | `reply_to`, `reactions` (`{emoji, from_me}`, mine first) |
| error codes | (`not_linked` existed) | `not_confirmed` (send/retry ack timed out; before M2 it was `internal` with the same message) |

The Rust client parses an op it does not know as `DaemonFrame::Unknown` (U15), so an older TUI no
longer drops its event connection on a newer daemon frame.

---

## 5. Owner decisions taken today, and assumptions

Decisions (quoted verbatim where the ODD files quote them):

| Decision | Quote / detail | Where |
|---|---|---|
| Single PR, no stacked or chained PRs | "no necesitamos PRs apilados o encadenados es nuestro proyecto propio nadie mas que nosotros lo revisa, a lo mucho las RDD y GGA de gentle-orchetrator con lo que te dije que trabajes que vienen incluidas" | M1 L7 |
| GGA as per-commit review | owner "ok"; `AGENTS.md` "Review verdict": FAILED only for new or changed lines | M1 L8, L11 |
| T6 fixes 1 and 2 | "arregla ahora el 1 (que la respuesta automática de prueba de nuncio link no conteste a grupos, estados ni canales; riesgo de baneo) y el 2 (LoggedOut → unlinked; ban, stream reemplazado o fallo de conexión → offline), cada uno con sus tests. El 3 (segunda sesión de link con un dispositivo ya vinculado) déjalo anotado para T9/T12." Item 3 done in T9 (`link` refuses while the daemon answers) | M1 L12 |
| T11 outbox semantics | "hazlo, si necesitas algo de mi para pruebas o asi me avisas": queue offline, one drain on reconnect, in order, no timed retries, failures `not_sent`, manual retry | M1 L23 |
| T10 stops | StreamReplaced, ClientOutdated and ConnectFailure stop reconnecting too; reason logged once, not on the wire | report §7 |
| T12 | logout goes through the daemon (never a second session); purge never touches the network | report §7 |
| T13 | decryption delegated to whatsmeow `Client.Download` (open question below) | M1 L14 |
| Time limits dropped | "olvidemos eso de las 4:30 y 5:00 en general las horas restrictivas olvidemoslas, que mas podemos hacer aqui?" | M2 L13 |
| Reactions never queued | a reaction that cannot be sent now is refused, never sent later | report §7 (U2) |
| `mark_read`: choice B | "B porque es un cliente de whatsapp si leo el mensaje en un dispositivo o lugar se lee en todos tal cual funcioan whatsapp ahora". Opening a chat marks it read in nuncio and on WhatsApp; receipts off → read-self only | M2 S8 |
| Demo daemon | "no podemos probar el TUI aqui?" → proposal → "dale" | M2 S7, L9 |
| Follow-up rounds | "dale" to S9 items 1–8 and to S10 items 9–10 | M2 S9, S10, L16 |

Assumptions made (not confirmed by the owner):

- **50-id cap per read receipt**, sent as consecutive receipts with no extra retries (ban risk). M2 S9 (3).
- **ZWJ normalisation**: ZWJ joiners, FE0F and skin-tone modifiers are stripped at render so every
  terminal draws the same width. Trade-off: a family emoji shows as its separate parts. Documented in
  `crates/nuncio-tui/README.md`.
- U2 wire shape for search/reply/react was the orchestrator's choice (M2 L3).
- T10 equal jitter in [s/2, s) instead of full jitter (near-zero delays rejected).

---

## 6. Open questions for the owner

Ask **one at a time** and wait. Each has a short answer.

| # | Question | Choices |
|---|---|---|
| Q1 | T13: decryption is delegated to whatsmeow, so the test fake returns plaintext, not the "known ciphertext" the spec named. | accept delegation / re-implement HKDF+HMAC+AES-CBC and the media fetch |
| Q2 | T9 A1: whatsmeow sends to `digits@s.whatsapp.net` as the contact's `@lid` and replies arrive as `@lid`, so one conversation can split into two chats. | store sends under the JID whatsmeow reports (`resp.Chat`) / add a phone-number ↔ LID mapping |
| Q3 | T11 A1: a message in flight when the daemon dies stays `queued` and is resent on reconnect with the same WhatsApp id. | keep (recipients dedupe by id; check in T15) / mark leftover `queued` rows `not_sent` at startup |
| Q4 | T11 A6: the drain sends queued messages back to back. | add pacing between them / keep as is |
| Q5 | T17: where the Signal state lives. | option 1: keyring-held data key + sealed DB columns / option 2: encrypted DB file decrypted into tmpfs |
| Q6 | Show the T10 stop reason (e.g. a temporary ban and its expiry) to clients over the wire? | yes / no |
| Q7 | Link the work Mac as a second device? (not asked, not done) | yes / no |
| Q8 | M2 inline images in Ghostty need the `jpeg` feature of the `image` crate plus a startup terminal query. | add them / keep "open externally" only |
| Q9 | After seeing them on the real account: keep the 50-id cap and the blue-tick / read-self behaviour? | keep / change |
| Q10 | Reply or react on my own still-queued send (WhatsApp has no such message yet; left out on purpose, M2 S10). | support it / keep refusing |
| Q11 | Partial read-receipt failure (M2 L11): a later sender's receipt failing leaves earlier receipts sent but the chat unread; the next view resends. | keep / redesign |
| Q12 | The not-confirmed mark lives only in TUI memory (M2 L15). Put a `not_confirmed` message state on the wire? | yes / no |
| Q13 | When to open the single PR `feat/m2-tui` → `main`. | now (as draft) / after T15 / after M2 is fully verified |
| Q14 | CI workflows (Go + Rust; high risk, needs his OK). | add now / later |

---

## 7. What needs the home machine or the real account

In order. Steps marked **OWNER OK** need his explicit OK for that action and him at the phone.

1. [ ] Baseline at home (section 1 commands), including the D-Bus tests and `nuncio-desktop`
   (`cargo clippy --workspace`, `cargo test --workspace`). Install GGA's hook (`gga install`).
2. [ ] `chmod 600 ~/.local/share/nuncio/nuncio.db*` (the home DB was `0644` after M0; T1's code also
   tightens it on the next open). Check the dir is `0700`.
3. [ ] **T15 real-account run (OWNER OK)**, daemon from `feat/m2-tui`, a handful of messages only:
   - [ ] receive a text; send one reply; download one media file and compare with the phone;
   - [ ] airplane mode: queue, then one drain on reconnect; network drop mid-send: no duplicate, no
     silent loss; does the recipient dedupe a resend with the same WhatsApp id (Q3);
   - [ ] `mark_read`: with read receipts **on**, blue ticks appear on the sender's phone when the chat
     is opened; with receipts **off**, only read-self (nothing on the sender's side); a chat with
     more than 50 unread sends several receipts (Q9);
   - [ ] a quoted reply to a photo/video/document renders on the phone even though the quote carries
     no media key, URL or thumbnail (M2 L18); a quoted voice note (no PTT flag);
   - [ ] react and remove a reaction, both directions; a reaction from the phone shows in the TUI;
   - [ ] in-chat search, including emoji;
   - [ ] pin / archive / mute on the phone → `event_chat` updates the TUI live;
   - [ ] the `@lid` split (Q2): send to a `@s.whatsapp.net` JID and see where the reply lands.
4. [ ] **M2 terminal matrix** on Linux: foot, alacritty, ghostty, each at 80x24 and 240x60:
   whole read/reply flow keyboard only; emoji/ZWJ (check the U6 normalisation looks right), CJK
   and RTL text do not corrupt the frame; 200-message scroll stays interactive; `Alt-Enter` and
   `Ctrl-J` both insert a newline; no crash. The demo daemon is enough for this.
5. [ ] **T16 real logout acceptance (OWNER OK; destroys the test link, relink after)**: filesystem
   sweep, `secret-tool search service nuncio` empty, device gone from the phone's Linked devices.
6. [ ] **T17** Signal state behind the keyring: only after the owner answers Q5. ADR first
   (`docs(adr): record where the Signal state lives`), then fake-keyring tests, then the real Secret
   Service acceptance (no key byte in any DB file; keyring stopped → daemon refuses to persist).
   High risk: independent verifier.
7. [ ] **T18 D-Bus**: emit `UnreadChanged` (TODO at `wa.(*Session).publishUnread`), keep
   `UnreadCount` live, export `Introspectable` (`busctl introspect` was empty in M0), activation
   tests green.
8. [ ] **T19 24 h soak (OWNER OK)**: ~10 messages spread out; compare the phone with `nuncio chats`
   / the DB; record start/end, message count, reconnects and their delays, memory start/end.
9. [ ] `nuncio-desktop` builds and its tests pass on the current workspace (it is M3's starting point).

---

## 8. Known advisories still open

Short list; details in the ODD Logs. None blocks; each is a candidate follow-up.

| Source | Advisory |
|---|---|
| M2 L4 (left after U13/U14) | reply/react cannot name my own still-queued send (Q10) |
| M2 L11 | partial receipt failure leaves the chat unread (Q11) |
| M2 L14 | an in-flight `ErrClosed` reaches the client as "WhatsApp did not deliver the media" (daemon exits right after) |
| M2 L15 | a full sync of more than 64 chats drops a subscribed UI (it resubscribes in ~5 s); the not-confirmed mark is TUI-memory only (Q12) |
| M2 L17 | search mixing emoji and words goes through FTS and ignores the emoji; keycap emoji match their digit; `50%` also matches "50 off" (all at baseline too) |
| M2 L18 | quoted media carries no media key/URL/thumbnail; my own message in a LID-addressed **group** is still quoted with the PN; quoted voice note has no PTT flag |
| whatsmeow `SendFrame` | takes its write lock before checking ctx, and a frame whose ctx expired may still be written: a receipt reported "not sent" can still reach WhatsApp while the chat stays unread (upstream, pre-existing) |
| M1 leftovers | `Session` doc comment incomplete (rejected `ConnectFailure` stop); Rust client has no long wait for `download_media` (5 s default vs the daemon's 60 s); `ValidJID` accepts a misplaced hyphen in group ids; `daemon.Chats` error answers an empty list; stickers/PTV not stored; chat names not synced; `link`'s daemon check fails open with no `XDG_RUNTIME_DIR` |

---

## 9. Remaining roadmap

| Milestone | Original estimate | State tonight | Realistic view (assumption) |
|---|---|---|---|
| M1 core daemon | — | Mac part done (T1–T14). Left: T15–T19 (all home/real account), Q1–Q6 | A few home sessions, mostly bounded by the 24 h soak and the owner at the phone |
| M2 TUI | ~3–4 weeks | Code done except inline images (Q8). Left: terminal matrix, real-account check of the M2 flows, advisories above | Days, not weeks: verification work plus whatever the real account turns up |
| M3 desktop (`nuncio-desktop`, gtk4 + libadwaita) | ~8–12 weeks | **Not started.** Only the M0 theme probe exists | Still the long pole. Acceptance (Orca, Hyprland no title bar, notification click from cold start, cold start < 1.5 s, stable memory over a day) is Linux-only |
| M4 Omarchy glue + packaging | ~2–3 weeks | Not started | Linux only: theme/font watcher, Hyprland rounding, `hyprland.lua` snippet, Quickshell bar widget on D-Bus `UnreadCount`, systemd `--user` unit, `packaging/PKGBUILD.in`, GitHub Releases `.pkg.tar.zst` (AUR registrations closed as of 2026-10-15) |

M3 deliverables (plan): list + transcript + composer, command palette (F1), plain-file config,
background mode, notifications whose `default` action opens the right chat, `whatsapp:` link
handling, accessibility roles on bubble widgets. The Mac could only do the shared logic in
`nuncio-client` (config file, `whatsapp:` link parsing, notification-target logic); **none of it was
started**, so at home it can go straight to the real thing.

---

## 10. Environment notes

- **macOS only**: Go tests need `TMPDIR=/tmp` (Unix socket paths over ~104 bytes under
  `/var/folders`). Not needed on Linux.
- **macOS only**: `theme::watch_tests::a_theme_switch_is_noticed_within_500ms` and
  `the_same_palette_is_not_emitted_twice` are flaky (pre-existing on `ae35fea`; FSEvents timing,
  assumption). They should be stable on Linux inotify; if they flake at home, that is new.
- **GGA quirks**: it sends whole staged files (`git show ":$file"`), so pre-existing code reads as new;
  the `## Review verdict` section in `AGENTS.md` keeps FAILED for new or changed lines only. Use
  `GGA_TIMEOUT=600` for big commits. Verdicts are not deterministic: a bare FAILED with no findings
  is re-run once before acting on it.
- **RDD never triggered**: `gentle-ai review assess` answered `review_due: false, under_budget` for
  every commit, up to ~4000 changed lines (it needed `--untracked-scope=exclude` because of `.atl/`).
  GGA plus independent verifiers were the reviews. Suspicious; worth checking at home
  (`gentle-ai review mode status`, then an assess on a known high-risk commit such as `87e4d9e`).
- **Engram mirror pending**: the office Engram session was bound to another project
  (`session_project_mismatch`), so `odd/nuncio-m1/tasks` and `odd/nuncio-m2/tasks` were never
  mirrored. The repo files are complete; mirror them from the home session.
- Toolchain at the office: Go 1.27.1, rustc/cargo 1.99.1. whatsmeow pin unchanged
  (`go.mau.fi/whatsmeow v0.0.0-20261017175727-b2b1e71d45e2`).
- The work Mac holds no `nuncio.db` and no keyring items (checked at end of day).
