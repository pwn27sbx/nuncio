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
