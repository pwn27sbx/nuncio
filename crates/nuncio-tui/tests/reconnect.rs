//! B3: the TUI reconnects to `nunciod` after it restarts (or after a request timed out), with a
//! capped backoff, says so on screen, resubscribes and resyncs without losing the draft, the
//! selection or the open chat, and never retries a send by itself.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::{Chat, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Backoff, Daemon, Pane, Reconnect, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

const ADA: &str = "15551230001@s.whatsapp.net";
const GRACE: &str = "15551230002@s.whatsapp.net";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// --- the policy, with no clock and no socket -----------------------------------------------

#[test]
fn the_backoff_doubles_from_half_a_second_to_a_five_second_cap_and_resets() {
    let mut backoff = Backoff::default();
    let delays: Vec<Duration> = (0..7).map(|_| backoff.next_delay()).collect();
    assert_eq!(
        delays,
        [
            ms(500),
            ms(1000),
            ms(2000),
            ms(4000),
            ms(5000),
            ms(5000),
            ms(5000)
        ]
    );
    backoff.reset();
    assert_eq!(backoff.next_delay(), ms(500));
}

#[test]
fn a_reconnect_is_due_only_after_its_delay_and_a_success_starts_over() {
    let t0 = Instant::now();
    let mut reconnect = Reconnect::new(Backoff::new(ms(500), ms(5000)));
    assert!(!reconnect.is_reconnecting());
    assert!(!reconnect.due(t0));

    reconnect.lost(t0);
    assert!(reconnect.is_reconnecting());
    assert!(!reconnect.due(t0 + ms(499)));
    assert!(reconnect.due(t0 + ms(500)));
    // A second loss while already reconnecting does not push the attempt back.
    reconnect.lost(t0 + ms(400));
    assert!(reconnect.due(t0 + ms(500)));

    // The attempt failed: the next one waits twice as long.
    let t1 = t0 + ms(500);
    reconnect.failed(t1);
    assert!(!reconnect.due(t1 + ms(999)));
    assert!(reconnect.due(t1 + ms(1000)));

    reconnect.succeeded();
    assert!(!reconnect.is_reconnecting());
    assert!(!reconnect.due(t1 + ms(60_000)));
    let t2 = t1 + ms(60_000);
    reconnect.lost(t2);
    assert!(!reconnect.due(t2 + ms(499)));
    assert!(reconnect.due(t2 + ms(500)), "a success resets the backoff");
}

#[test]
fn the_header_says_reconnecting_instead_of_the_session_while_disconnected() {
    let mut app = app();
    assert!(app.connected());
    let header = first_line(&render(&mut app, 80, 24));
    assert!(header.contains("linked"), "{header}");
    assert!(!header.contains("reconnecting"), "{header}");

    app.set_connected(false);
    let header = first_line(&render(&mut app, 80, 24));
    assert!(header.contains("reconnecting…"), "{header}");
    assert!(!header.contains("linked"), "{header}");

    app.set_connected(true);
    let header = first_line(&render(&mut app, 80, 24));
    assert!(header.contains("linked"), "{header}");
}

// --- a fake daemon that restarts ------------------------------------------------------------

static NEXT: AtomicU32 = AtomicU32::new(0);

fn socket_path() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ntrc-{}-{n}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn send_line(stream: &mut UnixStream, value: Value) {
    stream
        .write_all(format!("{value}\n").as_bytes())
        .expect("write");
}

/// One life of the daemon: the request connection and the subscribed event connection.
struct Life {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: UnixStream,
    _events_reader: BufReader<UnixStream>,
}

impl Life {
    /// Accept both connections in the order the TUI makes them: requests (hello), then events
    /// (hello, subscribe).
    fn accept(listener: &UnixListener) -> Self {
        let hello_ok = json!({"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0});
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        // A TUI that never asks makes the script fail instead of hang.
        requests
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        assert!(line.contains("\"hello\""), "{line}");
        send_line(&mut writer, hello_ok.clone());
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        assert!(line.contains("\"hello\""), "{line}");
        send_line(&mut events_writer, hello_ok);
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        assert!(line.contains("\"subscribe\""), "{line}");
        send_line(&mut events_writer, json!({"v":1,"op":"subscribed"}));
        Self {
            reader,
            writer,
            events: events_writer,
            _events_reader: events_reader,
        }
    }

    /// Answer the next request as a daemon with Ada and Grace would, returning it.
    fn serve(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("a request");
        let request: Value = serde_json::from_str(&line).expect("json");
        let reply = match request["op"].as_str() {
            Some("status") => json!({"v":1,"op":"status","daemon":"fake/2","linked":true,
                "session":"linked","unread":0}),
            Some("chats") => json!({"v":1,"op":"chats","chats":[chat_json(ADA, "Ada Lovelace"),
                chat_json(GRACE, "Grace Hopper")]}),
            Some("messages") => json!({"v":1,"op":"messages","jid":request["jid"],"messages":[
                message_json("3EB0B", "the newest one", 31),
                message_json("3EB0A", "an older one", 30)]}),
            Some("mark_read") => {
                json!({"v":1,"op":"marked_read","jid":request["jid"],"unread":0})
            }
            other => panic!("unexpected request {other:?}"),
        };
        send_line(&mut self.writer, reply);
        request
    }

    fn push(&mut self, event: Value) {
        send_line(&mut self.events, event);
    }
}

fn chat_json(jid: &str, name: &str) -> Value {
    json!({"jid":jid,"name":name,"unread":0,"archived":false,"pinned":false,"muted":false})
}

fn message_json(id: &str, text: &str, minute: u32) -> Value {
    json!({"id":id,"kind":"text","text":text,"from_me":false,
        "timestamp":format!("2026-10-18T17:{minute:02}:00Z")})
}

/// The daemon goes away: its socket first (so nothing reconnects to the old life), then both
/// connections. It comes back on the same path after `down`.
fn restart(listener: UnixListener, life: Life, path: &Path, down: Duration) -> UnixListener {
    drop(listener);
    let _ = std::fs::remove_file(path);
    drop(life);
    thread::sleep(down);
    UnixListener::bind(path).expect("bind again")
}

// --- the TUI's side --------------------------------------------------------------------------

fn chat(jid: &str, name: &str) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: name.to_owned(),
        unread: 0,
        archived: false,
        pinned: false,
        muted: false,
        last_message: None,
    }
}

fn app() -> App {
    let snapshot = Snapshot {
        daemon: "fake".to_owned(),
        linked: true,
        session: Session::Linked,
        unread: 0,
        chats: vec![chat(ADA, "Ada Lovelace"), chat(GRACE, "Grace Hopper")],
    };
    App::new(snapshot, Source::Daemon, Palette::default()).with_client_prefix("t")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        assert_eq!(app.handle_key(key(KeyCode::Char(c))), None);
    }
}

fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draws");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()))
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn first_line(screen: &str) -> String {
    screen.lines().next().unwrap_or_default().to_owned()
}

fn last_line(screen: &str) -> String {
    screen.lines().last().unwrap_or_default().to_owned()
}

/// Open Ada's chat through the daemon.
fn open_ada(daemon: &mut Daemon, app: &mut App) {
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter opens the chat");
    };
    daemon.perform(app, &frame).expect("messages");
    assert!(render(app, 100, 24).contains("the newest one"));
}

/// Drain until `done` holds for the screen, recording whether the header ever said
/// `reconnecting…` on the way.
fn drain_until(
    daemon: &mut Daemon,
    app: &mut App,
    what: &str,
    done: impl Fn(&str) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_reconnecting = false;
    loop {
        let _ = daemon.drain_events(app);
        let screen = render(app, 100, 24);
        saw_reconnecting |= first_line(&screen).contains("reconnecting…");
        if done(&screen) {
            return saw_reconnecting;
        }
        assert!(
            Instant::now() < deadline,
            "{what} never happened:\n{screen}"
        );
        thread::sleep(ms(5));
    }
}

#[test]
fn after_a_daemon_restart_the_tui_reconnects_resyncs_and_keeps_the_draft_and_selection() {
    let path = socket_path();
    let listener = UnixListener::bind(&path).expect("bind");
    let (ready, go) = mpsc::channel::<()>();
    let (finished, done) = mpsc::channel::<()>();
    let server_path = path.clone();
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener);
        assert_eq!(first.serve()["op"], "messages");
        go.recv().expect("the TUI is ready");
        let listener = restart(listener, first, &server_path, ms(300));
        let mut second = Life::accept(&listener);
        let ops: Vec<Value> = (0..3).map(|_| second.serve()).collect();
        second.push(json!({"v":1,"op":"event_message","jid":ADA,"message":
            message_json("3EB0C", "after the restart", 32)}));
        // A new message in the open chat is marked read, on the new connection.
        let mut ops = ops;
        ops.push(second.serve());
        let _ = done.recv_timeout(Duration::from_secs(5));
        ops
    });

    let mut daemon = Daemon::connect(&path, "nuncio-tui/test")
        .expect("connect")
        .with_backoff(Backoff::new(ms(20), ms(80)));
    let mut app = app();
    open_ada(&mut daemon, &mut app);
    // Select the newest message, then write a draft.
    app.handle_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "half a thought");
    ready.send(()).expect("server");

    let saw_reconnecting = drain_until(&mut daemon, &mut app, "the event after the restart", |s| {
        s.contains("after the restart")
    });
    assert!(saw_reconnecting, "the header never said reconnecting…");
    let screen = render(&mut app, 100, 24);
    assert!(daemon.is_connected());
    assert!(app.connected());
    assert!(
        first_line(&screen).contains("fake/2 · linked"),
        "the status was resynced:\n{screen}"
    );
    assert!(!first_line(&screen).contains("reconnecting"), "{screen}");
    assert!(
        last_line(&screen).contains("reconnected to nunciod"),
        "{screen}"
    );
    // The draft, the open chat and the selected message are all still there.
    assert_eq!(app.composer(), "half a thought");
    assert_eq!(app.pane(), Pane::Compose);
    assert_eq!(app.selected_jid(), Some(ADA));
    app.handle_key(key(KeyCode::Esc));
    let footer = last_line(&render(&mut app, 120, 24));
    assert!(
        footer.contains("r reply"),
        "the selected message was lost: {footer}"
    );

    finished.send(()).expect("server");
    let ops = server.join().expect("the fake daemon ran its script");
    let ops: Vec<(&str, &str)> = ops
        .iter()
        .map(|op| {
            (
                op["op"].as_str().unwrap_or_default(),
                op["jid"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        ops,
        [
            ("status", ""),
            ("chats", ""),
            ("messages", ADA),
            ("mark_read", ADA)
        ]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_send_while_disconnected_is_not_sent_says_so_and_is_never_retried() {
    let path = socket_path();
    let listener = UnixListener::bind(&path).expect("bind");
    let (ready, go) = mpsc::channel::<()>();
    let (finished, done) = mpsc::channel::<()>();
    let server_path = path.clone();
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener);
        assert_eq!(first.serve()["op"], "messages");
        // Down until the TUI has tried its send.
        drop(listener);
        let _ = std::fs::remove_file(&server_path);
        drop(first);
        go.recv().expect("the TUI sent");
        let listener = UnixListener::bind(&server_path).expect("bind again");
        let mut second = Life::accept(&listener);
        let mut ops: Vec<Value> = (0..3).map(|_| second.serve()).collect();
        // Anything more the TUI sends (it must send nothing) is recorded too.
        second
            .reader
            .get_ref()
            .set_read_timeout(Some(ms(300)))
            .expect("timeout");
        let mut line = String::new();
        while second.reader.read_line(&mut line).unwrap_or(0) > 0 {
            ops.push(serde_json::from_str(&line).expect("json"));
            line.clear();
        }
        let _ = done.recv_timeout(Duration::from_secs(5));
        ops
    });

    let mut daemon = Daemon::connect(&path, "nuncio-tui/test")
        .expect("connect")
        .with_backoff(Backoff::new(ms(20), ms(80)));
    let mut app = app();
    open_ada(&mut daemon, &mut app);
    drain_until(&mut daemon, &mut app, "noticing the lost daemon", |s| {
        first_line(s).contains("reconnecting…")
    });
    assert!(!daemon.is_connected());

    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "while it is down");
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter sends");
    };
    let _ = daemon.perform(&mut app, &frame);
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("while it is down"), "{screen}");
    assert!(screen.contains("not sent"), "{screen}");
    assert!(
        last_line(&screen).contains("not connected to nunciod"),
        "{screen}"
    );
    ready.send(()).expect("server");

    drain_until(&mut daemon, &mut app, "the reconnect", |s| {
        last_line(s).contains("reconnected to nunciod")
    });
    let screen = render(&mut app, 100, 24);
    assert!(
        screen.contains("while it is down") && screen.contains("not sent"),
        "the failed send must stay visible after the resync:\n{screen}"
    );

    finished.send(()).expect("server");
    let ops = server.join().expect("the fake daemon ran its script");
    let ops: Vec<&str> = ops
        .iter()
        .map(|op| op["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ops, ["status", "chats", "messages"], "nothing is retried");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_request_that_timed_out_reconnects_instead_of_leaving_the_tui_without_a_daemon() {
    let path = socket_path();
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener);
        // The first `messages` gets no answer within the TUI's read bound.
        let mut line = String::new();
        first.reader.read_line(&mut line).expect("messages");
        assert!(line.contains("\"messages\""), "{line}");
        // The same daemon, still up: the TUI comes back on new connections.
        let mut second = Life::accept(&listener);
        let ops: Vec<Value> = (0..3).map(|_| second.serve()).collect();
        drop(first);
        thread::sleep(ms(300));
        ops
    });

    let mut daemon = Daemon::connect_with_timeout(&path, "nuncio-tui/test", ms(150))
        .expect("connect")
        .with_backoff(Backoff::new(ms(20), ms(80)));
    let mut app = app();
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter opens the chat");
    };
    assert!(
        daemon.perform(&mut app, &frame).is_err(),
        "the read timed out"
    );
    assert!(!daemon.is_connected());
    drain_until(&mut daemon, &mut app, "the reconnect", |s| {
        s.contains("the newest one")
    });
    assert!(daemon.is_connected());
    let ops = server.join().expect("the fake daemon ran its script");
    let ops: Vec<&str> = ops
        .iter()
        .map(|op| op["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ops, ["status", "chats", "messages"]);
    let _ = std::fs::remove_file(&path);
}
