//! The TUI's I/O: one connection for requests and one subscribed connection for events, as
//! `nuncio_client::conn` recommends, reconnected on a capped backoff when lost. Everything it reads goes through [`App::apply`]; an
//! [`Action::Open`] it gets back goes to the opener (the desktop's by default).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nuncio_client::{
    ClientError, ClientFrame, Connection, DEFAULT_READ_TIMEOUT, DaemonFrame, ErrorCode,
};

use crate::{Action, App, Backoff, Reconnect};

/// How long a drain waits for one more event before the tick moves on.
const EVENT_WAIT: Duration = Duration::from_millis(1);
/// The footer's word for a request made while the driver is reconnecting.
const NOT_CONNECTED: &str = "not connected to nunciod";
/// How long a `download_media` reply may take (the Go client's bound too).
const MEDIA_WAIT: Duration = Duration::from_secs(90);

/// Opens a downloaded media file outside the terminal.
type Opener = Box<dyn FnMut(&Path) -> std::io::Result<()>>;

/// The production opener: `open` on macOS, `xdg-open` elsewhere, detached from the terminal.
/// It is reaped on a thread so it never lingers as a zombie.
pub fn open_externally(path: &Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

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

struct Link {
    requests: Connection,
    events: Connection,
}

impl Link {
    fn open(path: &Path, client: &str, timeout: Duration) -> Result<Self, ClientError> {
        let mut requests = Connection::connect_with_timeout(path, timeout)?;
        requests.handshake(client)?;
        let mut events = Connection::connect_with_timeout(path, timeout)?;
        events.handshake(client)?;
        events.subscribe()?;
        Ok(Self { requests, events })
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
        app.set_connected(false);
        app.set_notice(format!("daemon connection lost: {error}; reconnecting…"));
    }

    /// Send `frame`, then fold the reply into `app`. A send (or retry) the daemon answers with
    /// `not_confirmed` is shown as not confirmed (it may have been delivered); one it refuses
    /// with any other `error` (as `not_linked`) is shown as not sent; every `error` also becomes
    /// the footer notice. A broken connection (closed, timed out, malformed) is returned as the
    /// error after a send (or retry) it carried is marked not sent, and the driver reconnects.
    /// While disconnected nothing is sent: a send (or retry) is shown as not sent, the footer
    /// says `not connected to nunciod`, and nothing retries it later. A `media_ready` the user
    /// asked for is opened, and a request the reply asks for (a `mark_read` after a chat is
    /// opened) is sent. A reply whose op this build does not know (a newer daemon) becomes the
    /// footer notice, and a send (or retry) it answered is shown as not confirmed.
    pub fn perform(&mut self, app: &mut App, frame: &ClientFrame) -> Result<(), ClientError> {
        let failed_send = |app: &mut App, message: String| {
            if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } =
                frame
            {
                app.apply(&DaemonFrame::SendFailed {
                    v: nuncio_client::PROTOCOL_VERSION,
                    client_id: client_id.clone(),
                    message,
                });
            }
        };
        let Some(link) = self.link.as_mut() else {
            failed_send(app, NOT_CONNECTED.to_owned());
            let notice = match frame {
                ClientFrame::Send { .. } | ClientFrame::Retry { .. } => {
                    format!("{NOT_CONNECTED}: not sent (R retries it once reconnected)")
                }
                _ => format!("{NOT_CONNECTED}; reconnecting…"),
            };
            app.request_failed(frame, notice);
            return Ok(());
        };
        let result = match frame {
            ClientFrame::DownloadMedia { .. } => link.requests.request_within(frame, MEDIA_WAIT),
            _ => link.requests.request(frame),
        };
        match result {
            // A newer daemon's reply this build cannot read: the request's outcome is unknown, so
            // a send (or retry) is shown as not confirmed (`R` reuses its WhatsApp id).
            Ok(DaemonFrame::Unknown { op }) => {
                if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } =
                    frame
                {
                    app.send_not_confirmed(client_id);
                }
                app.request_failed(
                    frame,
                    format!("daemon: unknown reply `{op}`; this nuncio-tui may be too old"),
                );
                Ok(())
            }
            // The reply to this very request: a different chat in it is the one the daemon served
            // (ADR 0001), which `apply_reply` adopts.
            Ok(reply) => match app.apply_reply(frame, &reply) {
                Some(Action::Open(path)) => {
                    self.open(app, &path);
                    Ok(())
                }
                Some(Action::Request(next)) => self.perform(app, &next),
                Some(Action::Quit) | None => Ok(()),
            },
            Err(ClientError::Daemon { code, message }) => {
                match frame {
                    ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. }
                        if code == ErrorCode::NotConfirmed.as_str() =>
                    {
                        app.send_not_confirmed(client_id)
                    }
                    _ => failed_send(app, message.clone()),
                }
                app.request_failed(frame, format!("daemon: {code}: {message}"));
                Ok(())
            }
            Err(error) => {
                failed_send(app, error.to_string());
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
                    app.set_connected(true);
                    app.set_notice("reconnected to nunciod");
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

    /// Ask again for what events would have said while disconnected; stops at a loss.
    fn resync(&mut self, app: &mut App) {
        for frame in app.resync() {
            if self.perform(app, &frame).is_err() {
                return;
            }
        }
    }
}
