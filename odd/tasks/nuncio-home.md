# nuncio: the home run (2026-10-22, Omarchy machine)

Home run of the checklist in `nuncio-handoff-omarchy.md`, on `feat/m4-omarchy` (base `main`
`ae35fea`). Toolchain here: Go 1.27.2, Rust 1.99.1 through rustup with `RUSTUP_TOOLCHAIN=stable`
(the owner approved skipping the pinned-toolchain download; stable *is* 1.99.1), `TMPDIR=/tmp`.
Runner: `cargo test --workspace --no-fail-fast`, `go test -race ./...`.

## Specs

- **S1.** Handoff 6.3, pass criterion, verbatim: "Pass: every command exits 0; the `-v` D-Bus run
  shows `--- PASS` and **no `--- SKIP`**".
- **S2.** Handoff T10.2, verbatim: "`cargo test -p nuncio-desktop --test smoke -- --nocapture` under
  Hyprland: runs, does not print `smoke: skipped, GTK cannot start here`."
- **S3.** Handoff T7, row verbatim: "GFile URI normalisation of D-Bus `Open` | T4.7" — the desktop
  smoke test must hold on Linux, where GLib's normalisation has never been observed.
- **S4.** Handoff 6.16, verbatim: "Every failure found at home goes into a feature document before
  it is fixed".

## Tasks

| ID | Specs | What | Route | Status |
|---|---|---|---|---|
| T1 | S3 | Assert that the link's **request** survives `GFile`'s URI normalisation, instead of the exact bytes | inline | done |
| T2 | S1, S2 | Re-run the smoke test and the Rust workspace suite on Linux with a display | inline | done |
| T3 | S4 | Commit and push to `origin/feat/m4-omarchy` after a green suite (standing OK L13) | inline | done |

## Log

- **L1.** The owner's request, verbatim (relayed from the work Mac): "Task for tonight (deadline
  midnight, about 4 hours; the owner is present with both phones): run the whole nuncio home
  procedure, parts A and B. The only source of truth is this handoff from the work Mac".
- **L2.** 6.3 on Linux, first run: `cargo test --workspace --no-fail-fast` exits 101. The only
  failing target is `-p nuncio-desktop --test smoke`:
  `smoke: FAIL: the open handler gets the link as given: whatsapp://send?phone=15551230002&text=hola%20Ben`.
  Every other target passed (159, 13, 25, 10, 8, 123, 7, 11, 55, 29, …). The same commit is green
  in CI, where the smoke test skips for lack of a display: this is the first run with a display.
- **L3.** Diagnosis, not a product defect. `gtk::gio::File::for_commandline_arg(link).uri()` returns
  `whatsapp://send/?phone=15551230002&text=hola%20Ben`: GLib inserts the empty path before the
  query. Reproduced outside the test with PyGObject on this machine. `parse_link`
  (`crates/nuncio-client/src/link.rs`) documents and tests both `whatsapp://send?…` and
  `whatsapp://send/?…`, so the app receives a form it already handles; `https://wa.me/…` comes back
  byte-identical. The assertion encoded an unverified assumption about GLib (T7's row), so the test
  changes, not the code.
- **L4.** T1: the loop now compares `parse_link(link)` with `parse_link(normalised)` and requires
  the link to be a valid request; the behavioural checks below it now feed `ui.open_link` the
  normalised URI, which is what the app is actually handed. RED observed in L2.
- **L5.** T2: `cargo test -p nuncio-desktop --test smoke` passes with a display (no
  `smoke: skipped`), and `cargo test --workspace --no-fail-fast` exits 0.
- **L6.** Home run, what was executed and what was not, by the owner's decision at 21:52 (option
  "B", T4 before everything else): executed 6.1–6.5, T1, T6, T2.1, B1, B2, B3 and Q2, plus the T4
  subset (window, palette, settings, cold start). Not run tonight: T3 (the whole terminal matrix),
  P2c (`queued`, it needs a ~5 minute outage, and a host outage also takes this session offline),
  G1 group senders (no group available), T5, the rest of T8, T9 and T10. T2.11 relink and T5.9
  uninstall were skipped by design.
- **L7.** Observation from B3, open follow-up, not a blocker. With the daemon stopped, a send made
  in the gap shows `not sent` (correct). Pressing `R` in the TUI did not fire the retry in three
  attempts, while `nuncio retry <client_id>` on the same state worked
  (`outbox.state` `not_confirmed` → `sent`, messages 14 → 15). The client's `retry_selected`
  (`crates/nuncio-client/src/model/mod.rs:616`) needs the selected message to be
  `MessageState::NotSent` with a `client_id`; the row the user sees is the daemon's `not_confirmed`
  one, so the key does nothing and says nothing. Worth deciding: allow retrying `not_confirmed`, or
  tell the user why it cannot.
- **L8.** Observations, none of them a rule violation. `namcap` reports only `W:` lines (FULL RELRO
  on the two Go binaries, implicitly-satisfied dependencies, `gcc-libs` possibly unneeded) and no
  `E:`; `makepkg` printed `libfakeroot internal error: payload not recognized!` while stripping and
  still produced the package (exit 0); the first `nuncio-desktop` start measured 1724 ms (cold disk
  cache, right after the release build) against 353 ms on a warm cache, both under the 1.5 s target
  once warm; RSS baseline 125–131 MiB, the 24 h check is part of the T19 soak.
- **L9.** T5.8 first run on Linux: the bar widget failed to load —
  `Plugin widget pwn27sbx.nuncio-unread failed: Cannot assign to non-existent property
  "labelText"`. Omarchy 4.0.6's `BarIconButton` (`/usr/share/omarchy/shell/Ui/BarIconButton.qml`)
  draws `text` and inherits `WidgetButton`, which has no `labelText`; the widget was written against
  an API that does not exist here. Fixed by drawing the count in the button's only text slot
  (`unread > 99 ? "99+" : String(unread)`), which is what the badge needs, and the widget is hidden
  at 0 anyway. `qmllint` went from 12 warnings to **0** once the import path was right: the
  documented `-I "$OMARCHY_PATH/shell"` cannot resolve `qs.*` here, because the modules under
  `/usr/share/omarchy/shell/{Commons,Ui}` declare `module qs.*`; a scratch dir with `qs ->
  /usr/share/omarchy/shell` resolves them. Quickshell caches the failed compile, so the live check
  needed `omarchy-restart-shell`; after it, no widget error appears in `qs log`.
- **L10.** T5.7 first run on Linux: copying `contrib/omarchy/hypr/nuncio.lua` made Hyprland show
  `require("hypr.nuncio"): …/nuncio.lua:27: attempt to call a nil value (field 'rebind')`. Omarchy
  4.0.6's helper table (`/usr/share/omarchy/default/hypr/helpers.lua`) has `o.bind` and no
  `o.rebind`, so the hotkey never loaded and the old WhatsApp web-app bind stayed. Fixed by using
  `o.bind` with the same `{launch, focus}` table (`command_from` expands it to
  `omarchy-launch-or-focus '<class>' 'uwsm-app -- nuncio-desktop'`), preceded by a guarded
  `hl.unbind` so the key is not bound twice. After the reload: `hyprctl configerrors` empty and
  exactly one bind on SUPER+SHIFT+ALT+G, described "WhatsApp (nuncio)".
- **L11.** T9.2 on Linux: a `~/.config/gtk-4.0/settings.ini` holding `[Settings]
  gtk-enable-animations=0` did **not** stop an animated sticker, even after restarting the app. GTK
  4.24 no longer routes that file into `GtkSettings`; the key is GSettings, and
  `gsettings set org.gnome.desktop.interface enable-animations false` (then a restart) does keep the
  sticker on its first frame, as the code expects. The code was right; the handoff's instruction for
  turning animations off was not. Recorded in `crates/nuncio-desktop/README.md`. Q3 and G1 also
  passed: the queued message reached the contact once, and a real group shows its message with the
  author's name, not digits.
