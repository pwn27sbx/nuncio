//! The NDJSON connection: handshake over a real `AF_UNIX` socket.
//!
//! These tests run a fake daemon on a socket in a temporary directory (never inside the
//! worktree) and speak the protocol to it.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::{
    ClientError, ClientFrame, Connection, MAX_FRAME_BYTES, Session, fetch, socket_path_from,
};
use serde_json::{Value, json};

const HELLO_OK: &str = include_str!("../../../proto/fixtures/hello_ok_daemon.json");
const STATUS: &str = include_str!("../../../proto/fixtures/status_response.json");
const CHATS: &str = include_str!("../../../proto/fixtures/chats_response.json");
const VERSION_MISMATCH: &str = include_str!("../../../proto/fixtures/error_version_mismatch.json");

/// A directory under the system temp dir, removed on drop.
///
/// Each instance gets its own path (tests run in parallel, so a shared tag would have one
/// test deleting another's socket).
struct TempDir(PathBuf);

static NEXT_TEMP_DIR: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nuncio-client-{}-{unique}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir is creatable");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A fake daemon that accepts one connection and answers each request with the next reply.
///
/// Returns the requests it received, in order.
fn fake_daemon(replies: Vec<Vec<u8>>) -> (TempDir, PathBuf, thread::JoinHandle<Vec<String>>) {
    let dir = TempDir::new("daemon");
    let path = dir.0.join("core.sock");
    let listener = UnixListener::bind(&path).expect("socket binds");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("one client connects");
        let mut reader = BufReader::new(stream.try_clone().expect("socket clones"));
        let mut writer = stream;
        let mut requests = Vec::new();
        for reply in replies {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("request is readable") == 0 {
                break;
            }
            requests.push(line.trim_end().to_owned());
            writer.write_all(&reply).expect("reply is writable");
            writer.write_all(b"\n").expect("terminator is writable");
            writer.flush().expect("reply flushes");
        }
        requests
    });
    (dir, path, server)
}

/// Fixture files end with a newline; a frame carries exactly one.
fn lines(replies: &[&str]) -> Vec<Vec<u8>> {
    replies
        .iter()
        .map(|reply| reply.trim_end().as_bytes().to_vec())
        .collect()
}

#[test]
fn fetch_performs_the_hello_status_chats_handshake() {
    let (_dir, path, server) = fake_daemon(lines(&[HELLO_OK, STATUS, CHATS]));

    let snapshot = fetch(&path, "nuncio-tui/0.1.0", 40).expect("the handshake succeeds");

    assert_eq!(snapshot.daemon, "nunciod/0.1.0");
    assert_eq!(snapshot.session, Session::Unlinked);
    assert!(!snapshot.linked);
    assert_eq!(snapshot.unread, 0);
    assert_eq!(snapshot.chats.len(), 3);
    assert_eq!(snapshot.chats[0].name, "Ada Lovelace");
    assert_eq!(snapshot.chats[2].unread, 17);

    let requests: Vec<Value> = server
        .join()
        .expect("the fake daemon finishes")
        .iter()
        .map(|line| serde_json::from_str(line).expect("requests are JSON"))
        .collect();
    assert_eq!(
        requests,
        vec![
            json!({"v": 1, "op": "hello", "client": "nuncio-tui/0.1.0"}),
            json!({"v": 1, "op": "status"}),
            json!({"v": 1, "op": "chats", "limit": 40}),
        ]
    );
}

#[test]
fn a_daemon_that_refuses_our_version_is_a_daemon_error() {
    let (_dir, path, server) = fake_daemon(lines(&[VERSION_MISMATCH]));

    let mut connection = Connection::connect(&path).expect("connects");
    let err = connection.handshake("nuncio-tui/0.1.0").unwrap_err();

    match err {
        ClientError::Daemon {
            ref code,
            ref message,
        } => {
            assert_eq!(code, "version_mismatch");
            assert_eq!(message, "unsupported protocol version");
        }
        other => panic!("expected a daemon error, got {other:?}"),
    }
    let _ = server.join();
}

#[test]
fn an_unreachable_socket_is_a_connect_error() {
    let dir = TempDir::new("missing");
    let err = fetch(&dir.0.join("nothing.sock"), "nuncio-tui/0.1.0", 40).unwrap_err();
    assert!(
        matches!(err, ClientError::Connect { .. }),
        "expected a connect error, got {err:?}"
    );
}

#[test]
fn malformed_json_is_reported_as_a_json_error() {
    let (_dir, path, server) = fake_daemon(lines(&["{not json at all"]));

    let mut connection = Connection::connect(&path).expect("connects");
    connection
        .send(&ClientFrame::status())
        .expect("the request is sent");
    let err = connection.recv().unwrap_err();

    assert!(
        matches!(err, ClientError::Json(_)),
        "expected a JSON error, got {err:?}"
    );
    let _ = server.join();
}

#[test]
fn a_frame_over_one_mebibyte_is_rejected() {
    // No trailing newline, so the client must give up on the size, not on the terminator.
    let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
    let (_dir, path, server) = fake_daemon(vec![oversized]);

    let mut connection = Connection::connect(&path).expect("connects");
    connection
        .send(&ClientFrame::status())
        .expect("the request is sent");
    let err = connection.recv().unwrap_err();

    assert!(
        matches!(err, ClientError::FrameTooLarge { .. }),
        "expected a frame-size error, got {err:?}"
    );
    drop(server);
}

#[test]
fn a_closed_connection_is_reported_as_closed() {
    let (_dir, path, server) = fake_daemon(vec![]);

    let mut connection = Connection::connect(&path).expect("connects");
    let err = connection.recv().unwrap_err();

    assert!(
        matches!(err, ClientError::Closed),
        "expected a closed connection, got {err:?}"
    );
    let _ = server.join();
}

/// A daemon that drops each connection the moment it accepts it (as one shutting down
/// does) can close before the client has set its read timeout, and macOS then refuses the
/// timeout with EINVAL. Connecting must still succeed and the read must say `Closed`, never
/// an opaque I/O error. Many attempts, because the window is a few microseconds wide.
#[test]
fn a_daemon_that_closes_right_after_accept_is_reported_as_closed() {
    const ATTEMPTS: usize = 20_000;
    let dir = TempDir::new("closer");
    let path = dir.0.join("core.sock");
    let listener = UnixListener::bind(&path).expect("socket binds");
    let server = thread::spawn(move || {
        for _ in 0..ATTEMPTS {
            let (stream, _) = listener.accept().expect("a client connects");
            drop(stream);
        }
    });

    for attempt in 0..ATTEMPTS {
        let mut connection = Connection::connect(&path)
            .unwrap_or_else(|err| panic!("attempt {attempt}: connects, got {err:?}"));
        let err = connection.recv().unwrap_err();
        assert!(
            matches!(err, ClientError::Closed),
            "attempt {attempt}: expected a closed connection, got {err:?}"
        );
    }
    server.join().expect("the daemon accepted every attempt");
}

#[test]
fn socket_path_follows_xdg_runtime_dir() {
    assert_eq!(
        socket_path_from(Some("/run/user/1000".as_ref())).expect("a path"),
        Path::new("/run/user/1000/nuncio/core.sock")
    );
    assert!(matches!(
        socket_path_from(None),
        Err(ClientError::NoRuntimeDir)
    ));
}

/// A wedged daemon: it accepts, reads requests and never answers.
fn silent_daemon() -> (TempDir, PathBuf, thread::JoinHandle<()>) {
    let dir = TempDir::new("silent");
    let path = dir.0.join("core.sock");
    let listener = UnixListener::bind(&path).expect("socket binds");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("one client connects");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // Hold the connection open until the client gives up and closes it.
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            line.clear();
        }
    });
    (dir, path, server)
}

const TEST_READ_TIMEOUT: Duration = Duration::from_millis(100);

#[test]
fn a_wedged_daemon_makes_request_time_out() {
    let (_dir, path, server) = silent_daemon();

    let mut connection =
        Connection::connect_with_timeout(&path, TEST_READ_TIMEOUT).expect("connects");
    let started = Instant::now();
    let err = connection.request(&ClientFrame::status()).unwrap_err();
    let elapsed = started.elapsed();

    assert!(
        matches!(err, ClientError::Timeout(bound) if bound == TEST_READ_TIMEOUT),
        "expected a timeout, got {err:?}"
    );
    assert!(
        elapsed < 10 * TEST_READ_TIMEOUT,
        "gave up after {elapsed:?}"
    );
    drop(connection);
    let _ = server.join();
}

#[test]
fn a_short_timeout_does_not_break_a_normal_exchange() {
    let (_dir, path, server) = fake_daemon(lines(&[HELLO_OK, STATUS, STATUS]));

    let mut connection =
        Connection::connect_with_timeout(&path, TEST_READ_TIMEOUT).expect("connects");
    connection.handshake("nuncio-tui/0.1.0").expect("handshake");
    for _ in 0..2 {
        // Idle time between requests must not count against a read.
        thread::sleep(TEST_READ_TIMEOUT);
        connection
            .request(&ClientFrame::status())
            .expect("status answers");
    }
    let _ = server.join();
}
