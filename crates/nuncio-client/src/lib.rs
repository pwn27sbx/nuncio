//! Shared nuncio client library.
//!
//! `nuncio-tui` and `nuncio-desktop` both talk to the Go daemon (`nunciod`) through this
//! crate, so the client-side work — the NDJSON wire contract, the connection, Omarchy theme
//! following, the config file, `whatsapp:` links and which message notifies — has exactly one
//! Rust implementation.
//!
//! The wire contract itself lives in `proto/wire.md` and is checked against the shared
//! fixtures in `proto/fixtures/` by the Go daemon's tests and by `tests/contract.rs` here.

pub mod config;
pub mod conn;
pub mod link;
pub mod notify;
pub mod theme;
pub mod wire;

pub use config::{Config, ConfigError, ImageProtocol, config_path_from};
pub use conn::{
    ClientError, Connection, DEFAULT_READ_TIMEOUT, Snapshot, fetch, socket_path, socket_path_from,
};
pub use link::{LinkError, LinkTarget, MAX_DRAFT_CHARS, parse_link};
pub use notify::{Notification, Notified, ViewState, chat_title, default_action, notification_for};
pub use theme::{Mode, Palette, Rgb, ThemeError, ThemeWatcher, default_path_from};
pub use wire::{
    Chat, ClientFrame, DaemonFrame, ErrorCode, MAX_FRAME_BYTES, Media, Message, PROTOCOL_VERSION,
    Reaction, SendState, Session,
};
