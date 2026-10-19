# nuncio — M0 (account-free spikes)

Objective: de-risk M0 before committing to M1. Branch `main` (local only, no GitHub remote).
Delivery: work-unit commits. Runner: `go test ./...` and `cargo test --workspace`.
Source of record: `~/Documents/Plans/2026-10-18-whatsapp-client/README.md`, sections
"## Round 3" and "## Round 3b".

## Specs

- **S1** Local repo at `~/Projects/nuncio`, `git init`, layout from round-2 §5 as amended by
  round 3b: **the Go module holds only the daemon**; the **Rust workspace** holds
  `nuncio-client` (shared), `nuncio-tui`, `nuncio-desktop`.
- **S2** **Do not create the GitHub repo.** Conventional commits. **No AI attribution or
  Co-authored-by.**
- **S3** No `sudo`. User-space toolchains are fine. Do not touch other projects.
- **S4** Do **only** the spikes that need no WhatsApp account: **S4** keyring-backed store
  (against a fake/unpaired device store where possible), **S5** GTK theme watcher,
  **S6** D-Bus activation and single instance, **S7** the whatsmeow bump drill (timed),
  **S8** a pure-Go SQLite driver with FTS5, **S9** the ratatui TUI.
- **S5** Also: the daemon skeleton, the NDJSON handshake, the CLI stub, and their tests.
- **S6** **S1–S3 (pairing) need a spare test number from the owner and his explicit go.**
  Prepare everything up to the point of showing a QR or requesting a pairing code, then
  **stop and ask**. Never pair the owner's main number; never display a QR without his go.
- **S7** Tests for each spike's acceptance test, **written first where it makes sense**.
  Record the S7 timing.
- **S8** "The NDJSON wire protocol is defined once as a small schema (JSON Schema or a
  documented spec file) and checked from both sides by **contract tests**: Go serialises,
  Rust deserialises, and the reverse, using shared fixture files."
- **S9** S9 acceptance: "a minimal `nuncio-tui` (ratatui) that connects to the daemon skeleton
  over the socket, does the handshake and renders a fake chat list from fixtures; plus a
  `ratatui-image` check in Ghostty, rendering a local PNG inline. Acceptance: it works at
  80×24 and in Ghostty, and the contract tests pass."
- **S10** "the simplest code that meets each acceptance test; no speculative abstractions or
  frameworks beyond what the plan names; small, reviewable commits."
- **S11** Deliverable: `## OpenCode answer (round 3)` appended to the plan README with each
  spike's result against its acceptance test, the commits, the S7 time, and what is needed
  from the owner to run S1–S3. Then summarise in Spanish and stop.

## Tasks

| ID | Spec | Route | Status | Evidence |
|---|---|---|---|---|
| T1 | S1,S2,S3,S8 | inline | done | repo + `proto/wire.md` + fixtures + workspace skeleton committed |
| T2 | S4,S5,S6,S7 | delegated (Go) | in progress | daemon skeleton, NDJSON, CLI stub, D-Bus, keyring store, SQLite+FTS5, bump drill |
| T3 | S8,S9 | delegated (Rust) | in progress | `nuncio-client` + contract tests + `nuncio-tui` + ratatui-image |
| T4 | S7 | inline | pending | `crates/nuncio-desktop` theme watcher spike |
| T5 | S7,S11 | inline | pending | verification run, S7 timing, round-3 answer |

## Log

- **L1** (owner, 2026-10-18, verbatim): "Empieza a construir siguiendo la sección `## Round 3`
  ... el M0 primero, solo las pruebas que no necesitan cuenta de WhatsApp. Antes de vincular
  cualquier cosa, detente y pregunta. Repo local en `~/Projects/nuncio`, sin crear nada en
  GitHub. Lo más simple que cumpla cada prueba, sin sobreingeniería."
- **L2** (owner, 2026-10-18, verbatim, supersedes the TUI line of round 3): "la TERMINAL va en
  Rust + ratatui (no en Bubble Tea), compartiendo una librería Rust de cliente con la app de
  escritorio GTK. El módulo Go es solo el núcleo." Adds spike **S9**.
- **L3** Repo created, contract (`proto/wire.md` + 7 fixtures) written by the parent because
  both languages depend on it; root `Cargo.toml` (3 members) and `go.mod` fixed as the
  contract so no worker has to invent it.
- **L4** Environment verified: rustc/cargo 1.99.1, go 1.27.2, GTK 4.24.1 + libadwaita 1.10.0
  dev files, session bus up at `/run/user/1000/bus`, `org.freedesktop.secrets` owned by
  gnome-keyring, and `TERM_PROGRAM=ghostty` — so S9's kitty-protocol path is testable here.
