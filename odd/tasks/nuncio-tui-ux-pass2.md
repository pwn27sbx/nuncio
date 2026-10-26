# nuncio TUI — UI/UX pass 2 (2026-10-25)

Objective: the TUI reads like a finished chat client — one hierarchy, one place per datum,
legible meta, a list that scans, a transcript that guides, a footer that helps.
Branch: `feat/tui-ux-pass2`, from `main@a35c700`. Delivery: `single-pr`, work-unit commits.
Runners: `cargo test -p nuncio-tui`, `cargo test --workspace`, `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`.

## Specs

- S1 (owner, 2026-10-25, verbatim): "genial hora de mejorar la UI/UX porque esta horrible asi
  que arma el plan y lo verificas luego un agente con memoria limpia que lo verifique para que
  cubra todo y nada se escape y me lo das para aprobarlo luego"
- S2 (owner, 2026-10-25, verbatim): "aprobado" — Q1–Q6 of the plan are approved as written.
- S3 The plan is the specification: `~/Documents/ObsidianVault/nuncio/2026-10-25-tui-ux-pass2/README.md`
  (v3, audited twice). Its §3.3 rules are repeated here because they are the executable
  contract; its §2 holds the defect evidence with `lib.rs` line anchors.
  - R1 One status line only, in the footer's right slot; the header carries no telemetry.
  - R2 Footer: a notice, when there is one, takes the left side (it answers something the owner
    just did); then the primary key in the accent and `? all keys`; extra keys are added while
    they fit and drop from the tail in priority order; no key twice; the line never exceeds the
    width (four context keys at most at 80 columns). With a notice that fills the line, the key
    hints are the ones that give way, never the state on the right.
  - R3 Meta and placeholders use `text_muted()`; no text style in the TUI carries `Modifier::DIM`;
    `text_faint` stays unused (no contrast floor).
  - R4 Chat row columns fixed: name, inline markers when present, 5-column time, 4-column
    badge (`● 3`, `●99+`); preview one line, truncated.
  - R5 Only the pane with the keys paints `surface_active` (conversation group = Messages or
    Compose); an unfocused selection keeps `▌`; overlays exempt.
  - R6 Breakpoints: >= 100 → 40 %; 60–99 → 32 columns; < 60 → single pane with `Tab`, and the
    hidden pane's hit-test rectangles are cleared every frame.
  - R7 Every empty or error state: one `text_muted` sentence plus one actionable hint.
  - R8 Overlays share one width rule `min(area.width - 4, 76)`, right-aligned keys.
  - R9 Every key the TUI handles appears in the footer (as many as fit) or in `?`, and every
    key the footer prints is handled; a test enumerates both sets.
  - Q3 filters: title stays `Chats (12)`; with a filter other than All, `Chats · Unread (3)`;
    `[`/`]` cycle All/Unread/Archived; `Filter` moves to `nuncio-client`.
- S4 Repo rules (`AGENTS.md`): tests first and through the public interface; no `unwrap` outside
  tests; `cargo fmt` and `clippy -D warnings` clean; every changed message/option updates its
  help text and docs; no secrets, no real phone numbers; small Conventional Commits, no AI
  attribution.
- S5 Units and files are the plan's §5 table (W1–W11), including the test files each unit
  breaks and must update. Nothing outside `crates/nuncio-tui`, `crates/nuncio-client` (the
  `Filter` move) and `crates/nuncio-desktop` (its `Filter` import) is touched.
- S6 Acceptance: the plan's coverage matrix (§6) has no empty cell; the gate is `cargo fmt
  --check`, `clippy -D warnings`, `cargo test --workspace`, plus screenshots at 80×24, 120×40
  and 204×46 in Ghostty/foot/alacritty and one light theme, and the mouse checks.
- S7 Not in scope: the GTK app's look, the wire protocol, the daemon, new dependencies, theme
  editing, rebinding existing keys, Orca.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| W1 | S3 R3: `text_muted` for meta and placeholders, no `DIM` | inline | done: RED (DIM found on every screen) → GREEN; the test scans both sizes for `Modifier::DIM`, holds the role's 4.5:1 on both surfaces and checks a time, the day label, a footer key and an empty state; review approved with 7 informational findings, folded into W2/W3 | 8f05077 |
| W2 | S3 R1: header brand + accent unread, `FIXTURE DATA` case, version into `?` | inline | done with W3 (one unit: moving the state between the two chrome lines cannot be split without a red tree). Header = `nuncio` + `N unread` accent (only when > 0) or `FIXTURE DATA`; the daemon build, the session and the picture protocol now head the `?` panel | this commit |
| W3 | S3 R2: footer budget/priority, notice keeps primary + `?`, state right | inline | done: ordered hints per pane, tail dropped by budget, no repeated key, state (`linked`/`reconnecting…`/`offline`/`not linked`/`unknown`/`no daemon`) right-aligned in `warning` unless linked; a notice takes the left side. Updated `design.rs`, `keys.rs`, `render.rs`, `app.rs`, `images.rs` and `stop_reason.rs` | this commit |
| W4 | S3 R4: row time column, inline markers, badge | inline | done: two fixed rows — name + inline markers + a five-column time slot; preview + a four-column badge `●n`/`●99+`; a narrow list drops the time, then the markers, before cutting the name under six columns. Updated `design.rs` (accent set, badge drawn), `keys.rs` (markers follow the name), `render.rs`, `app.rs`; README's chat-list section rewritten | this commit |
| W5 | S3 R5: one selection fill, unfocused keeps `▌` | inline | done with W6: the list fills only while it has the keys, the conversation (composer included) the same, overlays exempt; `design.rs` scans both panes | this commit |
| W6 | S3 R6: breakpoints, single pane, cleared hit-test rects | inline | done: 40 % at 100+ columns, a fixed 32 from 60 to 99, one pane under 60 with `· Tab` on its rule; the map is reset every frame so a hidden pane takes no click. Tests pin 48/32/32 and no split at 59, and a click on the hidden list. README's focus and widths sections | this commit |
| W7 | S3 R7: transcript start marker, scroll gutter, empty hint, `· sending` at 40 | inline | done with W8/W10: the marker opens the transcript (no blank under it), `▲`/`▼` in the free right column, `No messages yet · i writes the first one`, `No chat open · Tab switches panes`, `No chats yet · nuncio link` | this commit |
| W8 | S3 R7/Q4: composer placeholder, `Enter sends`, accent bar | inline | done: `i or Enter to write` when empty, the accent `▌` on the rule when focused, `Enter sends · Alt-Enter newline` on the rule's right when there is a draft; `?` and the README say the same | this commit |
| W9 | S3 R8: overlay width rule, right-aligned keys, picker < 52 | inline | pending | — |
| W10 | S3 R7: state copy (list, transcript, offline, archived-only) | inline | done for the list, the empty transcript and the no-chat pane; the offline and stop-reason copy came with W3, and an archived-only list already sections rather than empties, so it keeps the chat list as it was | this commit |
| W11 | S3 Q3: `Filter` in `nuncio-client`, active-filter tag, `[`/`]` | inline | done: `Filter` moved to `nuncio-client` (the desktop re-exports it, behaviour unchanged), `[`/`]` cycle All/Unread/Archived, the rule shows `Chats · Unread (3)`, the mouse maps a drawn row to its model chat through `map.chat_indices`, navigation and frames keep the selection inside the filter, and an empty filter says so. The tag is drawn in the rule's own style, not the accent: the rule title is one span, and painting part of it would need a second style channel for one word (recorded as a deviation from Q6) | this commit |

## Log

- L1 (owner, 2026-10-25, verbatim): "genial hora de mejorar la UI/UX porque esta horrible asi
  que arma el plan y lo verificas luego un agente con memoria limpia que lo verifique para que
  cubra todo y nada se escape y me lo das para aprobarlo luego"
- L2 (owner, 2026-10-25, verbatim): "aprobado" — approves Q1–Q6.
- L3 Plan v1 was audited cold against the code: REWORK, 8 blockers (red tree between units,
  unsatisfiable badge/filter/footer widths, `ScreenMap` stale rectangles, `window.rs` missing
  from the `Filter` move). v2 fixed them; the second read gave APPROVE WITH CHANGES with two
  required edits (`tests/render.rs` in W11, R9 vs R2), applied in v3. Evidence is in the plan's
  §9 and the audit transcripts.
- L4 Implemented starting on the feature branch; the untracked `.atl/` directory is not ours
  and stays untouched.
- L5 (evidence, 2026-10-25) W1 review: granted by the owner, two lenses (readability,
  reliability), result approved, 7 informational findings: the `dim` local, a weak assertion on
  the message text colour, the unnamed 4.5 constant, the copied contrast math, the ledger still
  saying pending and the scope sentence not covering `.gitignore`/`docs`. All fixed in W2/W3
  except the contrast-copy (kept, with its reason in the test) — 213 changed lines.
- L6 (decision, 2026-10-25) `.atl/` (the agent skill registry) is now ignored: it is generated
  tooling state, and its presence forced an untracked-selection step in every review preflight.
  Its own commit, `7bc9aa4`.
- L9 (evidence, 2026-10-25) W5-W11 landed in four commits, each with the design tests
  updated and the README/docs in the same change: `47d8909` (selection fill + breakpoints),
  `ce75f72` (start marker, gutter, composer rule and copy), `a0e6159` (overlay width, filters,
  the `Filter` move to `nuncio-client`) and `6401ee0` (filtered navigation). `cargo test
  --workspace` and `clippy -D warnings` are green after each; the branch is pushed.
- L11 (evidence, 2026-10-25) The final native review of the four closing commits approved with
  two BLOCKERs refuted (the composer consumes `[`/`]` only after its own key handler, and the
  start marker's owner entry is padded by the first `owners.resize`), and three warnings fixed
  here: the composer rule counted a column it never drew when quoting a reply, the Archived
  filter had no positive case, and the exact 60/100-column breakpoints were unasserted.
- L10 (deviation, 2026-10-25) Q6's accent on the active filter is not implemented: the rule
  title is drawn as one span, and splitting it for one word would add a second style channel
  to the pane rule. The accent keeps the badge, the selection bar, the header count, the
  focused composer rule and the footer's primary key.
- L8 (evidence, 2026-10-25) The W2/W3 review approved with 8 informational findings; two were
  behavioural and fixed in `68c8dd4`: the state's right slot lost text at 80 columns while five
  hints stayed, and `j/k select` had left the selected-message hints. The footer now drops the
  optional keys, then `? all keys`, then the primary, and only then cuts the state.
- L7 (decision, 2026-10-25) W2 and W3 are one commit: the session state has to leave the header
  in the same change that gives it a home in the footer, or the tree is red between them (the
  cold audit's B1). Order after that: W4, W5, W6, W7, W8, W9, W10, W11.
