//! The outbox against a fake daemon on a Unix socket. Times are scaled: 40 ms stand for 1 s, so
//! the read bound is 200 ms (5 s), the send wait 1 s (25 s), and replies come at 240 ms (6 s) and
//! 760 ms (19 s), after the read bound and before the send wait.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::AtomicU32;
use std::time::Instant;

use crate::wire::ErrorCode;

const HELLO_OK: &str = r#"{"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}"#;
const ANA: &str = "15551230001@s.whatsapp.net";
const READ_BOUND: Duration = Duration::from_millis(200);
const SEND_WAIT_SCALED: Duration = Duration::from_millis(1000);

static NEXT: AtomicU32 = AtomicU32::new(0);

fn socket() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ncob-{}-{n}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn settings(path: &Path, send_wait: Duration) -> Settings {
    Settings {
        send_wait,
        ..Settings::new(path, "nuncio-test", READ_BOUND)
    }
}

/// The outbox, posting into a channel the test reads.
fn start(path: &Path, send_wait: Duration) -> (Outbox, mpsc::Receiver<Posted>) {
    let (tx, rx) = mpsc::channel();
    let outbox = Outbox::start(settings(path, send_wait), move |posted| {
        let _ = tx.send(posted);
    });
    (outbox, rx)
}

fn next(rx: &mpsc::Receiver<Posted>) -> Posted {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the outbox posts")
}

/// The daemon's side of the outbox connection, handshaken.
struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    fn accept(listener: &UnixListener) -> Self {
        let (stream, _) = listener.accept().expect("the outbox connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let writer = stream.try_clone().expect("clone");
        let mut peer = Self {
            reader: BufReader::new(stream),
            writer,
        };
        assert!(peer.line().contains("\"hello\""));
        peer.reply(HELLO_OK);
        peer
    }

    fn line(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("a line");
        line
    }

    fn request(&mut self) -> serde_json::Value {
        serde_json::from_str(&self.line()).expect("json")
    }

    fn reply(&mut self, frame: &str) {
        self.writer
            .write_all(format!("{frame}\n").as_bytes())
            .expect("write");
    }

    fn sent(&mut self, client_id: &str, id: &str) {
        self.reply(&format!(
            r#"{{"v":1,"op":"sent","client_id":"{client_id}","state":"sent","id":"{id}","timestamp":"2026-10-22T10:00:00Z"}}"#
        ));
    }

    /// Nothing more arrives within `wait` (EOF or silence).
    fn nothing_more(&mut self, wait: Duration) -> bool {
        // macOS refuses a timeout once the outbox closed its end; the read then ends at once.
        let _ = self.reader.get_ref().set_read_timeout(Some(wait));
        let mut line = String::new();
        matches!(self.reader.read_line(&mut line), Ok(0) | Err(_)) && line.is_empty()
    }
}

fn send(client_id: &str) -> ClientFrame {
    ClientFrame::send(ANA, "hi", client_id)
}

fn sent_id(posted: Posted) -> (String, String) {
    match posted {
        Posted::Reply {
            request: ClientFrame::Send { client_id, .. },
            result: Ok(DaemonFrame::Sent { id, .. }),
        } => (client_id, id.unwrap_or_default()),
        other => panic!("expected a sent reply, got {other:?}"),
    }
}

/// D1: replies at "6 s" and "19 s" are past the read bound and within the send wait: both are
/// answered, each to its own send, in order, and each send reached the daemon once.
#[test]
fn replies_past_the_read_bound_come_back_sent_each_to_its_send_in_order() {
    let path = socket();
    let listener = UnixListener::bind(&path).expect("bind");
    let (outbox, rx) = start(&path, SEND_WAIT_SCALED);
    outbox.push(send("t-1"));
    outbox.push(send("t-2"));
    let mut peer = Peer::accept(&listener);
    let first = peer.request();
    assert_eq!(first["client_id"], "t-1");
    thread::sleep(Duration::from_millis(240));
    peer.sent("t-1", "M1");
    let second = peer.request();
    assert_eq!(second["client_id"], "t-2", "in the order pushed");
    thread::sleep(Duration::from_millis(760));
    peer.sent("t-2", "M2");

    assert_eq!(sent_id(next(&rx)), ("t-1".to_owned(), "M1".to_owned()));
    assert_eq!(sent_id(next(&rx)), ("t-2".to_owned(), "M2".to_owned()));
    assert!(
        peer.nothing_more(Duration::from_millis(200)),
        "no duplicate"
    );
    drop(outbox);
    let _ = std::fs::remove_file(&path);
}

/// D1: the daemon's own `not_confirmed` (sent at "10 s" here) comes back as its answer, not as
/// the client's own timeout.
#[test]
fn the_daemons_not_confirmed_past_the_read_bound_comes_back_as_its_answer() {
    let path = socket();
    let listener = UnixListener::bind(&path).expect("bind");
    let (outbox, rx) = start(&path, SEND_WAIT_SCALED);
    outbox.push(ClientFrame::retry("t-1"));
    let mut peer = Peer::accept(&listener);
    assert_eq!(peer.request()["op"], "retry");
    thread::sleep(Duration::from_millis(400));
    peer.reply(r#"{"v":1,"op":"error","code":"not_confirmed","message":"timed out"}"#);
    match next(&rx) {
        Posted::Reply {
            request: ClientFrame::Retry { .. },
            result: Err(ClientError::Daemon { code, .. }),
        } => assert_eq!(code, ErrorCode::NotConfirmed.as_str()),
        other => panic!("expected the daemon's not_confirmed, got {other:?}"),
    }
    drop(outbox);
    let _ = std::fs::remove_file(&path);
}

/// F3 kept: a send with no reply within the send wait is not confirmed (`SendUnanswered`, after
/// the whole wait), and the frame queued behind it is never written.
#[test]
fn a_send_unanswered_within_the_wait_is_unanswered_and_the_next_is_not_sent() {
    let path = socket();
    let listener = UnixListener::bind(&path).expect("bind");
    let wait = Duration::from_millis(500);
    let (outbox, rx) = start(&path, wait);
    let pushed = Instant::now();
    outbox.push(send("t-1"));
    outbox.push(send("t-2"));
    let mut peer = Peer::accept(&listener);
    assert_eq!(peer.request()["client_id"], "t-1");
    match next(&rx) {
        Posted::Reply {
            result: Err(ClientError::SendUnanswered(waited)),
            ..
        } => assert_eq!(waited, wait),
        other => panic!("expected an unanswered send, got {other:?}"),
    }
    assert!(pushed.elapsed() >= wait, "it waited the whole send wait");
    assert!(matches!(
        next(&rx),
        Posted::NotConnected(ClientFrame::Send { ref client_id, .. }) if client_id == "t-2"
    ));
    assert!(
        peer.nothing_more(Duration::from_millis(200)),
        "t-2 never sent"
    );
    drop(outbox);
    let _ = std::fs::remove_file(&path);
}

/// The client lost its link: the send in flight is still answered (applied by its client id),
/// and what was waiting behind it is never written.
#[test]
fn a_dropped_outbox_answers_the_send_in_flight_and_writes_nothing_more() {
    let path = socket();
    let listener = UnixListener::bind(&path).expect("bind");
    let (outbox, rx) = start(&path, SEND_WAIT_SCALED);
    outbox.push(send("t-1"));
    let mut peer = Peer::accept(&listener);
    assert_eq!(peer.request()["client_id"], "t-1");
    outbox.push(send("t-2"));
    drop(outbox);
    thread::sleep(Duration::from_millis(240));
    peer.sent("t-1", "M1");
    assert_eq!(sent_id(next(&rx)), ("t-1".to_owned(), "M1".to_owned()));
    assert!(matches!(next(&rx), Posted::NotConnected(_)));
    assert!(
        peer.nothing_more(Duration::from_millis(200)),
        "t-2 never sent"
    );
    let _ = std::fs::remove_file(&path);
}

/// F3 kept: a send that could not be written (no daemon) is a loss that never reached it, so the
/// driver shows it not sent.
#[test]
fn a_send_with_no_daemon_to_connect_to_is_a_connect_loss() {
    let path = socket();
    let (outbox, rx) = start(&path, SEND_WAIT_SCALED);
    outbox.push(send("t-1"));
    assert!(matches!(
        next(&rx),
        Posted::Reply {
            result: Err(ClientError::Connect { .. }),
            ..
        }
    ));
    outbox.push(send("t-2"));
    assert!(matches!(next(&rx), Posted::NotConnected(_)));
}
