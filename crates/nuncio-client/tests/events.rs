//! Receiving daemon events over a real `AF_UNIX` socket.
//!
//! A scripted fake daemon runs on a socket in a temporary directory (never inside the
//! worktree): it answers requests, pushes frames unprompted and pauses, as the real daemon
//! does once a connection subscribed.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::{ClientError, Connection, DaemonFrame, Session};
use serde_json::{Value, json};

const HELLO_OK: &str = include_str!("../../../proto/fixtures/hello_ok_daemon.json");
const SUBSCRIBED: &str = include_str!("../../../proto/fixtures/subscribed_response.json");
const STATUS: &str = include_str!("../../../proto/fixtures/status_response.json");
const REFUSED: &str = include_str!("../../../proto/fixtures/error_version_mismatch.json");

/// Every event fixture, so a new event kind without a client test is noticed here.
const EVENT_FIXTURES: [(&str, &str); 4] = [
    (
        "event_message",
        include_str!("../../../proto/fixtures/event_message.json"),
    ),
    (
        "event_connection",
        include_str!("../../../proto/fixtures/event_connection.json"),
    ),
    (
        "event_unread",
        include_str!("../../../proto/fixtures/event_unread.json"),
    ),
    (
        "event_focus",
        include_str!("../../../proto/fixtures/event_focus.json"),
    ),
];

/// A directory under the system temp dir, removed on drop; unique per instance.
struct TempDir(PathBuf);

static NEXT_TEMP_DIR: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nuncio-events-{}-{unique}-{tag}",
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

/// One step of the fake daemon's script.
enum Step {
    /// Read one request line, then answer with this frame.
    Reply(&'static str),
    /// Write these bytes unprompted, exactly as given (no newline is added).
    Push(Vec<u8>),
    /// Wait before the next step.
    Pause(Duration),
}

/// A frame as one NDJSON line: fixtures end with a newline, a frame carries exactly one.
fn line(frame: &str) -> Vec<u8> {
    let mut bytes = frame.trim_end().as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
}

/// Runs `script` against the first client; returns the requests it read.
fn fake_daemon(script: Vec<Step>) -> (TempDir, PathBuf, thread::JoinHandle<Vec<Value>>) {
    let dir = TempDir::new("daemon");
    let path = dir.0.join("core.sock");
    let listener = UnixListener::bind(&path).expect("socket binds");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("one client connects");
        let mut reader = BufReader::new(stream.try_clone().expect("socket clones"));
        let mut writer = stream;
        let mut requests = Vec::new();
        for step in script {
            match step {
                Step::Reply(frame) => {
                    let mut request = String::new();
                    if reader.read_line(&mut request).expect("request is readable") == 0 {
                        break;
                    }
                    requests.push(serde_json::from_str(&request).expect("requests are JSON"));
                    writer.write_all(&line(frame)).expect("reply is writable");
                }
                Step::Push(bytes) => writer.write_all(&bytes).expect("push is writable"),
                Step::Pause(wait) => thread::sleep(wait),
            }
            writer.flush().expect("frame flushes");
        }
        requests
    });
    (dir, path, server)
}

fn subscribed_connection(path: &std::path::Path) -> Connection {
    let mut connection = Connection::connect(path).expect("connects");
    connection.handshake("nuncio-tui/0.1.0").expect("handshake");
    connection.subscribe().expect("subscribe is answered");
    connection
}

const WAIT: Duration = Duration::from_secs(2);
const SHORT: Duration = Duration::from_millis(50);

#[test]
fn every_event_fixture_is_received_through_next_event() {
    let mut script = vec![Step::Reply(HELLO_OK), Step::Reply(SUBSCRIBED)];
    script.extend(EVENT_FIXTURES.iter().map(|(_, f)| Step::Push(line(f))));
    let (_dir, path, server) = fake_daemon(script);

    let mut connection = subscribed_connection(&path);
    for (op, fixture) in EVENT_FIXTURES {
        let event = connection
            .next_event(WAIT)
            .expect("the event is received")
            .expect("an event arrives within the wait");
        assert_eq!(event.op(), op);
        let want: Value = serde_json::from_str(fixture).expect("fixture is JSON");
        assert_eq!(
            serde_json::to_value(&event).expect("the event serialises"),
            want
        );
    }

    let requests = server.join().expect("the fake daemon finishes");
    assert_eq!(
        requests,
        vec![
            json!({"v": 1, "op": "hello", "client": "nuncio-tui/0.1.0"}),
            json!({"v": 1, "op": "subscribe"}),
        ]
    );
}

#[test]
fn received_events_carry_their_fields() {
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(SUBSCRIBED),
        Step::Push(line(EVENT_FIXTURES[0].1)),
        Step::Push(line(EVENT_FIXTURES[1].1)),
        Step::Push(line(EVENT_FIXTURES[2].1)),
    ]);

    let mut connection = subscribed_connection(&path);
    match connection.next_event(WAIT).expect("receives") {
        Some(DaemonFrame::EventMessage { jid, message, .. }) => {
            assert_eq!(jid, "15551230001@s.whatsapp.net");
            assert_eq!(message.id, "3EB0AAA3");
            assert_eq!(message.text, "Running late");
            assert!(!message.from_me);
        }
        other => panic!("expected event_message, got {other:?}"),
    }
    assert!(matches!(
        connection.next_event(WAIT).expect("receives"),
        Some(DaemonFrame::EventConnection {
            session: Session::Offline,
            ..
        })
    ));
    assert!(matches!(
        connection.next_event(WAIT).expect("receives"),
        Some(DaemonFrame::EventUnread {
            unread: 20,
            chat_unread: 3,
            ..
        })
    ));
    let _ = server.join();
}

#[test]
fn no_event_within_the_wait_is_none_and_the_connection_stays_usable() {
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(SUBSCRIBED),
        Step::Pause(Duration::from_millis(300)),
        Step::Push(line(EVENT_FIXTURES[3].1)),
    ]);

    let mut connection = subscribed_connection(&path);
    let started = Instant::now();
    assert_eq!(connection.next_event(SHORT).expect("a quiet wait"), None);
    assert!(
        started.elapsed() < 10 * SHORT,
        "waited {:?}",
        started.elapsed()
    );
    let event = connection.next_event(WAIT).expect("receives");
    assert!(
        matches!(event, Some(DaemonFrame::EventFocus { ref jid, .. }) if jid == "15551230002@s.whatsapp.net"),
        "expected event_focus, got {event:?}"
    );
    let _ = server.join();
}

#[test]
fn a_frame_split_across_a_quiet_wait_is_not_lost() {
    let frame = line(EVENT_FIXTURES[0].1);
    let (head, tail) = frame.split_at(frame.len() / 2);
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(SUBSCRIBED),
        Step::Push(head.to_vec()),
        Step::Pause(Duration::from_millis(300)),
        Step::Push(tail.to_vec()),
    ]);

    let mut connection = subscribed_connection(&path);
    assert_eq!(
        connection
            .next_event(SHORT)
            .expect("half a frame is a quiet wait"),
        None
    );
    let event = connection.next_event(WAIT).expect("the whole frame parses");
    assert!(
        matches!(event, Some(DaemonFrame::EventMessage { .. })),
        "expected event_message, got {event:?}"
    );
    let _ = server.join();
}

#[test]
fn the_read_bound_of_requests_survives_a_quiet_wait() {
    // After next_event's own short wait, reads are bounded by the connection's timeout again.
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(STATUS),
        Step::Pause(Duration::from_secs(2)),
    ]);

    let bound = Duration::from_millis(400);
    let mut connection = Connection::connect_with_timeout(&path, bound).expect("connects");
    connection.handshake("nuncio-tui/0.1.0").expect("handshake");
    assert_eq!(connection.next_event(SHORT).expect("a quiet wait"), None);
    connection
        .request(&nuncio_client::ClientFrame::status())
        .expect("status answers");
    let started = Instant::now();
    let err = connection.recv().unwrap_err();
    let waited = started.elapsed();
    assert!(
        matches!(err, ClientError::Timeout(b) if b == bound),
        "expected the connection's own bound, got {err:?}"
    );
    assert!(
        waited >= bound * 3 / 4,
        "recv gave up after {waited:?}, not after the {bound:?} bound"
    );
    let _ = server.join();
}

#[test]
fn a_subscribe_answered_with_another_frame_is_unexpected() {
    let (_dir, path, server) = fake_daemon(vec![Step::Reply(HELLO_OK), Step::Reply(STATUS)]);

    let mut connection = Connection::connect(&path).expect("connects");
    connection.handshake("nuncio-tui/0.1.0").expect("handshake");
    let err = connection.subscribe().unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Unexpected {
                expected: "subscribed",
                got: "status"
            }
        ),
        "expected an unexpected-frame error, got {err:?}"
    );
    let _ = server.join();
}

#[test]
fn a_refused_subscribe_is_a_daemon_error() {
    let (_dir, path, server) = fake_daemon(vec![Step::Reply(HELLO_OK), Step::Reply(REFUSED)]);

    let mut connection = Connection::connect(&path).expect("connects");
    connection.handshake("nuncio-tui/0.1.0").expect("handshake");
    let err = connection.subscribe().unwrap_err();
    assert!(
        matches!(err, ClientError::Daemon { ref code, .. } if code == "version_mismatch"),
        "expected a daemon error, got {err:?}"
    );
    let _ = server.join();
}

#[test]
fn a_reply_where_an_event_belongs_is_unexpected() {
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(SUBSCRIBED),
        Step::Push(line(STATUS)),
    ]);

    let mut connection = subscribed_connection(&path);
    let err = connection.next_event(WAIT).unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Unexpected {
                expected: "event",
                got: "status"
            }
        ),
        "expected an unexpected-frame error, got {err:?}"
    );
    let _ = server.join();
}

#[test]
fn a_dropped_subscriber_cut_mid_frame_is_closed() {
    let event = EVENT_FIXTURES[0].1;
    let mut cut = line(event);
    cut.truncate(cut.len() / 2);
    let (_dir, path, server) = fake_daemon(vec![
        Step::Reply(HELLO_OK),
        Step::Reply(SUBSCRIBED),
        Step::Push(line(event)),
        Step::Push(cut),
    ]);
    let mut connection = subscribed_connection(&path);
    let _ = server.join();
    assert!(matches!(connection.next_event(WAIT), Ok(Some(_))));
    let second = connection.next_event(WAIT);
    assert!(
        matches!(second, Err(ClientError::Closed)),
        "second = {second:?}"
    );
}
