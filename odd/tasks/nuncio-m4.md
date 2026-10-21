# nuncio M4: Omarchy glue and packaging (office preparation)

Objective: write on the work Mac everything M4 needs that does not require Linux to author; all testing and acceptance happen at home.
Branch: `feat/m4-omarchy` (from `feat/m3-desktop` at `018d66a`). Delivery: `single-pr`. Runners: `cargo test --workspace --no-fail-fast`, `TMPDIR=/tmp go test ./...`, plus static checks for packaging files.

## Specs

- S1 M4 (original plan, handoff 7b, verbatim): "**Deliverables:** theme/font watcher integration, Hyprland rounding, a `hyprland.lua` binding snippet, Quickshell `bar-widget` plugin reading the D-Bus `UnreadCount`, systemd `--user` unit + XDG autostart, `packaging/PKGBUILD.in` (Sealwax-style: `cargo fetch --locked`, offline release build, `!debug !lto`), GitHub Releases `.pkg.tar.zst` + one-line `sudo pacman -U` installer, docs (README, SECURITY, INTEGRATIONS, CREDITS). AUR registrations were closed as of 2026-10-15, so GitHub Releases is the realistic channel." "**Acceptance:** installs on a clean VM from the release artifact with one documented command; the bar widget shows and clears unread; the hotkey launches-or-focuses; uninstall removes the unit, widget and files." "**Mac:** nothing. Linux only."
- S2 Home plan, Part 4 (verbatim): "Can be written on the Mac: `packaging/PKGBUILD.in`; the systemd `--user` unit and XDG autostart; the `hyprland.lua` binding snippet; the Quickshell bar-widget plugin skeleton reading D-Bus `UnreadCount`; the docs (README, SECURITY, INTEGRATIONS, CREDITS). **Testing is home-only.** **ASK** before CI (Q14): delivery/CI is high risk." "the theme/font watcher (Omarchy `~/.local/state/omarchy/current/theme/colors.toml` + font); Hyprland rounding (`hyprctl -j getoption decoration:rounding`) and no title bar" "**Publishing a release or creating anything on GitHub needs the owner's explicit OK.**"
- S3 Rules unchanged: tests first with RED where code is involved, GGA per commit, independent verifier for high-risk changes (packaging/install is delivery: item 5), no AI attribution, no push without the owner's explicit OK naming remote and branch, no GitHub release or CI without the owner's OK, no real-account actions from the Mac, never delete files you did not create.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| K1 | S1/S2 packaging: PKGBUILD.in, systemd --user unit for nunciod, XDG autostart, .desktop files (x-scheme-handler/whatsapp, DBusActivatable + D-Bus service), install/uninstall manifest, installer script draft (no publishing) | delegated writer (parallel); Risk 5 → verifier | done: check.sh 54 ok (MANIFEST ↔ package() 15 paths, required options, offline build, name agreement with the code); verifier: installer refuses hostile versions/no TTY/non-Arch, sha256 checked; B1 (wipe could not remove the self-bind-mounted data dir under ProtectSystem=strict → logout/purge always failed) fixed in internal/store and rechecked FIXED | (see commits) |
| K2 | S1/S2 Hyprland binding snippet (launch-or-focus) + Quickshell bar-widget skeleton over D-Bus UnreadCount | delegated writer (parallel) | done: Omarchy 4 Lua binding (o.rebind, anchored class regex), Quickshell plugin manifest + BarWidget.qml (busctl get-property --auto-start=no, monitor with backoff, 30 s poll until T18); luajit, jq, Omarchy validator pass | (see commits) |
| K3 | S2 desktop app: font from Omarchy, Hyprland rounding via hyprctl (behind fakes) | delegated writer (parallel) | done: font via fc-match monospace (Omarchy's own mechanism) followed live, rounding via hyprctl only under Hyprland, 200 ms bounds; app id renamed to io.github.pwn27sbx.nuncio.Desktop (it collided with the daemon's bus name); --gapplication-* passed through; watcher threads never spin; 9+ mutants killed; cargo 420/0 | (see commits) |
| K4 | S1/S2 docs: README, SECURITY, INTEGRATIONS, CREDITS | after K1–K3 | open | |

## Log

- L1 (2026-10-21, owner): "ssi" to pushing feat/m3-desktop and starting the M4 preparation. Pushed `origin/feat/m3-desktop` = 018d66a. CI not started (needs the owner's OK).
- L2 Found while writing K1/K2: the desktop app's GApplication id equalled the daemon's D-Bus name (io.github.pwn27sbx.nuncio) — they would fight for one bus name on Linux. Decision (orchestrator): rename the app to io.github.pwn27sbx.nuncio.Desktop; the daemon keeps its name (existing contract).
- L3 Verifier advisories left: StartLimitBurst=4 counts manual/D-Bus starts (documented reset-failed); uninstall removes the unit and files but the Omarchy widget/binding are removed by hand (S1 says "uninstall removes the unit, widget and files" — owner to decide); the bar badge only clears after T18; Wipe accepts EPERM/EACCES besides EBUSY/EROFS (only on an emptied, readable dir; narrowing optional); no app icon yet; install.sh curl lacks --proto-redir; a theme read that panicked inside notify's FSEvents callback aborted the process on macOS — the read now runs on its own thread.
- L4 Final checks: cargo fmt/clippy clean, cargo test --workspace 420/0 + smoke, gofmt/vet clean, go test all ok, packaging/check.sh all passed.
