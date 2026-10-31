//! The device-linking flow as a client folds it (`proto/wire.md`, "The link flow").
//!
//! The daemon owns the flow: a client sends `link_start` with the user's explicit consent, the
//! daemon answers `link_started`, and from then on it publishes `link_qr` and `link_state` to
//! subscribed connections. [`LinkFlow`] keeps what a screen needs from those frames — the code to
//! draw and where the flow stands — and nothing else. The connections stay the client's, so the
//! terminal client folds frames in its own loop and the desktop app from its worker thread; a
//! client that did not start the flow (another one did) folds the same frames with
//! [`LinkFlow::new`].

use crate::wire::{DaemonFrame, LinkState};

/// One QR code of a linking flow, as `link_qr` carries it: `size` modules per side, row 0 first,
/// each module `0` (light) or `1` (dark), with no quiet zone (a client draws a 4-module margin
/// around the code).
///
/// The code is pairing material: it is never logged, kept in a file or shown in a notification,
/// and not even `Debug` prints its matrix (a client must keep a raw `link_qr` frame out of its own
/// logs too).
#[derive(Clone, PartialEq, Eq)]
pub struct LinkQr {
    size: u32,
    rows: Vec<String>,
}

impl LinkQr {
    /// Modules per side: the matrix is `size` × `size`.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The matrix rows, row 0 first; each is `size` characters of `0` or `1`.
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// Whether the module at `row`/`col` is dark. Outside the matrix it is light, so drawing never
    /// indexes past the code a daemon sent.
    pub fn module(&self, row: u32, col: u32) -> bool {
        self.rows
            .get(row as usize)
            .and_then(|line| line.as_bytes().get(col as usize))
            == Some(&b'1')
    }
}

impl std::fmt::Debug for LinkQr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkQr").field("size", &self.size).finish()
    }
}

/// What one folded frame tells a client to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkUpdate {
    /// A new code arrived; each one replaces the one before, so draw [`LinkFlow::qr`] again.
    Qr,
    /// The flow's state changed.
    State(LinkState),
}

/// The device-linking flow as one client knows it.
#[derive(Debug, Default)]
pub struct LinkFlow {
    phase: Option<LinkState>,
    qr: Option<LinkQr>,
    reason: Option<String>,
}

impl LinkFlow {
    /// A flow about which nothing has been folded yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the flow stands; `None` until its first `link_state`.
    pub fn phase(&self) -> Option<LinkState> {
        self.phase.clone()
    }

    /// The code to draw right now; `None` before the first one, and once the flow ended (a linked
    /// or failed flow has nothing left to scan).
    pub fn qr(&self) -> Option<&LinkQr> {
        self.qr.as_ref()
    }

    /// The one fixed sentence to show when the flow failed, exactly as the daemon sent it; the
    /// client writes no sentence of its own.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Fold one frame: `Some` when it belongs to the flow, `None` when the client handles it (an
    /// event of another kind, a reply, or one this build does not know).
    pub fn apply(&mut self, frame: &DaemonFrame) -> Option<LinkUpdate> {
        match frame {
            DaemonFrame::EventLinkQr { size, rows, .. } => {
                self.qr = Some(LinkQr {
                    size: *size,
                    rows: rows.clone(),
                });
                Some(LinkUpdate::Qr)
            }
            DaemonFrame::EventLinkState { state, reason, .. } => {
                self.reason = reason.clone();
                if matches!(state, LinkState::Linked | LinkState::Failed) {
                    self.qr = None;
                }
                self.phase = Some(state.clone());
                Some(LinkUpdate::State(state.clone()))
            }
            _ => None,
        }
    }
}
