//! Shared nuncio client library.
//!
//! `nuncio-tui` and `nuncio-desktop` both talk to the Go daemon (`nunciod`) through this
//! crate, so the client-side work — the NDJSON wire contract, the connection, Omarchy theme
//! and font following, the config file, `whatsapp:` links, which message notifies, and the view models of
//! the chat list, the transcript and the composer ([`model`]) — has exactly one Rust implementation.
//!
//! The wire contract itself lives in `proto/wire.md` and is checked against the shared
//! fixtures in `proto/fixtures/` by the Go daemon's tests and by `tests/contract.rs` here.

pub mod config;
pub mod conn;
pub mod driver;
pub mod font;
pub mod link;
pub mod model;
pub mod notify;
pub mod pictures;
pub mod reconnect;
pub mod theme;
pub mod wire;

pub use config::{Config, ConfigError, ImageProtocol, SaveError, TitleBar, config_path_from};
pub use conn::{
    ClientError, Connection, DEFAULT_READ_TIMEOUT, Snapshot, fetch, socket_path, socket_path_from,
};
pub use driver::{Host, Link, Outcome, is_loss, open_externally};
pub use font::{FontError, FontWatcher, Run, RunError, current_font, parse_fc_match};
pub use link::{LinkError, LinkTarget, MAX_DRAFT_CHARS, parse_link};
pub use model::{
    ChatList, Composer, Download, Effect, Model, Search, Status, Transcript, chat_title,
};
pub use notify::{Notification, Notified, ViewState, default_action, notification_for};
pub use reconnect::{Backoff, Reconnect};
pub use theme::{Mode, Palette, Rgb, ThemeError, ThemeWatcher, default_path_from};
pub use wire::{
    Chat, ClientFrame, DaemonFrame, ErrorCode, MAX_FRAME_BYTES, Media, Message, PROTOCOL_VERSION,
    Reaction, SendState, Session,
};
