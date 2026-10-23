# Credits

nuncio is GPL-3.0-only (see `LICENSE`). This file names the projects that shaped
its design and the third-party code it is built on.

## Projects that shaped the design

- **nchat** by d99kris: a terminal chat client whose WhatsApp support set
  nuncio's reference points for the feature checklist, the media-export
  behaviour and the dependency update discipline. nuncio does not copy nchat's
  patched whatsmeow files; the media behaviour is re-implemented independently
  of upstream in `internal/media`.
- **Sealwax** ([PixDevsApps/sealwax](https://github.com/PixDevsApps/sealwax), MIT):
  the reference GTK 4 + libadwaita app. nuncio copied design ideas, not code:
  - in `nuncio-desktop`, the layout (the chat list as the sidebar and the
    conversation as the content of an `AdwNavigationSplitView`, one pane at a
    time on narrow windows, flat surfaces in the live Omarchy palette, toasts for
    notices), following the Omarchy theme, hiding the title bar on Hyprland, and
    drawing a picture's thumbnail first inside a box reserved at its final size,
    so nothing jumps when the full picture arrives;
  - in `packaging/`, the approach of `PKGBUILD.in`: a tarball of the committed
    tree, every dependency fetched in `prepare()` (`cargo fetch --locked`, here
    also `go mod download`), then an offline build and offline tests, with
    `options=('!debug' '!lto')`.
- **iamb** (a Matrix client on ratatui): studied as the reference design for
  drawing pictures in the terminal app; no code was copied.

## WhatsApp and Signal protocol stack

- **whatsmeow** (`go.mau.fi/whatsmeow`) by tulir (mautrix): the Go library for
  the WhatsApp web multi-device API. Licensed **MPL-2.0**; its files stay under
  MPL-2.0.
- **libsignal** (`go.mau.fi/libsignal`), whatsmeow's Signal-protocol layer, a
  fork of `tulir/libsignal-protocol-go`. Licensed **GPL-3.0**. This is why
  nuncio as a whole is GPL-3.0-only: a client built on it is a combined
  GPL-3.0 work.

## Other direct dependencies

Go daemon and CLI (`go.mod`):

| Module | Licence |
| --- | --- |
| `github.com/godbus/dbus/v5` | BSD-2-Clause |
| `github.com/google/uuid` | BSD-3-Clause |
| `github.com/mdp/qrterminal/v3` | MIT |
| `github.com/zalando/go-keyring` | MIT |
| `go.mau.fi/util` | MPL-2.0 |
| `google.golang.org/protobuf` | BSD-3-Clause |
| `modernc.org/sqlite` | BSD-3-Clause (bundled SQLite is public domain; bundled sqlite-vec is MIT) |

Rust clients (`crates/*/Cargo.toml`):

| Crate | Used by | Licence |
| --- | --- | --- |
| `serde`, `serde_json` | client, TUI (tests), desktop | MIT OR Apache-2.0 |
| `thiserror` | client | MIT OR Apache-2.0 |
| `toml` | client | MIT OR Apache-2.0 |
| `notify` | client | CC0-1.0 |
| `unicode-segmentation` | client, TUI | MIT OR Apache-2.0 |
| `base64` | client, TUI, desktop (tests) | MIT OR Apache-2.0 |
| `image` (PNG, JPEG, WebP only) | client, TUI, desktop | MIT OR Apache-2.0 |
| `ratatui` | TUI | MIT |
| `ratatui-image` | TUI | MIT |
| `time` | TUI | MIT OR Apache-2.0 |
| `libc` | TUI | MIT OR Apache-2.0 |
| `gtk4` | desktop | MIT |
| `libadwaita` | desktop | MIT |
| `glib` | desktop | MIT |
| `futures-channel` | desktop | MIT OR Apache-2.0 |

`image` decodes through `zune-jpeg` (MIT OR Apache-2.0 OR Zlib), `image-webp`
(MIT OR Apache-2.0) and `png` (MIT OR Apache-2.0); the TUI draws through
`crossterm` (MIT), which `ratatui` brings.

The Rust crates above are bindings: `nuncio-desktop` links at run time to the
system's GTK 4, libadwaita and GLib (all LGPL-2.1-or-later), which Arch provides
as the `gtk4`, `libadwaita` and `glib2` packages; nuncio does not ship them.

Each dependency's own licence file is the authoritative text. Versions are
pinned in `go.sum` and `Cargo.lock`.
