//! D1: a send (or retry) is awaited up to 25 s off the TUI's loop. Times are scaled (40 ms stand
//! for 1 s): the read bound is 200 ms ("5 s"), the send wait 1 s ("25 s"), and the fake daemon
//! answers at 240 ms ("6 s") or 760 ms ("19 s"). Meanwhile the loop keeps ticking and the user keeps
//! working; each answer lands on its own message, in order, and nothing is sent twice.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::{Chat, ClientFrame, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Backoff, Daemon, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

const ADA: &str = "15551230001@s.whatsapp.net";
const GRACE: &str = "15551230002@s.whatsapp.net";
const READ_BOUND: Duration = Duration::from_millis(200);
const SEND_WAIT: Duration = Duration::from_millis(1000);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// --- the fake daemon -------------------------------------------------------------------------

static NEXT: AtomicU32 = AtomicU32::new(0);

fn socket_path() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ntsd-{}-{n}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// Removes the socket file when dropped, so a failed test leaves nothing in /tmp.
struct Socket(PathBuf);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn hello_ok() -> Value {
    json!({"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0})
}

/// One connection of the fake daemon, handshaken.
struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    fn accept(listener: &UnixListener) -> Self {
        let (stream, _) = listener.accept().expect("a connection");
        stream.set_read_timeout(Some(ms(5000))).expect("timeout");
        let writer = stream.try_clone().expect("clone");
        let mut peer = Self {
            reader: BufReader::new(stream),
            writer,
        };
        assert_eq!(peer.read()["op"], "hello");
        peer.write(&hello_ok());
        peer
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("a line");
        serde_json::from_str(&line).expect("json")
    }

    fn write(&mut self, value: &Value) {
        self.writer
            .write_all(format!("{value}\n").as_bytes())
            .expect("write");
    }

    /// Nothing more arrives within `wait` (EOF or silence).
    fn nothing_more(&mut self, wait: Duration) -> bool {
        let _ = self.reader.get_ref().set_read_timeout(Some(wait));
        let mut line = String::new();
        matches!(self.reader.read_line(&mut line), Ok(0) | Err(_)) && line.is_empty()
    }
}

/// One life: the request connection, and the event connection subscribed.
struct Life {
    requests: Peer,
    _events: Peer,
}

impl Life {
    fn accept(listener: &UnixListener) -> Self {
        let requests = Peer::accept(listener);
        let mut events = Peer::accept(listener);
        assert_eq!(events.read()["op"], "subscribe");
        events.write(&json!({"v":1,"op":"subscribed"}));
        Self {
            requests,
            _events: events,
        }
    }

    /// Answer the next request as a daemon with Ada and Grace would, returning it.
    fn serve(&mut self) -> Value {
        let request = self.requests.read();
        let reply = match request["op"].as_str() {
            Some("status") => json!({"v":1,"op":"status","daemon":"fake/2","linked":true,
                "session":"linked","unread":0}),
            Some("chats") => json!({"v":1,"op":"chats","chats":[chat_json(ADA, "Ada Lovelace"),
                chat_json(GRACE, "Grace Hopper")]}),
            Some("messages") => json!({"v":1,"op":"messages","jid":request["jid"],"messages":[
                {"id":format!("{}-1", request["jid"].as_str().unwrap_or_default()),"kind":"text",
                 "text":"an older one","from_me":false,"timestamp":"2026-10-22T09:00:00Z"}]}),
            other => panic!("unexpected request {other:?}"),
        };
        self.requests.write(&reply);
        request
    }
}

fn chat_json(jid: &str, name: &str) -> Value {
    json!({"jid":jid,"name":name,"unread":0,"archived":false,"pinned":false,"muted":false})
}

fn sent_json(client_id: &Value, id: &str) -> Value {
    json!({"v":1,"op":"sent","client_id":client_id,"state":"sent","id":id,
        "timestamp":"2026-10-22T10:00:00Z"})
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
        aliases: Vec::new(),
        phone: None,
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
        stop_reason: None,
    };
    App::new(snapshot, Source::Daemon, Palette::default()).with_client_prefix("t")
}

fn connect(path: &std::path::Path) -> Daemon {
    Daemon::connect_with_timeout(path, "nuncio-tui/test", READ_BOUND)
        .expect("connect")
        .with_send_wait(SEND_WAIT)
        .with_backoff(Backoff::new(ms(20), ms(80)))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        assert_eq!(app.handle_key(key(KeyCode::Char(c))), None);
    }
}

fn render(app: &mut App) -> String {
    let (width, height) = (120, 24);
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

/// Open the selected chat through the daemon: Enter (the composer is focused), its messages.
fn open(daemon: &mut Daemon, app: &mut App) {
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter opens the chat");
    };
    daemon.perform(app, &frame).expect("messages");
    assert_eq!(app.pane(), Pane::Compose);
}

/// Write `text` and send it, returning its client id; the send is handed off at once.
fn send(daemon: &mut Daemon, app: &mut App, text: &str) -> String {
    if app.pane() != Pane::Compose {
        app.handle_key(key(KeyCode::Char('i')));
    }
    type_text(app, text);
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter sends");
    };
    let ClientFrame::Send { client_id, .. } = &frame else {
        panic!("a send: {frame:?}");
    };
    let started = Instant::now();
    daemon.perform(app, &frame).expect("handed to the outbox");
    assert!(
        started.elapsed() < ms(100),
        "the send blocked the loop for {:?}",
        started.elapsed()
    );
    client_id.clone()
}

/// The message of the selected chat whose text is `text`: its id, and whether it still waits.
fn shown(app: &App, text: &str) -> (String, bool) {
    in_chat(app, app.selected_jid().expect("a chat"), text)
}

/// The message of chat `jid` whose text is `text`, open or not: its id, and whether it waits.
fn in_chat(app: &App, jid: &str, text: &str) -> (String, bool) {
    let transcript = app.model().transcript();
    let message = transcript
        .conversation(jid)
        .unwrap_or_default()
        .iter()
        .find(|m| m.text == text)
        .unwrap_or_else(|| panic!("{text:?} is not in {jid}"));
    (message.id.clone(), transcript.is_sending(message))
}

/// One tick of the loop: drain, and it must not stop for the send in flight.
fn tick(daemon: &mut Daemon, app: &mut App) {
    let started = Instant::now();
    daemon.drain_events(app).expect("drain");
    assert!(
        started.elapsed() < ms(50),
        "a tick waited {:?} for the send",
        started.elapsed()
    );
}

/// Tick until `done`, failing after 5 s.
fn tick_until(daemon: &mut Daemon, app: &mut App, what: &str, done: impl Fn(&mut App) -> bool) {
    let deadline = Instant::now() + ms(5000);
    while !done(app) {
        assert!(Instant::now() < deadline, "{what} never happened");
        tick(daemon, app);
        thread::sleep(ms(5));
    }
}

/// The send's reply comes at "19 s", past the read bound: meanwhile the loop ticks, the user types
/// and opens another chat (its messages come on the request connection), and the message shows as
/// sending; then the reply is applied to that send, in its own chat, as sent.
#[test]
fn a_send_answered_at_19_s_is_sent_and_the_tui_keeps_working_meanwhile() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (opened, grace_open) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut life = Life::accept(&listener);
        assert_eq!(life.serve()["jid"], ADA);
        let mut outbox = Peer::accept(&listener);
        let send = outbox.read();
        let asked = Instant::now();
        assert_eq!(send["op"], "send");
        // Grace's chat is opened while the send waits: answered at once.
        assert_eq!(life.serve()["jid"], GRACE);
        grace_open.recv().expect("the TUI showed Grace");
        thread::sleep(ms(760).saturating_sub(asked.elapsed()));
        outbox.write(&sent_json(&send["client_id"], "3EB0S1"));
        assert!(outbox.nothing_more(ms(200)), "the send went once");
        drop(life);
        send
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    let client_id = send(&mut daemon, &mut app, "on my way");
    assert!(render(&mut app).contains("· sending"));
    assert_eq!(shown(&app, "on my way"), (String::new(), true));
    for _ in 0..5 {
        tick(&mut daemon, &mut app);
    }
    // The user goes on: a draft in the composer, then Grace's chat.
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "and a draft");
    assert_eq!(app.composer(), "and a draft");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.pane(), Pane::Chats);
    app.handle_key(key(KeyCode::Char('j')));
    open(&mut daemon, &mut app);
    assert_eq!(app.selected_jid(), Some(GRACE));
    assert!(render(&mut app).contains("an older one"));
    opened.send(()).expect("the fake daemon");

    // The answer lands on Ada's message while Grace's chat is open.
    tick_until(&mut daemon, &mut app, "the send's answer", |a| {
        !in_chat(a, ADA, "on my way").1
    });
    assert_eq!(app.selected_jid(), Some(GRACE), "Grace's chat stays open");
    assert!(
        !render(&mut app).contains("on my way"),
        "nothing landed in Grace's chat"
    );
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected_jid(), Some(ADA));
    assert_eq!(shown(&app, "on my way"), ("3EB0S1".to_owned(), false));
    let screen = render(&mut app);
    assert!(!screen.contains("not confirmed"), "{screen}");
    assert!(!screen.contains("not sent"), "{screen}");
    assert!(daemon.is_connected());
    let sent = server.join().expect("the fake daemon ran its script");
    assert_eq!(sent["client_id"], client_id.as_str());
}

/// The daemon's own `not_confirmed` (its 20 s, "10 s" here) reaches the TUI as not confirmed, and
/// it is the daemon's answer, not a loss: the TUI stays connected.
#[test]
fn the_daemons_not_confirmed_past_the_read_bound_is_shown_and_keeps_the_connection() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let mut life = Life::accept(&listener);
        life.serve();
        let mut outbox = Peer::accept(&listener);
        assert_eq!(outbox.read()["op"], "send");
        thread::sleep(ms(400));
        outbox.write(&json!({"v":1,"op":"error","code":"not_confirmed",
            "message":"not confirmed: WhatsApp did not acknowledge in time; it may have been delivered"}));
        thread::sleep(ms(300));
        drop(life);
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    send(&mut daemon, &mut app, "on my way");
    tick_until(&mut daemon, &mut app, "the not_confirmed answer", |a| {
        !shown(a, "on my way").1
    });
    let screen = render(&mut app);
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(!screen.contains("not sent"), "{screen}");
    assert!(daemon.is_connected(), "the daemon's answer is no loss");
    server.join().expect("the fake daemon ran its script");
}

/// Two sends in a row: each goes once, in order, and each gets its own answer (one sent at
/// "6 s", one refused); the second waits for the first, never sent twice.
#[test]
fn two_sends_in_a_row_each_get_their_own_answer_in_order() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let mut life = Life::accept(&listener);
        life.serve();
        let mut outbox = Peer::accept(&listener);
        let first = outbox.read();
        thread::sleep(ms(240));
        outbox.write(&sent_json(&first["client_id"], "3EB0S1"));
        let second = outbox.read();
        outbox
            .write(&json!({"v":1,"op":"error","code":"internal","message":"the store is closed"}));
        assert!(outbox.nothing_more(ms(300)), "each send went once");
        drop(life);
        [first, second]
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    let one = send(&mut daemon, &mut app, "first");
    let two = send(&mut daemon, &mut app, "second");
    assert!(shown(&app, "first").1 && shown(&app, "second").1);
    tick_until(&mut daemon, &mut app, "both answers", |a| {
        !shown(a, "first").1 && !shown(a, "second").1
    });
    assert_eq!(shown(&app, "first").0, "3EB0S1");
    let screen = render(&mut app);
    assert!(screen.contains("· not sent"), "{screen}");
    assert_eq!(screen.matches("not sent").count(), 1, "{screen}");
    let [first, second] = server.join().expect("the fake daemon ran its script");
    assert_eq!(
        (first["client_id"].as_str(), second["client_id"].as_str()),
        (Some(one.as_str()), Some(two.as_str())),
        "in the order sent"
    );
}

/// The link is lost and comes back while a send waits: its late answer (here none within the
/// wait) still marks that send, not confirmed, and is no loss of the new link.
#[test]
fn a_send_unanswered_after_a_reconnect_is_not_confirmed_and_the_new_link_stays() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (finished, done) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener);
        first.serve();
        let mut outbox = Peer::accept(&listener);
        assert_eq!(outbox.read()["op"], "send");
        // Both connections of this life go; its outbox connection stays, never answered.
        drop(first);
        let mut second = Life::accept(&listener);
        let ops: Vec<Value> = (0..3).map(|_| second.serve()).collect();
        let _ = done.recv_timeout(ms(5000));
        drop(outbox);
        ops
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    send(&mut daemon, &mut app, "on my way");
    tick_until(&mut daemon, &mut app, "the reconnect", |a| {
        a.model().notice() == Some("reconnected to nunciod")
    });
    assert!(
        shown(&app, "on my way").1,
        "still sending after the reconnect"
    );
    tick_until(&mut daemon, &mut app, "the end of the wait", |a| {
        !shown(a, "on my way").1
    });
    let screen = render(&mut app);
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(
        daemon.is_connected(),
        "an old life's loss is not the new link's"
    );
    assert!(app.connected());
    assert!(!screen.contains("reconnecting"), "{screen}");
    finished.send(()).expect("the fake daemon");
    let ops = server.join().expect("the fake daemon ran its script");
    let ops: Vec<&str> = ops
        .iter()
        .map(|op| op["op"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(ops, ["status", "chats", "messages"], "nothing resent");
}
