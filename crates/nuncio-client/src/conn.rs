//! The NDJSON connection to `nunciod` (see `proto/wire.md`).
//!
//! This uses a **blocking** `std::os::unix::net::UnixStream`, not `tokio`: the client
//! work is one short request/response handshake, so a blocking socket is the simpler of
//! the two and needs no async runtime in the TUI or in the GTK main loop.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::wire::{Chat, ClientFrame, DaemonFrame, MAX_FRAME_BYTES, Session};

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
    #[error("the daemon did not answer within {0:?}")]
    Timeout(Duration),
    #[error("the daemon closed the connection")]
    Closed,
    #[error("the daemon replied with error `{code}`: {message}")]
    Daemon { code: String, message: String },
    #[error("expected a `{expected}` frame, got `{got}`")]
    Unexpected {
        expected: &'static str,
        got: &'static str,
    },
}

/// What a client needs to render: the daemon state plus the chat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub daemon: String,
    pub linked: bool,
    pub session: Session,
    pub unread: u32,
    pub chats: Vec<Chat>,
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
            }),
            other => Err(ClientError::Unexpected {
                expected: "chats",
                got: other.op(),
            }),
        }
    }
}

/// An open connection to the daemon.
#[derive(Debug)]
pub struct Connection {
    stream: BufReader<UnixStream>,
    timeout: Duration,
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
        stream.set_read_timeout(Some(timeout))?;
        Ok(Self {
            stream: BufReader::new(stream),
            timeout,
        })
    }

    /// Connect to `$XDG_RUNTIME_DIR/nuncio/core.sock`.
    pub fn connect_default() -> Result<Self, ClientError> {
        Self::connect(&socket_path()?)
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
        let mut line = Vec::new();
        // `take` bounds the read so a daemon that never sends a newline cannot make the
        // client allocate without limit.
        let read = (&mut self.stream)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|err| match err.kind() {
                // A socket read timeout surfaces as WouldBlock on Unix and TimedOut elsewhere.
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                    ClientError::Timeout(self.timeout)
                }
                _ => ClientError::Io(err),
            })?;
        if read == 0 {
            return Err(ClientError::Closed);
        }
        if line.len() > MAX_FRAME_BYTES {
            return Err(ClientError::FrameTooLarge { bytes: line.len() });
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        Ok(serde_json::from_slice(&line)?)
    }

    /// Send a frame and read the reply, turning an `error` frame into
    /// [`ClientError::Daemon`].
    pub fn request(&mut self, frame: &ClientFrame) -> Result<DaemonFrame, ClientError> {
        self.send(frame)?;
        match self.recv()? {
            DaemonFrame::Error { code, message, .. } => Err(ClientError::Daemon {
                code: code.as_str().to_owned(),
                message,
            }),
            reply => Ok(reply),
        }
    }

    /// Send `hello` and check the daemon's `hello_ok`, which the wire contract requires to
    /// be the first exchange.
    pub fn handshake(&mut self, client: &str) -> Result<(), ClientError> {
        match self.request(&ClientFrame::hello(client))? {
            DaemonFrame::HelloOk { .. } => Ok(()),
            other => Err(ClientError::Unexpected {
                expected: "hello_ok",
                got: other.op(),
            }),
        }
    }
}

/// Connect, handshake, and fetch the daemon state and chat list.
pub fn fetch(path: &Path, client: &str, limit: u16) -> Result<Snapshot, ClientError> {
    let mut connection = Connection::connect(path)?;
    connection.handshake(client)?;

    let (daemon, linked, session, unread) = match connection.request(&ClientFrame::status())? {
        DaemonFrame::Status {
            daemon,
            linked,
            session,
            unread,
            ..
        } => (daemon, linked, session, unread),
        other => {
            return Err(ClientError::Unexpected {
                expected: "status",
                got: other.op(),
            });
        }
    };

    let chats = match connection.request(&ClientFrame::chats(limit))? {
        DaemonFrame::Chats { chats, .. } => chats,
        other => {
            return Err(ClientError::Unexpected {
                expected: "chats",
                got: other.op(),
            });
        }
    };

    Ok(Snapshot {
        daemon,
        linked,
        session,
        unread,
        chats,
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
                got: "hello_ok"
            }
        ));
    }
}
