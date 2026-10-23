# nuncio: UI/UX redesign (desktop first, then TUI)

Objective: build the design plan in `docs/design/redesign-plan.md` (written 2026-10-23 on the home
machine, Sealwax as reference), phase by phase. Delivery: `stacked-to-main`, one PR per phase
against `main`, each merged on its own with the six CI checks green. Runner: `cargo test --workspace`.

## Specs

- **S1. The plan is the spec.** `docs/design/redesign-plan.md` §2–§12 are built as written, with the
  three decisions in its §13. Owner, 2026-10-23: "Nothing else in this plan is open: §4–§12 are
  ready to build as written."
- **S2. Corners follow Hyprland.** Owner, 2026-10-23: "las esquinas se toman de la config que manda
  hyperland en este caso yo las tengo en 0". The radius of every element is
  `hyprctl -j getoption decoration:rounding` (plan §4.3). It is 0 on the owner's machine, so nothing
  is rounded there, the window included: libadwaita's own window corners must not round when
  Hyprland says 0 (the home run, T4, saw "rounded corners" on the window).
- **S3. Send keys.** Enter sends, Alt+Enter is a newline, Ctrl+Enter is an alias (plan §7). The
  Send button's tooltip names `Enter`. (The plan's §6.4 says `Ctrl+Enter` in the tooltip, which
  contradicts §7; resolved by the parent from §7.)
- **S4. Contrast on real palettes (assumption, parent).** The §4.1 contrast contract is tested on
  fixtures. When a real Omarchy palette gives `text_muted` under 4.5:1 on a surface, the role is
  moved toward `text` until it reaches 4.5:1; `text` itself is never changed.
- **S5. Nothing outside the design.** Plan §12: no change to the wire, the store, the ban-risk
  rules, the notification rules or the media caps; no new dependency; no feature added.

## Tasks

| ID | Phase (plan §10) | S# | Route | Status | PR / commit |
|---|---|---|---|---|---|
| R0 | P0 tokens: palette roles, contrast tests | S1, S4, S5 | delegated (session backstop) | done, Risk: none | branch `feat/redesign-p0-tokens` |
| R1 | P1 window chrome and layout | S1, S2, S5 | delegated (session backstop) | pending | |
| R2 | P2 chat list and transcript | S1, S5 | delegated (session backstop) | pending | |
| R3 | P3 composer | S1, S3, S5 | delegated (session backstop) | pending | |
| R4 | P4 palette, search, settings, dialogs | S1, S5 | delegated (session backstop) | pending | |
| R5 | P5 TUI parity | S1, S5 | delegated (session backstop) | pending | |
| R6 | P6 polish and proof (folds into R4/R5 if small) | S1 | delegated (session backstop) | pending | |

Route reason: this session resumed after a compaction, so the parent orchestrates and an Opus
writer with a clean context writes each phase; an independent verifier checks each phase
(R1 and R2 touch the shared view and layout).

## Log

- **L1** (2026-10-23, owner): "mi opencode de mi omarchy te envia esto para la UI/UX
  (~/Downloads/nuncio-design-plan-2026-10-23.md)"; then "si empezemos por eso que es
  facil, al final hacermos el rediseño".
- **L2** (2026-10-23, owner): "las esquinas se toman de la config que manda hyperland en este caso
  yo las tengo en 0, esa info te ayuda?" → S2.
- **L3** (2026-10-23, owner): delivery "A": one PR per phase against `main`.
- **L4** (2026-10-23): screenshots for the PR bodies can only be taken on the Omarchy machine; the
  Mac verifies with tests, clippy and the desktop smoke test (which skips without a display).
- **L5** (2026-10-23, R0): roles in `crates/nuncio-client/src/theme/roles.rs`, fixtures from real
  Omarchy themes (Gruvbox, Flexoki Light, Miasma, a Catppuccin Latte excerpt). RED 4 failing, GREEN
  8 passed 1 ignored; mutation of the S4 correction and of the luminance pick each fails a test.
  The writer found two parts of the plan's §4.1 contract that real palettes cannot meet, and the
  parent recomputed them: `text` on `surface_chrome` is 6.42:1 on Gruvbox (`#d4be98` on
  `#3c3836`; 8.16:1 on `surface`), also under 7:1 on Latte, Everforest and Tokyo Night; `accent_on`
  on `accent` is at best 4.34:1 on Latte and 3.14:1 on Rose Pine. Interpretations taken: `accent_on`
  is surface or text, whichever contrasts more; dark or light is decided by luminance.
- **L6** (2026-10-23, owner): "todo esto lo dejamos como followups para verlo bien en mi omarchy
  cuando pruebe los temas y lo vea con mis ojos". → Follow-ups F1–F2; the code keeps the plan's
  literal rules, and the 7:1 text-on-chrome test stays `#[ignore]` with the open decision.

## Follow-ups (to judge by eye on the Omarchy machine, with several themes)

- **F1.** `text` on `surface_chrome`: 7:1 (plan) cannot hold on Gruvbox and others. Parent's
  proposal: 7:1 on `surface`, 4.5:1 (WCAG AA) on `surface_chrome`. Test:
  `theme::roles::tests::text_reaches_7_to_1_on_the_chrome_of_gruvbox_and_latte` (ignored).
- **F2.** `accent_on` under 4.5:1 on Latte and Rose Pine. Parent's proposal: when neither surface
  nor text reaches 4.5:1 on the accent, use black or white, whichever contrasts more; the accent
  never changes.
