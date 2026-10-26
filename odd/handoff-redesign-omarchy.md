# nuncio: the UI/UX redesign — report and checks for the home Omarchy machine

> **Read this first.**
>
> This file is the whole context a fresh Claude Code session on the owner's home Arch Linux /
> Omarchy (Hyprland) machine needs to check the redesign. It says what was built, why each choice
> was made, how it was verified on the work Mac, what never ran, and the exact steps to check it
> all on screen. Written on 2026-10-23 on the work Mac. The spec and the full logbook are
> `odd/tasks/nuncio-redesign.md` (S1–S8, L1–L25); the design is `docs/design/redesign-plan.md`
> (the plan the Omarchy session wrote on 2026-10-23, Sealwax as reference).
>
> **State in ten lines.**
>
> 1. Every phase of the plan is merged into `main`: ten PRs, #4 to #13, last merge `5eff448`.
> 2. All six CI checks were green on every PR (two needed one rerun of a known flaky timing test).
> 3. Each risky phase had an independent read-only verifier; no blocker is open.
> 4. **No window of the redesigned app has ever been shown on a screen.** The Mac never opens
>    windows (owner rule) and CI has no display, so the GTK smoke test was skipped everywhere.
> 5. So the first thing to run here is the smoke test (§4.1). Everything visual is §4.
> 6. Nothing touches WhatsApp: every check below runs against the demo daemon (`nuncio-demo`).
> 7. Corners follow Hyprland's `decoration:rounding`, which is 0 here: nothing may look rounded.
> 8. Three things are the owner's to judge by eye (§5): the 72-character transcript column, the
>    contrast follow-ups F1–F2, and the open choices F3–F8.
> 9. The only code that changed behaviour beyond the look: one draft per chat (desktop **and**
>    TUI), New message, the chat-name filter. The wire, the store and the ban-risk rules did not.
> 10. Report back as `nuncio-redesign-report-<date>.md` in the vault (§6).

---

## 1. What was built, phase by phase

| Task | Plan phase | PR | Merge | What it does |
|---|---|---|---|---|
| R0 | P0 tokens | #4 | `e0c137a` | Semantic palette roles (`surface`, `surface_chrome`, `surface_active`, `text`, `text_muted`, `accent`, `accent_on`, `warning`, …) computed from the Omarchy palette in `crates/nuncio-client/src/theme/roles.rs`, with contrast tests on real Omarchy themes. |
| R1 | P1 window and layout | #5 | `264d1cd` | Three panes (rail, chat list, conversation) with draggable seams, breakpoints at 1099 and 759 px, 44 px headers, the stop strip, the All / Unread / Archived filters. All CSS is generated from the roles in one provider that the theme watcher reloads. |
| R2 | P2 chat list and transcript | #6 | `cf72523` | Chat rows per §6.2; square bubbles grouped by author within 300 s; day labels; the meta line only under a group's last bubble. One `ListBoxRow` per message. |
| R3 | P3 composer | #7 | `f374609` | A 48 px docked composer that grows to 6 lines and then scrolls; Send shows only with text and reads `Sending…` while a send waits; the reply strip; the top line turns accent on focus. |
| R4 | P4 palette, search, settings | #8 | `21d1e97` | The command palette (560 px popover with a scrim), message search restyled inside the conversation, settings rows per §6.7. Keys `/`, j/k, g/G. |
| R5 | P5 TUI parity | #9 | `02e0060` | The TUI draws its panes with single rules and the same palette roles; `not sent` in the warning colour. |
| R7 | New message | #10 | `bac4247` | The rail's primary button asks for a phone number and opens that chat, through the same path as a `whatsapp://send?phone=…` link. It never sends. |
| R9 | One draft per chat | #11 | `11bcf65` | Each chat keeps its own draft and its own reply in progress, in both apps. |
| R8 | Chat-name filter | #12 | `5fd2d81` | The chat list header's ⌕ swaps the title for a filter: typing keeps the chats whose shown name holds the text, ignoring case. |
| R6 | P6 polish | #13 | `5eff448` | 120 ms hover and scrim fades only (all off with animations off), 1 px accent focus rings, 28 px header icon buttons, the transcript at most 72 characters wide on the left, the README "Design rules" page and its "On Omarchy" checklist. |

---

## 2. Why: the decisions and their reasons

### 2.1 The owner's decisions (verbatim where quoted)

| # | Decision | Reason |
|---|---|---|
| S1 | The plan is the spec: "Nothing else in this plan is open: §4–§12 are ready to build as written." | The plan came from this machine, after looking at the app. |
| S2 | "las esquinas se toman de la config que manda hyperland en este caso yo las tengo en 0". Every radius is `hyprctl -j getoption decoration:rounding`; 0 without Hyprland. libadwaita's own window corners must not round. | The home run (T4) saw rounded window corners on a 0-rounding desktop. |
| — | Delivery "A": one PR per phase against `main`. | Each phase reviewable and revertible on its own. |
| L6 | "todo esto lo dejamos como followups para verlo bien en mi omarchy cuando pruebe los temas y lo vea con mis ojos". | The plan's contrast targets fail on real palettes (F1, F2); judged by eye, not by the Mac. |
| S6 | New message: "B" (build it, instead of a `Commands` button or no button). | The rail needed a real primary action; the link path already existed and never sends. |
| S7 | Search placement: "si la C". Message search stays in the conversation; the list header gets a name filter. | The only search the daemon has is per chat; a list-level search box would have lied about its scope. |
| S8 | Drafts: "si" to an empty composer for a new number, plus one draft per chat. | A single global draft let a double Enter send Ana's text to the new number (found by the R7 verifier). |
| L7 | "tu haz el trabajo no neceistas mi permiso a menos que sea sobre decisiones". | The Mac session pushed, opened and merged each PR on green without asking. |
| L21 | No windows, screenshots or Spaces changes on the Mac. | They interrupted the owner's work; every visual check therefore moved here. |

### 2.2 Choices the Mac session made where the plan was silent or contradictory

| Where | Choice | Why |
|---|---|---|
| Send tooltip (S3) | It names `Enter`, not `Ctrl+Enter`. | Plan §6.4 contradicts §7; §7 (Enter sends, Alt+Enter newline, Ctrl+Enter alias) wins. |
| Muted text (S4) | When a real palette gives `text_muted` under 4.5:1 on a surface, the role moves toward `text` until it reaches 4.5:1; `text` never changes. | Keeps secondary text readable on every theme without altering the theme's own text colour. |
| No feature outside the design (S5) | Not built: system rows, read ticks, group member count, an attach button, `from:`/`is:unread` search, `u`/Shift+U. | None of them has data or an action in the model or the wire; the plan forbids wire changes (§12). |
| TUI drafts (L20) | One draft per chat holds in the TUI too. | The model is shared. Before, j/k carried Ada's draft to Grace, so Enter could send it to the wrong chat. A TUI-only exception would have kept that risk. |
| Reply in progress (L20) | Stays with its chat on a switch, instead of being cancelled. | Same rule as the draft. |
| Name filter (L22) | Substring match on the shown name, ignoring case; the rail counts ignore it; `/` still opens message search. | The plan says nothing on any of the three. |
| Transcript measure (L24) | Bubbles end at the 72-character column on a wide window. | The plan read literally (§5). Judge it (§5, J1). |
| Composer height (L24) | Not animated. | §4.4 allows it; it is not a CSS transition and needs its own animation driver. |
| Focus after Escape closes the message search | Still goes nowhere, as before. | The plan does not say where it should go. |
| Photo box (F4) | Stays 300 px, not the plan's 240×180. | Changing it changes the decode targets of every photo. |

### 2.3 How it was verified on the Mac

- Test first, one RED test per rule through the public interfaces; mutation checks on every rule
  (for example 8 mutants on the name filter, 12 on the polish CSS, all caught).
- An independent read-only verifier in a scratch copy for each risky phase, with a differential
  against the previous commit. Blockers it found and that were fixed: the open chat leaking into
  every filter and count, the selection lost after a row rebuild, a rounded window without
  Hyprland, the quote drawn after the text, and the stale draft sent by a double Enter.
- `cargo fmt --check`, `clippy -D warnings`, the whole workspace suite, and the six CI jobs.
- **What never ran anywhere:** the GTK window itself. The smoke test (`crates/nuncio-desktop/tests/smoke.rs`)
  prints `smoke: skipped` without a display, and it has new assertions from R7, R8, R9 and R6.
  The window wiring of R8 and R6 (filter Escape and focus, the scrim, the `Measure` widget, the
  live animations setting) was checked only by reading the code.
- One fix in R6 nobody executed: the transcript list box now takes the scroll's adjustment
  explicitly (`window.rs`, after `transcript_scroll` is built), because the new `Measure` widget
  is not a scrollable parent; without it PageUp/PageDown would stop paging. Check it (§4.6).

---

## 3. Setup on this machine

Facts from the home run (2026-10-22): rustup lives in `~/.cargo/bin`, which is **not on `PATH`**;
GTK 4.24.1, libadwaita 1.10.0; the window class is `io.github.pwn27sbx.nuncio.Desktop`; the user
has `~/.config/gtk-4.0/gtk.css` and `hyprchroma.css`, which override the app's CSS (F3).

```sh
cd ~/<where the nuncio checkout is>        # the one used for the home run
git switch main && git pull --ff-only      # head at or after 5eff448
export PATH=$HOME/.cargo/bin:$PATH
hyprctl -j getoption decoration:rounding   # expect "int": 0
```

The demo daemon takes the **real** runtime directory (nothing reaches WhatsApp; a text starting
with `fail` ends `not sent`, so Retry can be tried; it answers each send with an invented reply).
Corrected 2026-10-26 after the home check: the first version of this file pointed both at
`XDG_RUNTIME_DIR=/tmp/nuncio-demo`, which hides the Wayland socket, so the app fell back to
XWayland (window class `nuncio-desktop`, no Hyprland rounding read). In this order:

```sh
systemctl --user stop nunciod      # start it again when done
go run ./cmd/nuncio-demo           # terminal 1, first: an app started alone activates the real nunciod
cargo run -p nuncio-desktop        # terminal 2, once the demo is listening
```

Rules while checking: never send from the real account; never copy `~/.local/share/nuncio`; put
fake numbers only in anything written down (`+1 555 …`); close the demo, the app and every
terminal or server you start when done.

---

## 4. The checks, in order

Each check says what to do and what must be seen. Write the result of every one in the report
(§6), with a screenshot where it says so.

Screenshots: with the app focused,

```sh
hyprctl eval 'hl.dispatch(hl.dsp.window.float({ action = "set" }))'
hyprctl eval 'hl.dispatch(hl.dsp.window.resize({ x = 1920, y = 1000, relative = false }))'   # then 960, then 600
grim -g "$(hyprctl -j activewindow | jq -r '"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')" ~/nuncio-shot-1920-gruvbox.png
```

(If `grim` or `jq` are missing, say so in the report and use Omarchy's own screenshot key.)

### 4.1 The smoke test (first)

```sh
cargo test -p nuncio-desktop --test smoke
```

Expected: `smoke: ok`. It opens the real window for a moment; that is fine here. If it fails,
stop, copy the failing assertion into the report, and do not judge the look until it is
understood: the smoke test is the only proof that the window wiring of R7, R8, R9 and R6 works.
Also run the whole suite once: `cargo test --workspace` (expect all green; the theme 500 ms and
font tests are timing-sensitive under load, rerun them alone once before calling them failed).

### 4.2 Layout, corners, theme (P1, P2)

1. Screenshots at 1920, 960 and 600 px wide, in **Gruvbox** and in **Catppuccin Latte** (six in
   all). At 1920 three panes; under 1100 the rail becomes an icon strip; under 760 one pane at a
   time (open a chat, go back with Escape and with the back button).
2. Drag both seams.
3. With rounding 0, nothing is rounded: window, buttons, rows, bubbles, badge, popovers, toasts,
   settings, switches. If anything is, check F3 first.
4. Headers 44 px with a 1 px line; the active filter on `surface_active` with a 2 px accent bar.
5. Live theme switch with the app open (`omarchy theme set <name>`, as the home run did; the
   README writes `omarchy-theme-set`; use whichever exists and note which): everything repaints,
   no restart. Switch back.
6. Chat rows: unread marks, the selected bar, group rows, dates. Bubbles: grouping, day labels,
   the meta line only under the last bubble; a `fail…` send shows `not sent` in `warning` and
   Retry (F6).
7. Quotes, reactions, the photo frame (F4), stickers, a document row, the empty states.
8. The header no longer shows "linked · N unread": the rail counts chats. Judge whether it is missed.

### 4.3 The composer (P3)

1. Type several lines: the 48 px bar grows to 6 lines, then scrolls.
2. The top line turns accent while the composer has the focus.
3. Send is hidden with an empty draft; Enter sends, Alt+Enter and Shift+Enter add a newline;
   the Send tooltip says Enter. While a send waits it reads `Sending…` (the demo answers fast; it
   may not be visible, say so).
4. Reply (`r` on a message): the reply strip and its close button.

### 4.4 Palette, search, settings (P4)

1. F1 or Ctrl+K: the palette is centred over a darkened scrim; type to filter, arrows, Enter runs,
   Escape closes. **After closing it, click the chat list and the conversation: they must respond**
   (the scrim is kept at opacity 0 now; it must never eat clicks).
2. Message search: the conversation header's ⌕ or Ctrl+F or `/`; the row sits under the header,
   the hint shows only while empty and focused, the border turns accent on focus.
3. Ctrl+,: settings rows 40 px with subtitles, not clipped; switches square; a change writes
   `~/.config/nuncio/config.toml` (check `stat -c %a` gives 600 and its directory 700).

### 4.5 New message, drafts, the name filter (R7, R9, R8)

1. Rail button "New message": the only filled accent element; the dialog is 480 px, square,
   1 px border, no shadow. A bad number (`12`) shows the error in `warning`, which clears when you
   type; the error never repeats the number.
2. Type a draft in chat A. New message to a **new** fake number (`15550000001`): the composer is
   empty; press Enter twice: **nothing is sent**.
3. Type in A, switch to B, type something else, come back to A: each chat shows its own text and
   its own reply in progress. New message to A's number: A's own draft shows.
4. The list header ⌕: type part of a name, ignoring case: the list narrows; a text that matches
   nothing says "No matching chats"; Escape clears it, the list and the selection come back, and
   the focus is in the chat list. With the filter and message search both open, the first Escape
   in the list closes the filter, the second the search.
5. While the open chat is filtered out of the list, its conversation and composer still work.
6. TUI (`cargo run -p nuncio-tui`, against the same demo): type in one chat, move with
   j/k to another: its composer does not show the first chat's text; come back: it is there.

### 4.6 Polish (P6)

1. Keyboard only: Tab moves between list, conversation and composer; every action reachable;
   the focus ring (1 px accent) always visible, outside the fill on New message and Send.
2. Hover a row and a header button: a quick (120 ms) fill. The header icon buttons are 28 px.
3. Animations off: `gsettings set org.gnome.desktop.interface enable-animations false` with the
   app open: no hover fade, no scrim fade, stickers still. Back to `true`: they return without a
   restart. (The `settings.ini` key no longer reaches GTK 4.24; use gsettings.)
4. The transcript: on a 1920 px window it stays at most 72 characters wide, left-aligned, and
   bubbles end at that column (judge, J1); on a narrow window it fills the width.
5. **PageUp / PageDown** in a long conversation page through it; j/k and g/G still move and keep
   the focused message in view.

### 4.7 The TUI look (P5)

In Ghostty, foot and alacritty, at 80×24 and at 240×60, on Gruvbox and on a light theme: single
rules and their joins; contrast between chrome, surface and active rows; dim and bold visible;
accent only on `●`, `▌` and the primary key; pictures (kitty, sixel, half blocks) inside their
box; `not sent` in the warning colour.

---

## 5. What the owner judges by eye (each needs a yes/no or a choice)

| ID | Question | Mac's proposal |
|---|---|---|
| J1 | Bubbles end at the 72-character column on a wide window. Keep? | Keep (plan §5); if it looks cramped, let bubbles use the full width and keep only the text measure. |
| F1 | `text` on `surface_chrome` is 6.42:1 on Gruvbox, not the plan's 7:1. | 7:1 on `surface`, 4.5:1 (WCAG AA) on `surface_chrome`; then un-ignore the test with the new target. |
| F2 | `accent_on` is under 4.5:1 on Latte (4.34) and Rose Pine (3.14). | When neither surface nor text reaches 4.5:1 on the accent, use black or white, whichever contrasts more; the accent never changes. |
| F3 | Anything still rounded? | Check `~/.config/gtk-4.0/gtk.css` and `hyprchroma.css`: a user `gtk.css` (priority 800) overrides the app (600). |
| F4 | Photo box 300 px vs the plan's 240×180. | Judge both on screen; changing it changes decode targets. |
| F5 | An animated sticker restarts when a new message joins its group. | Noticeable or not. |
| F6 | The failed bubble has no red fill (text, `warning` meta line and Retry carry it). | Does it still read as failed? |
| F7 | Not built: system rows, read ticks, group member count. | They need model data; later, as their own feature. |
| F8 | Composer: no attach button; Shift+Enter kept besides Alt+Enter; when `Sending…` shows. | Keep all three. |
| — | The header lost "linked · N unread". | Keep it gone unless missed. |
| — | Focus after Escape closes the message search goes nowhere. | Send it back to the conversation, if the owner agrees. |

---

## 6. Report back

Write `~/Documents/ObsidianVault/Comunicaciones - Claudes/nuncio-redesign-report-<YYYY-MM-DD>.md`
with: the machine state (head commit, theme, rounding); §4.1 output; every check of §4 as
pass / fail / not possible, with the screenshot paths; each failure with exact steps and what was
seen; the owner's answers to §5, quoted; and anything this file got wrong. No phone numbers, no
message text from the real account, nothing from `~/.local/share/nuncio`.

If a check fails and the cause is clear, fix it on a `fix/redesign-*` branch from `main` with a
test first, one Conventional Commit per fix, no AI attribution, and open a PR against `main`;
leave product choices (§5) to the owner.
