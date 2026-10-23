# nuncio — UI/UX redesign: design plan for the Mac session

**Written:** 2026-10-23, on the home machine. **To be built by:** the work-Mac Claude Code.
**Reference:** Sealwax 0.4.8 (`io.github.pixdevsapps.Sealwax`), installed here, studied live.
**Scope:** the GTK desktop app first, the TUI second, with the shared view models untouched in
their semantics.

This is a *design* plan: it says what the interface is, with numbers, and in what order to build
it. It does not add features, touch the wire protocol, the store, the daemon, the ban-risk rules or
the notification rules. Every number below is either measured from Sealwax on this machine or
derived from the Omarchy palette that both apps already read.

---

## 1. Why Sealwax is the right reference

It is the same species as nuncio, not a web app that happens to look good:

- **GTK 4 + libadwaita, written in Rust** — the exact stack of `nuncio-desktop`.
- It follows **the live Omarchy theme and font**, light and dark, with corners that follow
  Hyprland, and **no title bar** on Hyprland.
- It has **layouts for wide, half-screen and narrow tiled windows**, with panes the user can drag.
- It is **keyboard-first** with one-key shortcuts and a command palette (F1).
- Settings live in plain files (`config.toml`, `keys.toml`, `accounts.toml`), as nuncio's do.

So its proportions, density and restraint are reproducible with nuncio's current dependencies: a
`gtk::CssProvider` and the palette it already loads. Nothing here needs a new crate.

### 1.1 What I measured on screen (1920×1080, Gruvbox dark, window tiled 1910×1042)

| Measured | Value | How |
|---|---|---|
| Pane widths | sidebar **218 px**, list **380 px**, reading pane **1311 px** | 1 px separators at x=223 and x=603 |
| Separators | **1 px**, colour `#272524` (a shade below both surfaces) | sampled |
| Chat/message row height | **85 px** for four text lines (sender, subject, two preview lines) | band analysis of the list |
| Row padding | 12 px left of the avatar, 16 px between avatar and text, 24 px under the last line | measured |
| Avatar | **29×29 px, square corners** (the corner pixel is the fill) | bounding box of the accent block |
| Backgrounds | list and sidebar `#3c3836` (Omarchy bg1), reading pane `#2a2a29` (bg0) | sampled |
| Body/preview text | `#fbfbfb` for name and subject; `#9a8c72` for the preview | brightest pixel per band |
| Accent | `#7caca1` (Omarchy accent) on the single filled button | sampled |
| Active tab | `#37403e`, one step above the surface | sampled |
| Reading pane | subject ≈ 28 px semibold, sender 13 px semibold, meta 11–12 px muted, body 13–14 px | measured |
| Corners | **0** everywhere (and Hyprland's `decoration:rounding` is 0 here, so the app and the compositor agree) | `hyprctl getoption` |

---

## 2. The ten rules that make it look "polished"

These are the transferable principles, in priority order. When a later decision is unclear, resolve
it with this list.

1. **One surface hierarchy, no cards.** Three tones: chrome (bg1), content (bg0), hover/active one
   step up. Panes are separated by **1 px lines**, never by cards, borders around every block, or
   shadows. Nothing floats.
2. **Square by default.** 0 px radius on buttons, avatars, inputs, chips, dialogs, thumbnails. The
   only curves on screen are glyphs.
3. **Density with air.** 85 px rows holding four lines, 12–24 px of padding, and generous
   *horizontal* breathing room inside a row. Dense does not mean cramped.
4. **One accent, one filled button per screen.** The accent marks the primary action and the three
   or four things that are alive (unread dot, active tab, selection bar). Everything else is
   foreground or muted foreground.
5. **Type does the hierarchy.** Weight and size carry meaning: subject large, sender semibold,
   preview regular and muted, meta small. No colour-coding of senders or folders.
6. **Actions are icons until they matter.** Toolbar actions are 28 px icon buttons with a hover
   fill and no border. Text buttons are reserved for sentence-level decisions ("Show images",
   "Always for this sender").
7. **The document wins.** In the reader the content is a single un-tinted sheet with a comfortable
   measure; chrome retreats. The transcript should feel the same: the messages are the content.
8. **Quiet, inline safety.** The security notice is one muted line with two text buttons. No
   banners, no shields, no colour.
9. **Selection is a thin marker, not a block.** Sealwax uses a subtle raised row plus context; the
   *only* strong marks are the 1 px accent bar and the unread dot. Never a rounded filled card.
10. **The palette is the desktop's.** Colours come from `colors.toml`, so the app is "Gruvbox" or
    "Catppuccin" because the desktop is, including live switches. Hardcoding is a bug.

---

## 3. Diagnosis: what is wrong with nuncio today

From the screenshots taken during the run (desktop and TUI). These are the things to remove, not
just to restyle:

1. **Everything is rounded and boxed.** Chat-list selection is a big rounded filled block; the
   composer is a rounded box; buttons are pills. With a 0-rounding compositor, the app fights the
   desktop.
2. **Too many accent-coloured objects.** Magenta avatars, an accent "Send" pill, an accent
   selection, accent ticks: the eye has no primary thing to land on.
3. **The transcript is mostly empty.** One message sits at the bottom of a 1000 px void; there is no
   day header rhythm, no grouping, no measure limit, and the header repeats the same information
   twice ("nuncio / Linked" and the chat title / "Linked").
4. **The list row is a card, not a row.** Name and preview are stacked inside a filled rounded
   rectangle; time and unread compete; there is no unread dot, and the avatar is a big saturated
   circle with a letter.
5. **The composer is a giant empty field** with a floating pill button, not a docked bar with a
   known height and a clear send affordance.
6. **The empty state is a void.** No icon, no sentence, no hint at the keyboard.
7. **The TUI uses double borders everywhere**, so every pane shouts equally; the palette is used
   decoratively.
8. **No density contract.** Row heights, paddings and type sizes differ between the list, the
   transcript and the composer, which is what reads as "unfinished".

---

## 4. Design tokens

### 4.1 Palette: semantic roles over the Omarchy file

`crates/nuncio-client/src/theme.rs` currently parses `background`, `foreground`, `accent`, the six
ANSI colours and their bright variants. Omarchy's `colors.toml` also carries
`lighter_background`, `dark_foreground`, `light_foreground`, `bright_foreground` (the file's own
test lists them), which the struct does not read yet. **Add them** (optional, serde default) and
expose semantic roles; both apps consume the roles, never raw ANSI names:

| Role | Source (dark) | Why |
|---|---|---|
| `surface` | `background` | content pane (transcript, reader) |
| `surface_chrome` | `lighter_background` (falls back to a +8 % lighten of `surface`) | sidebar and chat list |
| `surface_hover` | `surface_chrome` +6 % toward `foreground` | row hover |
| `surface_active` | `surface_chrome` +10 % | selected row, active tab |
| `border` | `surface` +10 % toward `foreground` (or `darker_background` when present) | 1 px lines |
| `text` | `foreground` | names, subjects, message text |
| `text_muted` | `light_foreground` (fallback `foreground` at 70 %) | previews, times, meta |
| `text_faint` | `dark_foreground` | disabled, tick glyphs, placeholders |
| `accent` | `accent` | primary button, unread dot, selection bar |
| `accent_on` | `surface` or `background`, whichever contrasts more with `accent` | the label on a filled accent button |
| `danger` | `red` | destructive confirmations only |
| `warning` | `yellow` | "not confirmed", link warnings |
| `success` | `green` | sent ticks, when a colour is needed at all |

**Contrast contract (enforced by a unit test, see §10 P0):** `text` on `surface` and on
`surface_chrome` ≥ 7:1; `text_muted` on both ≥ **4.5:1**; `accent_on` on `accent` ≥ 4.5:1.

Measured reference: Sealwax's preview (`#9a8c72` on `#3c3836`) is **3.52:1** — beautiful, but it
fails AA. The same slot with `#bdae93` is **5.32:1**. **We match the look and beat the ratio**: use
`light_foreground`/`#bdae93`-class tones for previews. The accent itself is 4.57:1 on the chrome,
which is fine for a 6 px dot and a 2 px bar; it is **not** used for body text.

Light themes: the same roles, sourced from the light `colors.toml`; `surface_chrome` is the *darker*
of the two backgrounds in light mode (the file's `lighter_background` is named for dark themes), so
the mapping picks by luminance, not by name.

### 4.2 Type scale

| Token | Size / weight | Use |
|---|---|---|
| `title_l` | 26 px semibold | conversation title in the reader header area, only where a document title is needed |
| `title` | 15 px semibold | pane headers ("Chats", the open chat's name) |
| `body` | 13 px regular | message text, form values |
| `label` | 13 px semibold | sender name, chat name, subject line in the list |
| `meta` | 11 px regular | times, counts, ticks, keybindings, section headers (uppercase, +0.06 em tracking) |

Font family: whatever `fc-match monospace`/the theme's UI font resolves to; **never** hardcode a
family name (nuncio already follows the theme's font; keep that).

### 4.3 Spacing, sizes, lines

- Spacing scale: **4, 8, 12, 16, 24, 32, 48** — nothing else.
- Borders: **1 px**, `border`, drawn as a `separator` widget or CSS `border`, never both.
- Radii: **0** everywhere. If a future platform forces a radius, take it from
  `hyprctl -j getoption decoration:rounding` and apply it to **all** corners, as Sealwax does.
- Control heights: icon button **28×28**, text button **32**, input **32**, row **72** (chat) /
  **85** (mail-like, not used), header **44**, composer min **48**.
- Icon size: **16 px** glyphs, stroke-thin set (the GTK symbolic icons), never filled shapes.
- Avatars: **32×32**, **square**, initials at 11 px semibold, background one of six muted tones
  derived from the palette at ≤ 45 % saturation, foreground = `text`. No gradients, no photos in
  v1 (a photo, if added later, is a square 32 px thumbnail with a 1 px `border`).

### 4.4 Motion

Only three animations, all ≤ 120 ms, all disabled when `gtk-enable-animations` is off (read it
correctly — `gsettings`, see the home report §4.5): hover fill fade, palette scrim fade,
composer height change. No slide-ins, no bounce, no skeleton shimmer.

---

## 5. Layout

Three panes, one window, no title bar. Same structure as Sealwax, adapted to a messenger.

```
┌──────────┬───────────────┬──────────────────────────────────────┐
│ rail     │ chat list     │ transcript                           │
│ 200      │ 360           │ rest                                 │
│          │               │                                      │
│ New      │  Chats  ⌕  ⋯  │  Name                         ⋯     │
│ message  │ ───────────── │ ──────────────────────────────────── │
│          │  row 72       │  day label                           │
│ All      │  row 72       │  message                             │
│ Unread   │  row 72       │  message                             │
│ Archived │  …            │  ────────────────────────────────    │
│          │               │                                      │
│ ▪ linked │               │                                      │
│          │               │  composer                            │
└──────────┴───────────────┴──────────────────────────────────────┘
```

- **Rail (200 px):** the primary button at the top (the only filled accent element in the window),
  then three flat filters — **All, Unread, Archived** — with counts right-aligned in `meta`, then
  the connection line at the bottom: a 6 px `accent` square and `linked`, or `offline` in
  `text_muted`. No icons for filters; no folder tree. Archived is the only model state with no
  other way in; groups are marked by a glyph in the row, not by a filter.
- **Chat list (360 px):** a 44 px header with the title, a search button (⌕) and an overflow; then
  rows. Rows are separated by **space, not lines** (as Sealwax does), 72 px tall: line 1
  name (`label`) + time (`meta`, right); line 2 preview (`body`/`text_muted`, one line, ellipsis).
  Unread: a **6 px accent square** at x=8 and the name in `text` (read rows use `text_muted` for
  the name); no badge unless the count is > 1, then a 16×16 square `accent` chip with the count in
  `accent_on` at 10 px. Selected: `surface_active` + a **2 px accent bar on the left edge**.
- **Transcript (rest):** a 44 px header (name in `title`, a `meta` status word, icon actions:
  search, overflow); day labels in `meta` uppercase; messages; the composer docked at the bottom.
  The transcript has a **max measure of 72 characters** and is left-aligned, not centred; on very
  wide windows the empty space stays on the right (as Sealwax leaves it).
- **Breakpoints** (tile width, not screen): ≥ 1 100 px three panes; 760–1 100 px rail collapses to a
  48 px icon strip (New, filters as icons, account); < 760 px single pane with a stack (list →
  conversation, Esc back), exactly Sealwax's narrow layout.
- Gaps between panes remain **1 px lines**, and the window fills its tile: no outer margin, no
  rounded window.

---

## 6. Components

### 6.1 Pane headers (44 px)

`title` left, 16 px from the edge; actions right as 28 px icon buttons 4 px apart; a 1 px `border`
line under. The transcript header carries **only** the chat name (`title`) and, for a group, the
member count in `meta`; the connection state lives in the rail (§5), once, never twice as today.
When a `stop_reason` exists — a rare, actionable state such as `keyring locked: unlock it to
connect` — show its client text as a **one-line strip under the header**, full width, in
`warning`; it disappears when the state clears. The rail keeps the short word.

### 6.2 Chat row (72 px)

- Left inset 8 px for the unread marker, avatar at 16 px, text at 60 px, right inset 16 px.
- Line 1 at y=14: name (`label`, `text` if unread else `text_muted`), time (`meta`,
  `text_faint`, right, "21:52" or "Yesterday").
- Line 2 at y=36: preview (`body`, `text_muted`, one line). Ticks for my last message go **before**
  the preview text at `text_faint`, 12 px, not coloured.
- Hover: `surface_hover`. Selected: `surface_active` + 2 px accent bar. No radius, no shadow.
- Group chats: a 12 px `#`-like glyph before the name in `text_faint` (symbolic icon), and the
  preview prefixed with the author's name in `text` when the group is not muted.

### 6.3 Transcript

- **Day label:** `meta` uppercase, `text_faint`, 16 px top / 8 px bottom, 1 px rule to its right
  (the rule is optional; Sealwax uses space only — prefer space).
- **Grouping:** consecutive messages from one author within 5 minutes share one header line
  (author + time at 11 px). A new author or a gap > 5 min starts a new group.
- **Message bubble:** max width **68 ch**; padding **8 px 12 px**; radius **0**; mine:
  `surface_active` with `text`; theirs: `surface_chrome` with a **1 px `border`** and `text`.
  No tails, no shadows, no gradients. Consecutive bubbles stack with 2 px between; groups with
  12 px.
- **Meta line:** time and state in `meta`/`text_faint`, right-aligned under the group's last bubble,
  not inside every bubble. States: `not sent` in `warning`, `queued`/`sending` with the text as-is.
- **Quote/reply:** a 2 px `accent` bar on the left inside the bubble, then the quoted author in
  `label` at 11 px and the quoted text in `text_muted` at 11 px, one line, ellipsis.
- **Reactions:** a flat chip `surface` + 1 px `border`, emoji at 13 px and count in `meta`; placed
  under the bubble, 4 px apart. No pill.
- **Media:** thumbnail box 240×180 (or the picture's ratio, capped), **0 radius**, 1 px `border`;
  a caption line in `meta`; a `[document]` row is a 32 px flat row with a file glyph, name in
  `body`, size in `meta`. Stickers: 160×160, square, no border, transparent background.
- **System rows** (missed calls, encryption notices): centred, `meta`, `text_faint`, no bubble.
- The **empty transcript** state: 32 px glyph, `title` line "No messages yet", one `meta` line with
  the truth ("This chat has no messages" / "Pick a chat to read it here"), nothing else.

### 6.4 Composer

Docked at the bottom of the transcript, full width, separated by a 1 px `border` line.

- Height 48 px for one line, growing to at most 6 lines (then it scrolls).
- Inside: a 28 px icon button (attach) at the left, the entry, and on the right the **Send** button:
  32 px tall, `accent` fill, `accent_on` label at 12 px semibold, **0 radius**, with `Ctrl+Enter`
  in the tooltip. This is the second (and last) filled element in the window; show it **only when
  there is text** — otherwise no button at all, so a screen never has two primaries.
- Focus: the border of the composer becomes `accent` (1 px), nothing glows.
- Reply mode: a 1 px `border` strip above the input with the quoted message in `meta`, and a 28 px
  close button.
- Send states: the button label becomes `Sending…` (disabled) and the bubble shows its state. Never
  a spinner overlay.

### 6.5 Command palette (F1)

Centred panel **560 px**, `surface_chrome`, **1 px `border`**, `surface` scrim at ~70 % over the
whole window (Sealwax dims everything behind), input row 44 px with a 16 px magnifier and an × at
the right, a 1 px line under it, then 36 px rows: command label in `body`, its keys right-aligned
in `meta`/`text_faint`. The first row is `surface_active`. Type to filter; ↑/↓ move; Enter runs;
Esc closes. No categories in the row text (Sealwax appends a muted category; we use groups with
`meta` headers instead, closer to a command list).

### 6.6 Search

Inline, not a dialog: the list header's ⌕ swaps the title for a 32 px input with a 1 px `border`
and `accent` on focus; results replace the list rows; the query syntax hint (`from:`, `is:unread`)
is a `meta` line under the input, shown only while the input is empty and focused.

### 6.7 Settings (Ctrl+,)

A single **480 px** column, not a card grid: section headers in `meta` uppercase with a 1 px rule,
then 40 px rows: label left (`body`), control right (switch, segmented control or value). Switches
are libadwaita's; do not custom-draw them. The "Title bar" row keeps the existing values. Every
row's help text is a second `meta` line under the label, only when it exists (as today).

### 6.8 Dialogs

480 px, `surface`, 1 px `border`, title `title`, body `body`, actions bottom-right: the primary as
a text button in `accent` (not filled) and the destructive one in `danger`. One dialog at a time.

### 6.9 Unread badge (bar widget)

The count in `accent_on` on an `accent` square, 11 px semibold, no radius — consistent with the
list's count chip. Hidden at 0, as today.

---

## 7. Keyboard model

Keep the current bindings and add the missing ones; every action in the palette must also be a
single key where it is frequent.

| Context | Keys |
|---|---|
| Global | `F1` palette, `Ctrl+K` palette, `Ctrl+,` settings, `Ctrl+Q` quit, `/` search, `Ctrl+F` search in the open chat |
| List | `j`/`k` or ↑/↓ move, `Enter` open, `u` toggle unread, `Shift+U` mark all read, `Esc` clear search |
| Transcript | `j`/`k` move, `Enter` open composer, `r` reply, `e` react, `o` open media, `O` open externally, `Esc` back to list, `G` bottom, `g` top |
| Composer | `Enter` send, `Alt+Enter` newline, `Esc` cancel reply, `Ctrl+Enter` send (alias) |
| Palette | type filter, ↑/↓, `Enter`, `Esc` |

Focus is always visible as a 1 px `accent` border or the 2 px selection bar — never a glow, never a
colour change of the whole row.

---

## 8. Accessibility (do not regress what exists)

- Every row and bubble keeps its accessible role and label: list/listitem already asserted by the
  smoke test; a bubble reads "Ana, 10:05: Photo, the beach", a sticker "Sticker".
- Unread is carried by three signals: the dot, the name's colour and the count chip — not colour
  alone.
- All interactive elements are ≥ 28×28 px and reachable by keyboard; focus order follows the panes
  left→right.
- Contrast as in §4.1, enforced by test.
- No information in icons alone: the palette rows carry text, the toolbar buttons carry tooltips
  and accessible names.

---

## 9. The TUI, same system

The TUI cannot use GTK, but it can obey the same rules with ratatui:

1. **No double borders.** A single 1 px line between panes (box-drawing `│`/`─`), and no box around
   the transcript at all: only the header rule and the composer rule.
2. **Same tones:** `surface` for the transcript, `surface_chrome` for the list, `surface_active` for
   the selected row, `accent` for the unread dot (`●`), the selection bar (`▌`) and the one primary
   hint. Nothing else is coloured.
3. **Same type do:** if the terminal font supports bold/underline, use bold for names and subjects,
   dim for meta. No colour-coded senders.
4. **Same density:** one line per message (author + time in dim, then text), a day label in dim
   caps with a blank line, no blank line between messages of the same author.
5. **Same palette overlay:** centred panel with a solid `surface` scrim (no alpha in a terminal),
   36 px rows, keys right-aligned in dim.
6. **Same empty states:** one line of text, dim, plus the key hint.
7. The footer stays one line: keys in dim, protocol and state on the right, nothing else.

---

## 10. Build plan for tomorrow

Ordered so that every phase is shippable and screenshot-verifiable on its own. Rough sizes are for
the Mac session's budget, not a promise.

### P0 — Tokens (≈ 120 lines + tests)

- `crates/nuncio-client/src/theme.rs`: read `lighter_background`, `dark_foreground`,
  `light_foreground`, `bright_foreground` (optional, serde defaults); add the semantic roles of
  §4.1 as methods (`surface()`, `surface_chrome()`, `border()`, `text_muted()`…), computing derived
  tones with a small mix helper.
- Unit tests: the existing `parses_the_real_omarchy_palette` stays; add contrast assertions
  (≥ 7:1 and ≥ 4.5:1) for both a dark and a light palette fixture, and a test that a palette without
  the optional keys still yields usable roles (fallbacks).
- **Risk:** low. The wire and the store are untouched.
- **Verify:** `cargo test -p nuncio-client`; the TUI and desktop still build.

### P1 — Window chrome and layout (≈ 250 lines in `crates/nuncio-desktop/src/window.rs`)

- Replace the current widgets with the three-pane structure of §5: a `gtk::Paned` per seam (so the
  panes are draggable, like Sealwax), 1 px separators, 0 radius.
- Install one `gtk::CssProvider` built from the palette roles (a `format!` of the roles into CSS
  classes: `.pane`, `.row`, `.row:selected`, `.meta`, `.primary`…), and re-install it when the theme
  watcher fires — this is what makes the live theme switch correct.
- Header rows with title + icon buttons; the rail with the one primary button and three filters.
- **Risk:** medium (layout only, but it is the file everything else touches). Keep the controller
  untouched; if a widget needs new state, it goes through the existing `Ui` API.
- **Verify:** the app opens at 1920, 960 and 600 px wide; nothing overlaps; screenshots.

### P2 — Chat list and transcript (≈ 300 lines)

- Row builder per §6.2; bubble builder per §6.3; day labels; grouping; meta line; quote bar;
  reaction chips; media boxes; system rows; the two empty states.
- Keep the existing picture pipeline (`pictures.rs`) and sticker animation; only the frame changes
  (0 radius, 1 px border, 160×160 stickers).
- **Risk:** medium-high (it is where the model meets the view; the shared view models must not
  change semantics). Any change to `nuncio-client::model` needs the TUI kept identical, and that is
  asserted by the existing byte-identical TUI tests.
- **Verify:** the smoke test's role assertions still pass; screenshots with a text message, a photo,
  a document, a sticker, a quote, a reaction and a group chat.

### P3 — Composer (≈ 150 lines)

- Docked bar per §6.4, growing height, send button only with text, reply strip, states.
- **Risk:** low-medium (it owns focus and Enter; the smoke test covers Enter-to-send).
- **Verify:** Enter sends, Alt+Enter newlines, Esc cancels a reply, the height stops at 6 lines.

### P4 — Palette, search, settings, dialogs (≈ 250 lines)

- Palette per §6.5 (reuse the existing action list and `Target`), search per §6.6, settings per
  §6.7, dialogs per §6.8.
- **Risk:** low.
- **Verify:** F1 opens and filters; Enter runs; `/` focuses search; Ctrl+, writes the config 0600;
  a confirmation dialog fits 480 px.

### P5 — TUI parity (≈ 200 lines in `crates/nuncio-tui/src/lib.rs`)

- §9, one pane border instead of boxes, accent only for dot/bar/primary, day labels, dim meta,
  palette scrim, empty states.
- **Risk:** medium (the TUI is the owner's daily tool; keep the keys and the tests).
- **Verify:** the TUI tests, plus screenshots in Ghostty/foot/alacritty at 80×24 and 240×60.

### P6 — Polish and proof (≈ 100 lines)

- Motion, focus rings, hover states, tooltips, accessible names.
- Screenshots at three widths × two themes (dark Gruvbox, a light theme such as Catppuccin Latte),
  a live theme switch, keyboard-only pass, `gsettings` animations off pass.
- Update `crates/nuncio-desktop/README.md` (the design rules in one page) and the screenshots in
  the repository's `docs/` if that is where they live.

**Suggested commit slicing:** one work unit per phase (P0, P1, P2, P3, P4, P5), each with its
screenshots in the PR body; P6 folds into P4/P5 commits if it stays small.

---

## 11. Acceptance checklist (paste into the PR)

- [ ] Three panes at 1920 and 960 px; single-pane stack at 600 px; panes draggable.
- [ ] Zero border radius anywhere; 1 px separators; no shadows.
- [ ] One filled button per screen (New message; Send appears only with text).
- [ ] Row heights 72 px in the list; 44 px headers; composer 48 px growing to 6 lines.
- [ ] Unread: dot + name tone + optional count chip; selected: `surface_active` + 2 px accent bar.
- [ ] Message groups by author and 5 minutes; day labels; meta line with time and state.
- [ ] Quote bar, reaction chip, media box, sticker 160×160, document row, system rows.
- [ ] Empty states for the list, the transcript and the reader.
- [ ] Palette 560 px with scrim; search inline; settings 480 px; dialogs 480 px.
- [ ] Live theme switch repaints everything with no restart; light theme checked.
- [ ] Contrast test passes (≥ 7:1 text, ≥ 4.5:1 muted, ≥ 4.5:1 accent-on-accent).
- [ ] Keyboard-only pass for every action; focus visible.
- [ ] TUI uses one border style, the same accent discipline, the same density.
- [ ] `cargo test --workspace`, `cargo clippy -D warnings`, `cargo fmt --check` green; screenshots
      attached.

---

## 12. Explicit don'ts

- No rounded corners, pills, cards, shadows, gradients, glass, or blurred scrims.
- No colour-coded per-sender avatars beyond the six muted tones; no photos in v1.
- No icon-only primary actions (Send carries a label).
- No second accent colour; `danger` and `warning` appear only for their meaning.
- No animation longer than 120 ms; none when GTK animations are off.
- No hardcoded hex values outside the palette mapping; no font family names.
- No new dependency for layout, styling or icons (GTK CSS, libadwaita widgets, symbolic icons).
- No change to the wire, the store, the ban-risk rules, the notification rules or the media caps.
- No feature added to make the design work; if a design needs a feature, it waits for a decision.

---

## 13. Decisions taken (2026-10-23, with the owner)

All three were decided on a side-by-side mockup built with the measured tokens (panels A and B at
1920×1080, Gruvbox dark), not from prose:

1. **Square bubbles (option A), not a document transcript.** One bubble per author group, max
   68 ch, 0 radius, no tails or shadows; mine on `surface_active`, theirs on `surface_chrome` with a
   1 px border; time and state once under the group's last bubble. It keeps the messenger grammar
   (who speaks is legible at a glance) and takes the reference's restraint. A document layout
   (author line + paragraphs, a 2 px accent bar for mine) was rejected: it scans worse in long
   conversations and stops feeling like WhatsApp, which is the behaviour the owner has asked for
   since T15.
2. **The rail's filters are All / Unread / Archived.** Archived is the only state in the model with
   no other way in today; groups are marked by a glyph in the row instead of taking a slot. Four
   filters were rejected as noise.
3. **The connection state lives in the rail, once;** a `stop_reason` is a one-line `warning` strip
   under the transcript header. This removes today's duplicated "Linked" (it appears twice) and
   keeps the short state always visible, while giving the one actionable message the room it needs
   instead of truncating it into a 200 px rail.

Nothing else in this plan is open: §4–§12 are ready to build as written.
