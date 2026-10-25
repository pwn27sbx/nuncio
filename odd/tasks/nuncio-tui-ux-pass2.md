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
  - R2 Footer: notice (if any), primary in the accent and `? all keys` always fit; extra keys
    are added while they fit and drop from the tail in priority order; no key twice; the line
    never exceeds the width (four context keys at most at 80 columns).
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
| W1 | S3 R3: `text_muted` for meta and placeholders, no `DIM` | inline | pending | — |
| W2 | S3 R1: header brand + accent unread, `FIXTURE DATA` case, version into `?` | inline | pending | — |
| W3 | S3 R2: footer budget/priority, notice keeps primary + `?`, state right | inline | pending | — |
| W4 | S3 R4: row time column, inline markers, badge | inline | pending | — |
| W5 | S3 R5: one selection fill, unfocused keeps `▌` | inline | pending | — |
| W6 | S3 R6: breakpoints, single pane, cleared hit-test rects | inline | pending | — |
| W7 | S3 R7: transcript start marker, scroll gutter, empty hint, `· sending` at 40 | inline | pending | — |
| W8 | S3 R7/Q4: composer placeholder, `Enter sends`, accent bar | inline | pending | — |
| W9 | S3 R8: overlay width rule, right-aligned keys, picker < 52 | inline | pending | — |
| W10 | S3 R7: state copy (list, transcript, offline, archived-only) | inline | pending | — |
| W11 | S3 Q3: `Filter` in `nuncio-client`, active-filter tag, `[`/`]` | inline | pending | — |

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
