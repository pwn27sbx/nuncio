//! Shared nuncio client library.
//!
//! `nuncio-tui` and `nuncio-desktop` both talk to the Go daemon (`nunciod`) through this
//! crate, so the client-side work — the NDJSON wire contract, the connection and Omarchy
//! theme following — has exactly one Rust implementation.
//!
//! The wire contract itself lives in `proto/wire.md` and is checked against the shared
//! fixtures in `proto/fixtures/` by the Go daemon's tests and by `tests/contract.rs` here.

pub mod conn;
pub mod theme;
pub mod wire;

pub use conn::{ClientError, Connection, Snapshot, fetch, socket_path, socket_path_from};
pub use theme::{Mode, Palette, Rgb, ThemeError, ThemeWatcher, default_path_from};
pub use wire::{
    Chat, ClientFrame, DaemonFrame, ErrorCode, MAX_FRAME_BYTES, Message, PROTOCOL_VERSION, Session,
};
