//! The TUI's I/O: one connection for requests and one subscribed connection for events, as
//! `nuncio_client::conn` recommends. Everything it reads goes through [`App::apply`]; an
//! [`Action::Open`] it gets back goes to the opener (the desktop's by default).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nuncio_client::{ClientError, ClientFrame, Connection, DaemonFrame};

use crate::{Action, App};

/// How long a drain waits for one more event before the tick moves on.
const EVENT_WAIT: Duration = Duration::from_millis(1);
/// How long after a lost event connection the next resubscribe is tried.
const RESUBSCRIBE_AFTER: Duration = Duration::from_secs(5);
/// How many chats a resync asks for.
const CHAT_LIMIT: u16 = 200;
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

/// The connections to `nunciod`.
pub struct Daemon {
    path: PathBuf,
    client: String,
    requests: Connection,
    events: Option<Connection>,
    resubscribe_at: Instant,
    opener: Opener,
}

impl Daemon {
    /// Connect and handshake the request connection, then a second connection that subscribes.
    pub fn connect(path: &Path, client: &str) -> Result<Self, ClientError> {
        let mut requests = Connection::connect(path)?;
        requests.handshake(client)?;
        let events = subscribed(path, client)?;
        Ok(Self {
            path: path.to_owned(),
            client: client.to_owned(),
            requests,
            events: Some(events),
            resubscribe_at: Instant::now(),
            opener: Box::new(open_externally),
        })
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

    /// Send `frame`, then fold the reply into `app`. A send the daemon refuses with an `error`
    /// (as `not_linked`, or a send that was not confirmed) is shown as not sent; any other
    /// `error` becomes the footer notice. A broken connection is returned as the error, after a
    /// send (or retry) it carried is marked not sent. A `media_ready` the user asked for is
    /// opened.
    pub fn perform(&mut self, app: &mut App, frame: &ClientFrame) -> Result<(), ClientError> {
        let result = match frame {
            ClientFrame::DownloadMedia { .. } => self.requests.request_within(frame, MEDIA_WAIT),
            _ => self.requests.request(frame),
        };
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
        match result {
            Ok(reply) => {
                if let Some(Action::Open(path)) = app.apply(&reply) {
                    self.open(app, &path);
                }
                Ok(())
            }
            Err(ClientError::Daemon { code, message }) => {
                failed_send(app, message.clone());
                app.request_failed(frame, format!("daemon: {code}: {message}"));
                Ok(())
            }
            Err(error) => {
                failed_send(app, error.to_string());
                Err(error)
            }
        }
    }

    /// Read every queued event into `app` (the daemon drops a subscriber that falls behind).
    /// When the event connection is lost, subscribe again (at most every few seconds) and
    /// resync the chat list, the status and the open conversation, since events were missed.
    pub fn drain_events(&mut self, app: &mut App) -> Result<(), ClientError> {
        if self.events.is_none() && Instant::now() >= self.resubscribe_at {
            self.resubscribe_at = Instant::now() + RESUBSCRIBE_AFTER;
            if let Ok(events) = subscribed(&self.path, &self.client) {
                self.events = Some(events);
                self.resync(app)?;
            }
        }
        while let Some(events) = self.events.as_mut() {
            match events.next_event(EVENT_WAIT) {
                Ok(Some(frame)) => match app.apply(&frame) {
                    Some(Action::Request(next)) => self.perform(app, &next)?,
                    Some(Action::Open(path)) => self.open(app, &path),
                    Some(Action::Quit) | None => {}
                },
                Ok(None) => break,
                Err(_) => {
                    self.events = None;
                    app.set_notice("lost the event connection; resubscribing");
                }
            }
        }
        Ok(())
    }

    fn resync(&mut self, app: &mut App) -> Result<(), ClientError> {
        self.perform(app, &ClientFrame::status())?;
        self.perform(app, &ClientFrame::chats(CHAT_LIMIT))?;
        if let Some(jid) = app.selected_jid().map(str::to_owned) {
            self.perform(app, &ClientFrame::messages(jid, CHAT_LIMIT))?;
        }
        Ok(())
    }
}

fn subscribed(path: &Path, client: &str) -> Result<Connection, ClientError> {
    let mut events = Connection::connect(path)?;
    events.handshake(client)?;
    events.subscribe()?;
    Ok(events)
}
