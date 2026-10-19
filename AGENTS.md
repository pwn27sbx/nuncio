# nuncio: rules for code review and agents

nuncio is a native WhatsApp client for Omarchy: a Go daemon (`nunciod`, whatsmeow) owns the one
linked-device session; a ratatui TUI and a GTK 4 app are thin Rust clients over a Unix-socket
NDJSON protocol (`proto/wire.md`). Review every change against these rules.

## Code

- The simplest code that meets the task's acceptance test. No speculative abstractions, no
  frameworks beyond the ones already in `go.mod` and `Cargo.toml`.
- Tests first where meaningful; every test asserts observable behaviour through the public
  interface. Tests never connect to WhatsApp and never touch a real keyring.
- The daemon stays pure Go: no cgo (`CGO_ENABLED=0 go build ./...` must pass).
- Go: `gofmt` clean, `go vet` clean, errors wrapped with context. Rust: `cargo fmt` clean,
  `clippy -D warnings` clean, no `unwrap` outside tests.
- The wire protocol is defined once in `proto/wire.md` with shared fixtures in `proto/fixtures/`;
  a change to a frame updates the spec, the fixtures and both contract suites together.
- Every command, option or message that is added or changed updates its help text and docs.

## Privacy and safety

- No secrets in the repository: no database files, keyring dumps, tokens, session keys.
- No real phone numbers anywhere (code, tests, fixtures, logs, docs). Use obviously fake ones.
- Logs never contain message text, full phone numbers or key material.
- Files holding user data are private: directories `0700`, files `0600`.
- Nothing links, unlinks or logs out a device without explicit consent (`--consent` or an
  explicit confirmation flag), and nothing happens before that gate.

## Ban-risk rules (WhatsApp)

- No automation surface: the CLI sends exactly one message to one recipient per invocation; no
  batch, stdin loops or scripted sends.
- No reconnect storms: capped exponential backoff with jitter, and no reconnect at all after a
  logout or ban response.
- Never send "available" presence on connect; read receipts only on an explicit view.
- No silent background retry loops; failures stay visible.

## Commits

- Small Conventional Commits (`type(scope): subject`).
- No AI attribution and no `Co-Authored-By` trailer, ever.

## Review verdict

STATUS: FAILED only if a new or changed line in this commit breaks a rule in this file. Problems
that already existed in the file, style suggestions or optional improvements go as observations
under STATUS: PASSED, in a list titled "Observations". Cite the broken rule in every violation.
