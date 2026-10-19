# Credits

nuncio is GPL-3.0-only (see `LICENSE`). This file names the projects that shaped
its design and the third-party code it is built on.

## Projects that shaped the design

- **nchat** by d99kris: a terminal chat client whose WhatsApp support set
  nuncio's reference points for the feature checklist, the media-export
  behaviour and the dependency update discipline. nuncio does not copy nchat's
  patched whatsmeow files; the media behaviour is re-implemented independently
  of upstream in `internal/media`.
- **Sealwax**: the reference GTK 4 + libadwaita stack for the workspace layout,
  Omarchy theme following and PKGBUILD packaging patterns.

## WhatsApp and Signal protocol stack

- **whatsmeow** (`go.mau.fi/whatsmeow`) by tulir (mautrix): the Go library for
  the WhatsApp web multi-device API. Licensed **MPL-2.0**; its files stay under
  MPL-2.0.
- **libsignal** (`go.mau.fi/libsignal`), whatsmeow's Signal-protocol layer, a
  fork of `tulir/libsignal-protocol-go`. Licensed **GPL-3.0**. This is why
  nuncio as a whole is GPL-3.0-only: a client built on it is a combined
  GPL-3.0 work.

## Other direct dependencies

Go daemon (`go.mod`):

| Module | Licence |
| --- | --- |
| `github.com/godbus/dbus/v5` | BSD-2-Clause |
| `github.com/google/uuid` | BSD-3-Clause |
| `github.com/mdp/qrterminal/v3` | MIT |
| `github.com/zalando/go-keyring` | MIT |
| `google.golang.org/protobuf` | BSD-3-Clause |
| `modernc.org/sqlite` | BSD-3-Clause (bundled SQLite is public domain) |

Rust clients (`crates/*/Cargo.toml`):

| Crate | Licence |
| --- | --- |
| `serde`, `serde_json` | MIT OR Apache-2.0 |
| `thiserror` | MIT OR Apache-2.0 |
| `toml` | MIT OR Apache-2.0 |
| `notify` | CC0-1.0 |
| `ratatui` | MIT |
| `ratatui-image` | MIT |
| `image` | MIT OR Apache-2.0 |

Each dependency's own licence file is the authoritative text.
