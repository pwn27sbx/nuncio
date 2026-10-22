//! The NDJSON connection to `nunciod` (see `proto/wire.md`).
//!
//! This uses a **blocking** `std::os::unix::net::UnixStream`, not `tokio`: the client
//! work is one short request/response handshake, so a blocking socket is the simpler of
//! the two and needs no async runtime in the TUI or in the GTK main loop.
//!
//! Events are polled, not pushed: after [`Connection::subscribe`] a UI calls
//! [`Connection::next_event`] with a short wait from its own loop (a TUI tick, a GTK timeout).
//! A subscribed connection carries only events once it is subscribed, so a UI keeps it apart
//! from the connection it makes requests on. On each tick a UI drains the queue, calling
//! `next_event` until it returns `Ok(None)`: the daemon drops a subscriber that falls 64 events
//! behind, so reading one event per tick can lose the connection in a burst. After
//! [`ClientError::Closed`] the UI subscribes again on a new connection and resyncs its state
//! (`status`, `chats`), since events may have been missed.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::wire::{Chat, ClientFrame, DaemonFrame, MAX_FRAME_BYTES, Session, StopReason};

/// The socket path, relative to `$XDG_RUNTIME_DIR`.
const SOCKET_RELATIVE: &str = "nuncio/core.sock";

/// The bound on every read from the daemon, so a wedged daemon cannot wedge the client.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything that can go wrong while talking to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("$XDG_RUNTIME_DIR is not set, so the daemon socket path is unknown")]
    NoRuntimeDir,
    #[error("cannot connect to the daemon at {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("i/o error on the daemon socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed JSON frame: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame of {bytes} bytes is over the {MAX_FRAME_BYTES} byte limit")]
    FrameTooLarge { bytes: usize },
    /// A read hit its bound. Only reads are bounded, so a request that ends here was written
    /// in full; a send or retry whose reply timed out is [`ClientError::SendUnanswered`] instead.
    #[error("the daemon did not answer within {0:?}")]
    Timeout(Duration),
    /// A `send` or `retry` was written in full but its reply did not come within the bound: the
    /// daemon may already have handed it to WhatsApp, so it is not confirmed, never not sent.
    /// The connection is unusable, as after [`ClientError::Timeout`].
    #[error("not confirmed: the daemon did not answer within {0:?}; it may have been delivered")]
    SendUnanswered(Duration),
    #[error("the daemon closed the connection")]
    Closed,
    #[error("the daemon replied with error `{code}`: {message}")]
    Daemon { code: String, message: String },
    #[error("expected a `{expected}` frame, got `{got}`")]
    Unexpected { expected: &'static str, got: String },
}

/// What a client needs to render: the daemon state plus the chat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub daemon: String,
    pub linked: bool,
    pub session: Session,
    pub unread: u32,
    pub chats: Vec<Chat>,
    /// Why WhatsApp stopped the session, as `status` said; `None` while it runs.
    pub stop_reason: Option<StopReason>,
}

impl Snapshot {
    /// Parse a `chats` frame — as in `proto/fixtures/chats_response.json` — into a
    /// snapshot, for a client that has to render while the daemon is unreachable.
    ///
    /// `unread` is the sum of the chat counts (the authoritative count arrives in
    /// `status`), and `session`/`linked` are the unlinked defaults: this is fixture data
    /// standing in for a daemon that is not there.
    pub fn from_chats_json(text: &str) -> Result<Self, ClientError> {
        match serde_json::from_str(text)? {
            DaemonFrame::Chats { chats, .. } => Ok(Self {
                daemon: "fixture".to_owned(),
                linked: false,
                session: Session::Unlinked,
                unread: chats.iter().map(|chat| chat.unread).sum(),
                chats,
                stop_reason: None,
            }),
            other => Err(ClientError::Unexpected {
                expected: "chats",
                got: other.op().to_owned(),
            }),
        }
    }
}

/// An open connection to the daemon.
#[derive(Debug)]
pub struct Connection {
    stream: BufReader<UnixStream>,
    timeout: Duration,
    /// The start of a frame whose end has not arrived yet, kept across a quiet
    /// [`Connection::next_event`] wait so the stream never loses its place.
    pending: Vec<u8>,
}

impl Connection {
    /// Connect with every read bounded by [`DEFAULT_READ_TIMEOUT`].
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        Self::connect_with_timeout(path, DEFAULT_READ_TIMEOUT)
    }

    /// Connect with an explicit bound on every read. A read that hits the bound fails with
    /// [`ClientError::Timeout`], and the connection is then unusable: drop it.
    pub fn connect_with_timeout(path: &Path, timeout: Duration) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
            path: path.to_owned(),
            source,
        })?;
        // A daemon that drops the connection right after accepting it (one shutting down)
        // can close before this line, and macOS then refuses the timeout with EINVAL. Only a
        // socket shut down in both directions is refused, so no read can block on it: the
        // first `recv` returns what the daemon sent, then `Closed`.
        match stream.set_read_timeout(Some(timeout)) {
            Err(err) if err.kind() != std::io::ErrorKind::InvalidInput => return Err(err.into()),
            _ => {}
        }
        Ok(Self {
            stream: BufReader::new(stream),
            timeout,
            pending: Vec::new(),
        })
    }

    /// Connect to `$XDG_RUNTIME_DIR/nuncio/core.sock`.
    pub fn connect_default() -> Result<Self, ClientError> {
        Self::connect(&socket_path()?)
    }

    /// The bound on every read of this connection.
    pub fn read_bound(&self) -> Duration {
        self.timeout
    }

    /// Send one frame as a single NDJSON line.
    pub fn send(&mut self, frame: &ClientFrame) -> Result<(), ClientError> {
        let mut line = serde_json::to_string(frame)?;
        line.push('\n');
        let stream = self.stream.get_mut();
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// Read one frame, failing on a malformed line or one over the size limit.
    pub fn recv(&mut self) -> Result<DaemonFrame, ClientError> {
        // A frame begun before a timed-out wait continues where it stopped.
        let mut line = std::mem::take(&mut self.pending);
        // `take` bounds the read so a daemon that never sends a newline cannot make the
        // client allocate without limit.
        let room = (MAX_FRAME_BYTES + 1).saturating_sub(line.len()) as u64;
        let result = (&mut self.stream).take(room).read_until(b'\n', &mut line);
        if result.is_err() {
            self.pending = line;
            line = Vec::new();
        }
        let read = result.map_err(|err| match err.kind() {
            // A socket read timeout surfaces as WouldBlock on Unix and TimedOut elsewhere.
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                ClientError::Timeout(self.timeout)
            }
            _ => ClientError::Io(err),
        })?;
        if read == 0 && line.is_empty() {
            return Err(ClientError::Closed);
        }
        if line.len() > MAX_FRAME_BYTES {
            return Err(ClientError::FrameTooLarge { bytes: line.len() });
        }
        // Within the size bound, a line without its newline means the stream ended inside a
        // frame: the daemon closed the connection mid-write (as when it drops a subscriber).
        if line.pop() != Some(b'\n') {
            return Err(ClientError::Closed);
        }
        Ok(serde_json::from_slice(&line)?)
    }

    /// Send a frame and read the reply, turning an `error` frame into
    /// [`ClientError::Daemon`]. A reply whose op this build does not know comes back as
    /// [`DaemonFrame::Unknown`]; the caller decides what that means for its request. A `send` or
    /// `retry` written in full whose reply times out is [`ClientError::SendUnanswered`]; a
    /// failure before it was written stays what it was (`Connect`, `Io`, ...).
    pub fn request(&mut self, frame: &ClientFrame) -> Result<DaemonFrame, ClientError> {
        self.send(frame)?;
        let reply = self.recv().map_err(|error| match (error, frame) {
            (
                ClientError::Timeout(waited),
                ClientFrame::Send { .. } | ClientFrame::Retry { .. },
            ) => ClientError::SendUnanswered(waited),
            (error, _) => error,
        })?;
        match reply {
            DaemonFrame::Error { code, message, .. } => Err(ClientError::Daemon {
                code: code.as_str().to_owned(),
                message,
            }),
            reply => Ok(reply),
        }
    }

    /// [`Connection::request`] with this one reply allowed up to `wait` instead of the
    /// connection's read bound: a `download_media` reply can take far longer than an ordinary
    /// read (the Go client waits 90 s). Later reads are bounded as before.
    pub fn request_within(
        &mut self,
        frame: &ClientFrame,
        wait: Duration,
    ) -> Result<DaemonFrame, ClientError> {
        let bound = self.timeout;
        self.stream
            .get_ref()
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        self.timeout = wait;
        let reply = self.request(frame);
        self.timeout = bound;
        let _ = self.stream.get_ref().set_read_timeout(Some(bound));
        reply
    }

    /// Send `subscribe` and check the daemon's `subscribed`. From then on the daemon pushes
    /// event frames on this connection; read them with [`Connection::next_event`]. Subscribing
    /// again is harmless: each event still arrives once.
    pub fn subscribe(&mut self) -> Result<(), ClientError> {
        match self.request(&ClientFrame::subscribe())? {
            DaemonFrame::Subscribed { .. } => Ok(()),
            other => Err(ClientError::Unexpected {
                expected: "subscribed",
                got: other.op().to_owned(),
            }),
        }
    }

    /// Wait up to `wait` (at least 1 ms) for the next event frame. `Ok(None)` means
    /// none arrived in time, and the connection stays usable, even when the wait ended in the
    /// middle of a frame. A frame whose op this build does not know is returned as
    /// [`DaemonFrame::Unknown`] (a newer daemon's event: ignore it); any other frame that is not
    /// an event is [`ClientError::Unexpected`]. A closed connection is [`ClientError::Closed`], also when it
    /// ends inside a frame: the daemon drops a subscriber that falls behind. Drain on each tick
    /// (call until `Ok(None)`), and after `Closed` reconnect, subscribe and resync.
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<DaemonFrame>, ClientError> {
        // macOS refuses to set a socket timeout once the peer has closed (EINVAL). The read
        // still tells the truth then: the frames already buffered, then `Closed`.
        let _ = self
            .stream
            .get_ref()
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))));
        let frame = self.recv();
        let _ = self.stream.get_ref().set_read_timeout(Some(self.timeout));
        match frame {
            Ok(frame) if is_event(&frame) => Ok(Some(frame)),
            Ok(other) => Err(ClientError::Unexpected {
                expected: "event",
                got: other.op().to_owned(),
            }),
            Err(ClientError::Timeout(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Send `hello` and check the daemon's `hello_ok`, which the wire contract requires to
    /// be the first exchange.
    pub fn handshake(&mut self, client: &str) -> Result<(), ClientError> {
        match self.request(&ClientFrame::hello(client))? {
            DaemonFrame::HelloOk { .. } => Ok(()),
            other => Err(ClientError::Unexpected {
                expected: "hello_ok",
                got: other.op().to_owned(),
            }),
        }
    }
}

/// Whether `frame` is one the daemon pushes to subscribers, unprompted. An op this build does
/// not know counts as one: on a subscribed connection it is a newer daemon's event, which a UI
/// ignores rather than drop the connection.
fn is_event(frame: &DaemonFrame) -> bool {
    matches!(
        frame,
        DaemonFrame::EventMessage { .. }
            | DaemonFrame::EventConnection { .. }
            | DaemonFrame::EventUnread { .. }
            | DaemonFrame::EventFocus { .. }
            | DaemonFrame::EventChat { .. }
            | DaemonFrame::Unknown { .. }
    )
}

/// Connect, handshake, and fetch the daemon state and chat list.
pub fn fetch(path: &Path, client: &str, limit: u16) -> Result<Snapshot, ClientError> {
    let mut connection = Connection::connect(path)?;
    connection.handshake(client)?;

    let (daemon, linked, session, unread, stop_reason) =
        match connection.request(&ClientFrame::status())? {
            DaemonFrame::Status {
                daemon,
                linked,
                session,
                unread,
                stop_reason,
                ..
            } => (daemon, linked, session, unread, stop_reason),
            other => {
                return Err(ClientError::Unexpected {
                    expected: "status",
                    got: other.op().to_owned(),
                });
            }
        };

    let chats = match connection.request(&ClientFrame::chats(limit))? {
        DaemonFrame::Chats { chats, .. } => chats,
        other => {
            return Err(ClientError::Unexpected {
                expected: "chats",
                got: other.op().to_owned(),
            });
        }
    };

    Ok(Snapshot {
        daemon,
        linked,
        session,
        unread,
        chats,
        stop_reason,
    })
}

/// `$XDG_RUNTIME_DIR/nuncio/core.sock`.
pub fn socket_path() -> Result<PathBuf, ClientError> {
    socket_path_from(env::var_os("XDG_RUNTIME_DIR").as_deref().map(Path::new))
}

/// The socket path for a given runtime directory, split out so it is testable without
/// touching the process environment.
pub fn socket_path_from(runtime_dir: Option<&Path>) -> Result<PathBuf, ClientError> {
    let dir = runtime_dir.ok_or(ClientError::NoRuntimeDir)?;
    Ok(dir.join(SOCKET_RELATIVE))
}

#[cfg(test)]
mod tests {
    use super::Snapshot;
    use crate::wire::Session;

    #[test]
    fn a_chats_fixture_becomes_a_snapshot() {
        let text = include_str!("../../../proto/fixtures/chats_response.json");
        let snapshot = Snapshot::from_chats_json(text).expect("the fixture parses");

        assert_eq!(snapshot.chats.len(), 3);
        assert_eq!(snapshot.session, Session::Unlinked);
        assert!(!snapshot.linked);
        // 2 + 0 + 17
        assert_eq!(snapshot.unread, 19);
    }

    #[test]
    fn a_non_chats_frame_is_not_a_snapshot() {
        let hello_ok = include_str!("../../../proto/fixtures/hello_ok_daemon.json");
        let err = Snapshot::from_chats_json(hello_ok).unwrap_err();
        assert!(matches!(
            err,
            super::ClientError::Unexpected {
                expected: "chats",
                ref got
            } if got == "hello_ok"
        ));
    }

    #[test]
    fn request_within_waits_longer_than_the_read_bound_for_one_reply_only() {
        use std::io::{BufRead, BufReader, Write};
        use std::time::Duration;

        let path = std::path::PathBuf::from(format!("/tmp/ncl-within-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut writer = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            for _ in 0..2 {
                line.clear();
                reader.read_line(&mut line).expect("request");
                std::thread::sleep(Duration::from_millis(300));
                writer
                    .write_all(b"{\"v\":1,\"op\":\"subscribed\"}\n")
                    .expect("reply");
            }
        });
        let mut conn = super::Connection::connect_with_timeout(&path, Duration::from_millis(100))
            .expect("connect");
        let frame = crate::wire::ClientFrame::subscribe();
        let reply = conn.request_within(&frame, Duration::from_secs(2));
        assert!(
            matches!(reply, Ok(crate::wire::DaemonFrame::Subscribed { .. })),
            "{reply:?}"
        );
        // The longer wait was for that reply only.
        assert!(matches!(
            conn.request(&frame),
            Err(super::ClientError::Timeout(_))
        ));
        server.join().expect("server");
        let _ = std::fs::remove_file(&path);
    }
}
