# nuncio M3: desktop app (office part)

Objective: start M3 on the work Mac — first the logic the TUI and the GTK app share, then the GTK UI where the Mac allows it.
Branch: `feat/m3-desktop` (from `feat/m2-tui` at `f6af712`). Delivery: `single-pr`. Runners: `cargo test --workspace --exclude nuncio-desktop`, `TMPDIR=/tmp go test ./...`.

## Specs

- S1 M3 (original plan, handoff 7b, verbatim): "**Deliverables:** gtk4 + libadwaita UI (list + transcript + composer), command palette (F1), plain file config, background mode, notifications whose `default` action opens the right conversation, `whatsapp:`-style link handling, accessibility roles on custom bubble widgets." "**Acceptance:** Orca reads the chat list and transcript; a live theme switch leaves no stale colours; a notification click focuses the correct conversation from a cold start; cold start < 1.5 s; no title bar under Hyprland; stable memory over a day." "**Mac:** shared-client logic only. UI work and all acceptance on Linux/Hyprland."
- S2 Home plan, Part 3 (verbatim): "Mac-safe part first, in `nuncio-client`: the plain-file config, `whatsapp:` link parsing, the notification-target logic, and the view models shared with the TUI (chat list, transcript, composer state). Tests in Rust." "The GTK UI can be **developed** on the Mac with Homebrew `gtk4` and `libadwaita` (unverified cask names; check with `brew info`). Its **acceptance is Linux-only**" "Sealwax (MIT, `PixDevsApps/sealwax`) is the reference for layout, theme following and polish: read its structure, copy the design, and reuse code only if the licences allow."
- S3 Rules unchanged: tests first with RED, GGA per commit, independent verifier for high-risk changes, no AI attribution, no push without the owner's explicit OK naming remote and branch, no real-account actions from the Mac.
- S4 Assumptions (orchestrator, to confirm with the owner when they matter): a `whatsapp:` / `wa.me` link only opens the chat and pre-fills the composer, it never sends (ban-risk rule: one message per explicit user action); the config file is plain TOML under `$XDG_CONFIG_HOME/nuncio/config.toml` with every key optional and documented.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| D1 | S2 shared logic: config, links, notification target | delegated writer (context backstop) | done: RED per rule (29 client + 2 TUI), 17 mutants killed; no new dependency (toml 0.9 already used); parent reran fmt/clippy/cargo test --no-fail-fast | this commit |
| D2 | S2 shared view models (chat list, transcript, composer) | after D1; scope to agree with the owner (refactor of the TUI model) | open | |
| D3 | S1/S2 GTK UI slices on the Mac (needs Homebrew gtk4/libadwaita — ask first) | later | open | |

## Log

- L1 (2026-10-20, owner): "si" to pushing f6af712 and starting M3 with the logic shared by the TUI and the desktop app. Pushed `origin/feat/m2-tui` = f6af712.
- L2 D1 decisions to confirm with the owner: a bounded `notified` set (last 1024 messages) so a reaction that republishes a message does not notify again; link text over 65536 chars is an error, never cut; `chat_title` now exists in nuncio-client and nuncio-tui (dedupe in D2); a config file with an unknown key or wrong type stops the TUI with exit 2.
