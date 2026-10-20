//! The worker against a fake daemon on a Unix socket: no GTK, no WhatsApp.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

const HELLO_OK: &str = r#"{"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}"#;
const STATUS: &str =
    r#"{"v":1,"op":"status","daemon":"fake","linked":true,"session":"linked","unread":0}"#;
const EVENT: &str =
    r#"{"v":1,"op":"event_unread","unread":2,"jid":"15551230001@s.whatsapp.net","chat_unread":2}"#;

fn socket(label: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/ndw-{}-{label}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn line(reader: &mut BufReader<UnixStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("a line");
    line
}

fn reply(stream: &mut UnixStream, frame: &str) {
    stream
        .write_all(format!("{frame}\n").as_bytes())
        .expect("write");
}

/// The next connection, failing the test instead of hanging when the worker never connects.
fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).expect("nonblocking");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "the worker never connected");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
}

/// One life of the fake daemon: both connections accepted and handshaken.
struct Life {
    requests: BufReader<UnixStream>,
    requests_out: UnixStream,
    events_out: UnixStream,
}

impl Life {
    fn accept(listener: &UnixListener) -> Self {
        let requests = accept(listener);
        let mut requests_out = requests.try_clone().expect("clone");
        let mut requests = BufReader::new(requests);
        assert!(line(&mut requests).contains("\"hello\""));
        reply(&mut requests_out, HELLO_OK);
        let events = accept(listener);
        let mut events_out = events.try_clone().expect("clone");
        let mut events = BufReader::new(events);
        assert!(line(&mut events).contains("\"hello\""));
        reply(&mut events_out, HELLO_OK);
        assert!(line(&mut events).contains("\"subscribe\""));
        reply(&mut events_out, r#"{"v":1,"op":"subscribed"}"#);
        Self {
            requests,
            requests_out,
            events_out,
        }
    }

    /// Answer the next request with `status`, returning its op line.
    fn serve(&mut self) -> String {
        let asked = line(&mut self.requests);
        reply(&mut self.requests_out, STATUS);
        asked
    }
}

fn spawn(path: &std::path::Path) -> (Worker, mpsc::Receiver<Incoming>) {
    let (tx, rx) = mpsc::channel();
    let worker = Worker::spawn(
        Settings {
            path: path.to_owned(),
            client: "nuncio-desktop/test".to_owned(),
            timeout: Duration::from_secs(2),
            backoff: Backoff::new(Duration::from_millis(20), Duration::from_millis(80)),
        },
        move |incoming| {
            let _ = tx.send(incoming);
        },
    );
    (worker, rx)
}

fn next(rx: &mpsc::Receiver<Incoming>) -> Incoming {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the worker posts")
}

#[test]
fn it_connects_answers_requests_in_order_and_posts_events() {
    let path = socket("serve");
    let listener = UnixListener::bind(&path).expect("bind");
    let (worker, rx) = spawn(&path);
    let mut life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));

    worker.send(ClientFrame::status());
    worker.send(ClientFrame::chats(5));
    assert!(life.serve().contains("\"status\""));
    assert!(life.serve().contains("\"chats\""), "in the order asked");
    for _ in 0..2 {
        match next(&rx) {
            Incoming::Reply { result, .. } => {
                assert!(matches!(result, Ok(DaemonFrame::Status { .. })));
            }
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    reply(&mut life.events_out, EVENT);
    assert!(matches!(
        next(&rx),
        Incoming::Event(DaemonFrame::EventUnread { unread: 2, .. })
    ));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn while_no_daemon_answers_a_request_is_not_sent_and_it_connects_once_one_does() {
    let path = socket("late");
    let (worker, rx) = spawn(&path);
    worker.send(ClientFrame::status());
    assert!(matches!(
        next(&rx),
        Incoming::NotConnected(ClientFrame::Status { .. })
    ));

    let listener = UnixListener::bind(&path).expect("bind");
    let _life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_lost_event_connection_is_posted_and_both_reconnect_after_the_backoff() {
    let path = socket("events");
    let listener = UnixListener::bind(&path).expect("bind");
    let (worker, rx) = spawn(&path);
    let life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));

    let lost_at = Instant::now();
    drop(life);
    assert!(matches!(next(&rx), Incoming::Lost(_)));
    let mut life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));
    assert!(
        lost_at.elapsed() >= Duration::from_millis(20),
        "the first attempt waits the backoff's first delay"
    );

    worker.send(ClientFrame::status());
    assert!(
        life.serve().contains("\"status\""),
        "the new life serves requests"
    );
    assert!(matches!(next(&rx), Incoming::Reply { result: Ok(_), .. }));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_request_on_a_broken_connection_is_answered_with_the_loss_and_not_sent_again() {
    let path = socket("requests");
    let listener = UnixListener::bind(&path).expect("bind");
    let (worker, rx) = spawn(&path);
    let life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));

    drop(listener);
    let _ = std::fs::remove_file(&path);
    drop(life);
    worker.send(ClientFrame::send("15551230001@s.whatsapp.net", "hi", "d-1"));
    let mut saw_reply = false;
    // The event thread may notice first; either way the send comes back failed, exactly once.
    for _ in 0..2 {
        match next(&rx) {
            Incoming::Reply { request, result } => {
                assert!(matches!(request, ClientFrame::Send { .. }));
                assert!(result.as_ref().is_err_and(driver::is_loss), "{result:?}");
                saw_reply = true;
            }
            Incoming::NotConnected(ClientFrame::Send { .. }) => saw_reply = true,
            Incoming::Lost(_) => {}
            other => panic!("unexpected {other:?}"),
        }
        if saw_reply {
            break;
        }
    }
    assert!(saw_reply);
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Incoming::Lost(_)) | Err(_) => {}
            Ok(other) => panic!("nothing resends it, got {other:?}"),
        }
    }
}

#[test]
fn dropping_the_worker_closes_both_connections_and_stops_reconnecting() {
    let path = socket("drop");
    let listener = UnixListener::bind(&path).expect("bind");
    let (worker, rx) = spawn(&path);
    let mut life = Life::accept(&listener);
    assert!(matches!(next(&rx), Incoming::Connected));
    drop(worker);
    life.requests
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    let mut buf = String::new();
    assert_eq!(
        life.requests
            .read_line(&mut buf)
            .expect("EOF, not a timeout"),
        0
    );
    drop(life);
    listener.set_nonblocking(true).expect("nonblocking");
    thread::sleep(Duration::from_millis(300));
    assert!(listener.accept().is_err(), "a dropped worker reconnected");
    let _ = std::fs::remove_file(&path);
}

fn event() -> Incoming {
    Incoming::Event(DaemonFrame::EventUnread {
        v: 1,
        unread: 1,
        jid: "15551230001@s.whatsapp.net".to_owned(),
        chat_unread: 1,
    })
}

#[test]
fn events_past_the_backlog_are_dropped_as_one_overflow_and_replies_never_are() {
    let backlog = Backlog::new(2);
    assert!(matches!(backlog.admit(event()), Some(Incoming::Event(_))));
    assert!(matches!(backlog.admit(event()), Some(Incoming::Event(_))));
    assert!(matches!(backlog.admit(event()), Some(Incoming::Overflow)));
    assert!(
        backlog.admit(event()).is_none(),
        "one overflow for the whole burst"
    );
    assert!(
        matches!(
            backlog.admit(Incoming::NotConnected(ClientFrame::status())),
            Some(Incoming::NotConnected(_))
        ),
        "only events are dropped"
    );
    // The main loop catches up: events pass again, and a new burst says so again.
    backlog.delivered(&event());
    backlog.delivered(&Incoming::Overflow);
    assert!(matches!(backlog.admit(event()), Some(Incoming::Event(_))));
    assert!(matches!(backlog.admit(event()), Some(Incoming::Overflow)));
}
