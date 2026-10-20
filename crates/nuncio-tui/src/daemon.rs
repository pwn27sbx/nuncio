//! The TUI's I/O: one connection for requests and one subscribed connection for events, as
//! `nuncio_client::conn` recommends, reconnected on a capped backoff when lost. Everything it
//! reads goes through [`App::apply`]; an [`Action::Open`] it gets back goes to the opener (the
//! desktop's by default). What an answer, a failure or a loss means for the view is the shared
//! driver's ([`nuncio_client::driver`]), so the desktop app follows the same rules.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use nuncio_client::driver::open_externally;
use nuncio_client::driver::{self, Host, Link, Outcome};
use nuncio_client::model::Effect;
use nuncio_client::{ClientError, ClientFrame, DEFAULT_READ_TIMEOUT, DaemonFrame};

use crate::{Action, App, Backoff, Reconnect};

/// How long a drain waits for one more event before the tick moves on.
const EVENT_WAIT: Duration = Duration::from_millis(1);
/// How the terminal client names itself in the driver's notices.
const CLIENT: driver::Client = driver::Client {
    program: "nuncio-tui",
    retry_hint: "R retries it once reconnected",
};

/// Opens a downloaded media file outside the terminal.
type Opener = Box<dyn FnMut(&Path) -> std::io::Result<()>>;

/// The connections to `nunciod`. When either is lost (the daemon restarted, a read hit its
/// bound, a frame was malformed) both are dropped and the driver reconnects on a capped backoff
/// ([`Reconnect`]): hello and subscribe again, then a resync. A send made meanwhile is shown as
/// not sent and is never retried by itself (`R` retries it).
pub struct Daemon {
    path: PathBuf,
    client: String,
    timeout: Duration,
    /// The request connection and the subscribed event connection, both or neither.
    link: Option<Link>,
    reconnect: Reconnect,
    opener: Opener,
}

/// What an [`Action`] is to the shared driver (`Quit` never comes from the daemon).
fn effect(action: Option<Action>) -> Option<Effect> {
    match action? {
        Action::Request(frame) => Some(Effect::Request(frame)),
        Action::Open(path) => Some(Effect::Open(path)),
        Action::Quit => None,
    }
}

impl Host for App {
    fn apply(&mut self, frame: &DaemonFrame) -> Option<Effect> {
        effect(App::apply(self, frame))
    }

    fn apply_reply(&mut self, request: &ClientFrame, reply: &DaemonFrame) -> Option<Effect> {
        effect(App::apply_reply(self, request, reply))
    }

    fn request_failed(&mut self, frame: &ClientFrame, message: String) {
        App::request_failed(self, frame, message);
    }

    fn send_not_confirmed(&mut self, client_id: &str) {
        App::send_not_confirmed(self, client_id);
    }

    fn set_connected(&mut self, connected: bool) {
        App::set_connected(self, connected);
    }

    fn set_notice(&mut self, notice: String) {
        App::set_notice(self, notice);
    }

    fn resync(&mut self) -> Vec<ClientFrame> {
        App::resync(self)
    }
}

impl Daemon {
    /// Connect and handshake the request connection, then a second connection that subscribes.
    pub fn connect(path: &Path, client: &str) -> Result<Self, ClientError> {
        Self::connect_with_timeout(path, client, DEFAULT_READ_TIMEOUT)
    }

    /// [`Daemon::connect`] with every read bounded by `timeout` instead of 5 s (tests).
    pub fn connect_with_timeout(
        path: &Path,
        client: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let link = Link::open(path, client, timeout)?;
        Ok(Self {
            link: Some(link),
            ..Self::disconnected(path, client).with_timeout(timeout)
        })
    }

    /// A driver that is not connected yet and connects on its backoff, from the first
    /// [`Daemon::drain_events`] on (the daemon answered `fetch` but not this connect).
    pub fn disconnected(path: &Path, client: &str) -> Self {
        let mut reconnect = Reconnect::new(Backoff::default());
        reconnect.lost(Instant::now());
        Self {
            path: path.to_owned(),
            client: client.to_owned(),
            timeout: DEFAULT_READ_TIMEOUT,
            link: None,
            reconnect,
            opener: Box::new(open_externally),
        }
    }

    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Reconnect on this schedule instead of the default one (tests shorten it).
    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        let reconnecting = self.reconnect.is_reconnecting();
        self.reconnect = Reconnect::new(backoff);
        if reconnecting {
            self.reconnect.lost(Instant::now());
        }
        self
    }

    /// Whether both connections are up.
    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// Open downloaded media with `opener` instead of [`open_externally`] (tests record it).
    pub fn with_opener(
        mut self,
        opener: impl FnMut(&Path) -> std::io::Result<()> + 'static,
    ) -> Self {
        self.opener = Box::new(opener);
        self
    }

    /// Open a file the user asked to download; a failure becomes the footer notice.
    fn open(&mut self, app: &mut App, path: &Path) {
        if let Err(error) = (self.opener)(path) {
            app.set_notice(format!("cannot open the media: {error}"));
        }
    }

    /// Drop both connections and schedule the reconnect; the header and footer say so.
    fn lost(&mut self, app: &mut App, error: &ClientError) {
        self.link = None;
        self.reconnect.lost(Instant::now());
        driver::lost(app, error);
    }

    /// Send `frame`, then fold the reply into `app` ([`driver::settle`]): a send (or retry) the
    /// daemon answers with `not_confirmed` is shown as not confirmed (it may have been delivered);
    /// one it refuses with any other `error` (as `not_linked`) is shown as not sent; every `error`
    /// also becomes the footer notice. A broken connection (closed, timed out, malformed) is
    /// returned as the error after a send (or retry) it carried is marked not sent, and the driver
    /// reconnects. While disconnected nothing is sent: a send (or retry) is shown as not sent, the
    /// footer says `not connected to nunciod`, and nothing retries it later. A `media_ready` the
    /// user asked for is opened, and a request the reply asks for (a `mark_read` after a chat is
    /// opened) is sent. A reply whose op this build does not know (a newer daemon) becomes the
    /// footer notice, and a send (or retry) it answered is shown as not confirmed.
    pub fn perform(&mut self, app: &mut App, frame: &ClientFrame) -> Result<(), ClientError> {
        let Some(link) = self.link.as_mut() else {
            driver::not_connected(app, frame, CLIENT);
            return Ok(());
        };
        let result = driver::request_on(&mut link.requests, frame);
        match driver::settle(app, frame, result, CLIENT) {
            Outcome::Done => Ok(()),
            Outcome::Effect(Effect::Open(path)) => {
                self.open(app, &path);
                Ok(())
            }
            Outcome::Effect(Effect::Request(next)) => self.perform(app, &next),
            Outcome::Lost(error) => {
                self.lost(app, &error);
                Err(error)
            }
        }
    }

    /// Reconnect when it is due, then read every queued event into `app` (the daemon drops a
    /// subscriber that falls behind). An event whose op this build does not know (a newer
    /// daemon) is ignored. A lost connection is handled here (the driver reconnects), so this
    /// returns `Ok` then too.
    pub fn drain_events(&mut self, app: &mut App) -> Result<(), ClientError> {
        if self.link.is_none() && self.reconnect.due(Instant::now()) {
            match Link::open(&self.path, &self.client, self.timeout) {
                Ok(link) => {
                    self.link = Some(link);
                    self.reconnect.succeeded();
                    self.resync(app);
                }
                Err(_) => self.reconnect.failed(Instant::now()),
            }
        }
        while let Some(link) = self.link.as_mut() {
            match link.events.next_event(EVENT_WAIT) {
                Ok(Some(frame)) => match app.apply(&frame) {
                    // A lost connection is already handled by `perform`.
                    Some(Action::Request(next)) => {
                        let _ = self.perform(app, &next);
                    }
                    Some(Action::Open(path)) => self.open(app, &path),
                    Some(Action::Quit) | None => {}
                },
                Ok(None) => break,
                Err(error) => self.lost(app, &error),
            }
        }
        Ok(())
    }

    /// Say the connection is back and ask again for what events would have said while
    /// disconnected; stops at a loss.
    fn resync(&mut self, app: &mut App) {
        for frame in driver::reconnected(app) {
            if self.perform(app, &frame).is_err() {
                return;
            }
        }
    }
}
