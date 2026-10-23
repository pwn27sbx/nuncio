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
- **S6. "New message" (owner, L9: "B").** The rail's primary button is "New message": it asks for a
  phone number and opens that chat with an empty composer, through the same path as a
  `whatsapp://send?phone=…` link (which only fills the composer and never sends). It sends nothing
  by itself; the ban-risk rules are unchanged.
- **S7. Search placement (owner, L15: "C").** Message search stays in the conversation (its header
  ⌕ and Ctrl+F), styled per plan §6.6 (32 px input, 1 px border, `accent` on focus, the hint only
  while empty and focused), since it searches the open chat only. The chat list header gets a
  filter of the chats by name instead (task R8): typing narrows the listed chats; it is client-side,
  with no wire or daemon change.
- **S8. Drafts (owner, L18: "si"; widened in L20).** "New message" never shows another chat's
  draft: a new number opens with an empty composer (a double Enter must never send another chat's
  text to a new number), and an existing chat shows its own saved draft. Task R9: one draft per
  chat, so switching chats (click, link, New message) never carries a draft to another chat and
  never loses one. The model is shared, so this holds in both the desktop app and the TUI.

## Tasks

| ID | Phase (plan §10) | S# | Route | Status | PR / commit |
|---|---|---|---|---|---|
| R0 | P0 tokens: palette roles, contrast tests | S1, S4, S5 | delegated (session backstop) | done, Risk: none | PR #4, merged `e0c137a` |
| R1 | P1 window chrome and layout | S1, S2, S5 | delegated (session backstop) | done, Risk: item 6 (visual, checked on Omarchy) | PR, see L11 |
| R2 | P2 chat list and transcript | S1, S5 | delegated (session backstop) | done, Risk: item 6 (visual, checked on Omarchy) | PR, see L12 |
| R3 | P3 composer | S1, S3, S5 | delegated (session backstop) | done, Risk: item 6 (visual); inline checks, no verifier (plan: low-medium; send path unchanged) | PR, see L13 |
| R4 | P4 palette, search, settings, dialogs | S1, S5 | delegated (session backstop) | done, Risk: item 6 (visual); verifier passed | PR, see L16 |
| R5 | P5 TUI parity | S1, S5 | delegated (session backstop) | done, Risk: item 6 (visual) | PR #9, merged `02e0060` |
| R6 | P6 polish and proof (folds into R4/R5 if small) | S1 | delegated (session backstop) | done, Risk: item 6 (window wiring and visuals only on Omarchy); verifier passed | PR, see L24 |
| R7 | "New message" in the rail (after R1) | S6 | delegated (session backstop) | done, Risk: item 6 (visual); verifier passed after the S8 fix | PR, see L19 |
| R8 | Chat filter by name in the list header (after R4) | S7 | delegated (session backstop) | done, Risk: item 6 (window wiring only runs on Omarchy); verifier passed | PR, see L22 |
| R9 | One draft per chat in the desktop app (follow-up, high priority) | S8 | delegated (session backstop) | done, Risk: item 3 (shared model, TUI too); verifier passed | PR, see L20 |

Route reason: this session resumed after a compaction, so the parent orchestrates and an Opus
writer with a clean context writes each phase; an independent verifier checks each phase
(R1 and R2 touch the shared view and layout).

## Follow-ups (to judge by eye on the Omarchy machine, with several themes)

- **F1.** `text` on `surface_chrome`: 7:1 (plan) cannot hold on Gruvbox and others. Parent's
  proposal: 7:1 on `surface`, 4.5:1 (WCAG AA) on `surface_chrome`. Test:
  `theme::roles::tests::text_reaches_7_to_1_on_the_chrome_of_gruvbox_and_latte` (ignored).
- **F2.** `accent_on` under 4.5:1 on Latte and Rose Pine. Parent's proposal: when neither surface
  nor text reaches 4.5:1 on the accent, use black or white, whichever contrasts more; the accent
  never changes.
- **F3.** If anything still looks rounded on Omarchy after R1, check `~/.config/gtk-4.0/gtk.css` and
  `hyprchroma.css` first: a user `gtk.css` overrides the app's CSS.
- **F4.** Photo box: the plan says 240×180, `PHOTO_BOX` stays 300 (`pictures.rs`), since changing it
  changes decode targets; judge both sizes on screen.
- **F5.** A row is rebuilt when its group meta line moves (a new message from the same author within
  5 min), which restarts an animated sticker in that row; check whether it is noticeable.
- **F6.** The failed bubble lost its red fill (plan §6.3: text, `warning` meta line and Retry carry
  it); check it still reads as failed.
- **F7.** Not built (S5, no model data): system rows, read ticks, group member count. The
  transcript's left-aligned 72-character measure moves to R6.
- **F8.** The composer: no attach button (no attach action exists, S5); Shift+Enter kept as a newline
  besides Alt+Enter (existing key, plan §7 keeps current keys); `Sending…` shows while a send of
  the open chat waits and nothing new is typed (the draft empties on send). Judge on screen.

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
- **L7** (2026-10-23, owner): "no te deje indicaciones en otro lado pero tu haz el trabajo no
  neceistas mi permiso a menos que sea sobre decisiones". Standing approval: push the
  `feat/redesign-*` branches, open each phase's PR against `main`, and merge it with a merge commit
  once the six checks are green (and the verifier passed for a risky phase), without asking. Only
  product decisions go to the owner. Never force-push, tag or release.
- **L8** (2026-10-23, R1): the model has no "new message" action, so P1's rail primary button is
  `Commands` (opens the palette) for now; the wire's `Chat` has no member count, so a group's header
  shows only its name. Owner asked: A keep `Commands`, B build "New message", C no primary button.
- **L9** (2026-10-23, owner): "B". → S6, task R7. Until R7 lands, the button stays `Commands`.
- **L10** (2026-10-23): the owner's machine has `~/.config/gtk-4.0/gtk.css` and `hyprchroma.css`
  (home report §2); a user `gtk.css` has USER priority (800), above the app's provider (600). → F3.
- **L11** (2026-10-23, R1): three `gtk::Paned` panes in `nuncio-root`, `src/style.rs` CSS from the
  roles in the one reloaded provider, breakpoints at 1099/759, the stop strip, filters on the
  existing `chat.archived`/`chat.unread`. Verifier: S2 and S5 pass; blockers B1 (the open chat
  leaked into every filter and count) and B2 (lost selection after a row rebuild) and advisory A1
  (window rounded without Hyprland) fixed in one batch, RED then GREEN, 5 mutants killed; parent
  reran `present` tests and the smoke test. Open for the owner's eyes: the header lost the
  "linked · N unread" total (rail counts are chats); switches are square too. Owner rule from now:
  no visible windows or screenshots on the Mac.
- **L12** (2026-10-23, R2): chat rows per §6.2 and the transcript per §6.3, one `ListBoxRow` per
  message (day label and author line inside it, so the row index stays the message index),
  grouping by author within 300 s inclusive. Verifier: S1 and S5 pass, no blockers; advisories
  logged as F4–F7. Parent reran the `present` tests.
- **L13** (2026-10-23, R3): docked composer per §6.4 with the S3 keys; Send only with non-blank
  text, `Sending…` insensitive while a send of the open chat waits; reply strip; 6-line cap from
  the measured line height. RED (16 compile errors, one real smoke failure), GREEN; 4 mutants
  killed. Parent checked the scope (desktop only, the send call unchanged) and reran the key test
  and the smoke test. Choices for the owner: F8.
- **L14** (2026-10-23, R4): the plan's §6.6 global search with `from:`/`is:unread` does not exist;
  the only search is the per-chat wire `search`. The writer moved it into the list header.
  Verifier: S1 and S5 pass, no blockers. Owner asked: A keep it there, B back to the conversation,
  C back to the conversation plus a chat filter by name in the list header.
- **L15** (2026-10-23, owner): "si la C". → S7; R4 moves search back; R8 builds the filter.
- **L16** (2026-10-23, R4): palette per §6.5 (560 px popover, scrim, groups, best match first),
  settings per §6.7 (480 px column, `meta` headers, libadwaita rows, 0600/0700 kept), keys `/`,
  j/k, g/G; §6.8 skipped (the app has no dialogs), u/Shift+U skipped (no action). After L15 the
  per-chat search is back in the conversation, restyled per §6.6; the list header keeps its title
  and its existing menu. Verifier (before the move): S1 and S5 pass, no blockers. Parent reran the
  smoke test after the move.
- **L17** (2026-10-23, R7): "New message" built through `number_link` (percent-encodes every
  typed byte, then `parse_link`) and `open_link`; never a send; the error never repeats the
  number. Verifier: ban-risk, validation (24 probes) and privacy pass. Blocker: the desktop
  composer is one global draft, so a draft typed for Ana stays in the new chat and a double
  Enter sends it to the new number. Owner asked A (keep, as links do) or B (always empty).
- **L18** (2026-10-23, owner): "si" to B plus one draft per chat next. → S8, R9.
- **L19** (2026-10-23, R7): S8 fix: `new_message` empties the draft once the number is valid;
  RED smoke and unit tests reproduce the verifier's double Enter and now see no send; the
  mutation (no clear) fails both. Parent reran both tests.
- **L20** (2026-10-23, R9): one draft per chat in the shared model (`drafts` by chat key plus a
  loose draft; drafts follow a key rename and join on a merge). Independent verifier: PASS on
  every S8 point, no blockers; differential against 680f35b shows the TUI also changed: at the
  baseline j/k carried Ada's draft to Grace, so Enter could send it to the wrong chat. Accepted
  as part of S8 (both clients) instead of TUI-only code that would bring that risk back; the TUI
  README documents it. Also changed: a reply in progress now stays with its chat instead of being
  cancelled on a chat switch (desktop README). Advisories kept as follow-ups: blank drafts are
  kept per visited chat (no visible effect), a few README lines exceed 100 columns.
- **L21** (2026-10-23): the GTK smoke test only runs on Omarchy; on the Mac every cargo test
  runs with `GDK_BACKEND=broadway BROADWAY_DISPLAY=:99`, so it prints "smoke: skipped" (owner:
  no windows or Spaces changes on the Mac). Run it on Omarchy before closing the redesign.
- **L22** (2026-10-23, R8): the list header's ⌕ swaps the title for a filter entry; typing keeps
  the chats whose shown name (`chat_title`) holds the trimmed text, ignoring case; Escape clears
  and closes it before closing the message search. Assumptions (plan silent): substring match,
  the rail counts ignore the name filter, `/` still opens the message search. Mutation: 8 mutants,
  all caught. Independent verifier: PASS, no blockers; the window wiring was checked by reading
  only (the smoke test is skipped on the Mac and in CI), so check it on Omarchy. Parent fixed the
  README key table row for Escape. Moved to R6: focus goes nowhere after Escape closes the filter
  (the message search does the same at the baseline).
- **L23** (2026-10-23): PR #10 (R7) merged after one rerun of the theme 500 ms test.
- **L24** (2026-10-23, R6/P6): no transitions except 120 ms hover fills and the palette scrim
  fade, all off with `gtk-enable-animations`, followed live; 1 px accent focus rings; 28 px
  header icon buttons with hover and checked fills; accessible names on the reaction buttons
  (the rest already had tooltips and names); the transcript at most 72 characters wide at the
  left (`Measure` widget); focus back to the chat list after Escape closes the filter; README
  "Design rules" and "On Omarchy" checklist. 12 CSS mutants all caught. Verifier: no blockers;
  parent applied its high advisory: the transcript list box takes the scroll's adjustment
  explicitly, since `Measure` is not a scrollable parent (PageUp/PageDown). For the owner on
  Omarchy: bubbles now end at the 72-character column on a wide window (plan read literally);
  the composer height change is not animated (§4.4 allows it); focus after Escape closes the
  message search still goes nowhere (plan silent). Remaining: the On Omarchy checklist, F1–F8.
- **L25** (2026-10-23): PRs #11 (R9) and #12 (R8) merged on green.
- **L26** (2026-10-23): PR #13 (R6) merged on green; the redesign is complete on `main`. Report and
  on-screen checks for the home machine: `odd/handoff-redesign-omarchy.md`.
