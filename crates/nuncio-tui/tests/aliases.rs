//! One chat per contact (ADR 0001), through the real driver: the daemon names the phone-number
//! chats it merged into a LID chat in `aliases` (in `chats` and in every `event_chat`), and the
//! TUI moves everything it holds for such a number to the LID chat. A draft never changes
//! recipient. Fake numbers and LIDs only.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::{Chat, ClientFrame, DaemonFrame, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Backoff, Daemon, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

const ADA_PN: &str = "15551230001@s.whatsapp.net";
const ADA_LID: &str = "100000000000001@lid";
const GRACE: &str = "15551230002@s.whatsapp.net";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// --- a fake daemon ---------------------------------------------------------------------------

static NEXT: AtomicU32 = AtomicU32::new(0);

fn socket_path() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ntal-{}-{n}.sock", std::process::id()));
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

fn send_line(stream: &mut UnixStream, value: &Value) {
    stream
        .write_all(format!("{value}\n").as_bytes())
        .expect("write");
}

/// One life of the daemon: the request connection and the subscribed event connection. Once
/// `merged`, it serves Ada as her LID chat with her number as its alias.
struct Life {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: UnixStream,
    _events_reader: BufReader<UnixStream>,
    merged: bool,
    /// When set, the `chats` reply and the chat a request about Ada's number is served as.
    served: Option<(Value, &'static str)>,
}

impl Life {
    fn accept(listener: &UnixListener, merged: bool) -> Self {
        let hello_ok = json!({"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0});
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        requests
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        send_line(&mut writer, &hello_ok);
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        send_line(&mut events_writer, &hello_ok);
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        assert!(line.contains("\"subscribe\""), "{line}");
        send_line(&mut events_writer, &json!({"v":1,"op":"subscribed"}));
        Self {
            reader,
            writer,
            events: events_writer,
            _events_reader: events_reader,
            merged,
            served: None,
        }
    }

    /// Serve `chats` as the chat list and Ada's number as the chat `lid`.
    fn serving(mut self, chats: Value, lid: &'static str) -> Self {
        self.served = Some((chats, lid));
        self
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("a request");
        serde_json::from_str(&line).expect("json")
    }

    /// The chat a request about `jid` is served as.
    fn canonical(&self, jid: &Value) -> Value {
        if let Some((_, lid)) = &self.served
            && jid == ADA_PN
        {
            json!(lid)
        } else if self.merged && jid == ADA_PN {
            json!(ADA_LID)
        } else {
            jid.clone()
        }
    }

    fn reply(&mut self, value: &Value) {
        send_line(&mut self.writer, value);
    }

    /// Answer the next request, returning it.
    fn serve(&mut self) -> Value {
        let request = self.read();
        let reply = match request["op"].as_str() {
            Some("status") => json!({"v":1,"op":"status","daemon":"fake/2","linked":true,
                "session":"linked","unread":0}),
            Some("chats") if self.served.is_some() => {
                json!({"v":1,"op":"chats","chats":self.served.as_ref().map(|(c, _)| c)})
            }
            // Grace first: a selection that falls to the first chat lands on her.
            Some("chats") if self.merged => json!({"v":1,"op":"chats","chats":[
                chat_json(GRACE, "Grace Hopper"), ada_lid_json(json!(null))]}),
            Some("chats") => json!({"v":1,"op":"chats","chats":[chat_json(ADA_PN, "Ada Lovelace"),
                chat_json(GRACE, "Grace Hopper")]}),
            Some("messages") => json!({"v":1,"op":"messages",
                "jid":self.canonical(&request["jid"]),"messages":[
                message_json("3EB0B", "the newest one", 31),
                message_json("3EB0A", "an older one", 30)]}),
            Some("send") => sent_json(&request["client_id"], "3EB0S1"),
            Some("mark_read") => {
                json!({"v":1,"op":"marked_read","jid":self.canonical(&request["jid"]),"unread":0})
            }
            other => panic!("unexpected request {other:?}"),
        };
        self.reply(&reply);
        request
    }

    fn push(&mut self, event: &Value) {
        send_line(&mut self.events, event);
    }
}

fn chat_json(jid: &str, name: &str) -> Value {
    json!({"jid":jid,"name":name,"unread":0,"archived":false,"pinned":false,"muted":false})
}

/// Ada's LID chat with her number merged into it, and `last` as its newest message.
fn ada_lid_json(last: Value) -> Value {
    let mut chat = chat_json(ADA_LID, "Ada Lovelace");
    chat["aliases"] = json!([ADA_PN]);
    if !last.is_null() {
        chat["last_message"] = last;
    }
    chat
}

fn message_json(id: &str, text: &str, minute: u32) -> Value {
    json!({"id":id,"kind":"text","text":text,"from_me":false,
        "timestamp":format!("2026-10-18T17:{minute:02}:00Z")})
}

fn sent_json(client_id: &Value, id: &str) -> Value {
    json!({"v":1,"op":"sent","client_id":client_id,"state":"sent","id":id,
        "timestamp":"2026-10-18T17:40:00Z"})
}

fn mine_json(id: &str, text: &str, client_id: &str) -> Value {
    json!({"id":id,"kind":"text","text":text,"from_me":true,
        "timestamp":"2026-10-18T17:40:00Z","client_id":client_id})
}

fn restart(listener: UnixListener, life: Life, path: &Path) -> UnixListener {
    drop(listener);
    let _ = std::fs::remove_file(path);
    drop(life);
    thread::sleep(ms(300));
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
        aliases: Vec::new(),
        phone: None,
        last_message: None,
    }
}

fn app_with(chats: Vec<Chat>) -> App {
    let snapshot = Snapshot {
        daemon: "fake".to_owned(),
        linked: true,
        session: Session::Linked,
        unread: 0,
        chats,
    };
    App::new(snapshot, Source::Daemon, Palette::default()).with_client_prefix("t")
}

fn app() -> App {
    app_with(vec![
        chat(ADA_PN, "Ada Lovelace"),
        chat(GRACE, "Grace Hopper"),
    ])
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
    let (width, height) = (100, 24);
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

fn connect(path: &Path) -> Daemon {
    Daemon::connect(path, "nuncio-tui/test")
        .expect("connect")
        .with_backoff(Backoff::new(ms(20), ms(80)))
}

/// Open the selected chat through the daemon (Enter, which focuses the composer), then leave the
/// composer for the conversation (Esc).
fn open(daemon: &mut Daemon, app: &mut App) {
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter opens the chat");
    };
    assert_eq!(app.pane(), Pane::Compose);
    daemon.perform(app, &frame).expect("messages");
    assert!(render(app).contains("the newest one"));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Messages);
}

/// Drain events until `done` holds, then a little longer, so later events are applied too.
fn drain_until(daemon: &mut Daemon, app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        let _ = daemon.drain_events(app);
        assert!(Instant::now() < deadline, "{what} never happened");
        thread::sleep(ms(5));
    }
    for _ in 0..10 {
        let _ = daemon.drain_events(app);
        thread::sleep(ms(5));
    }
}

/// Type `text` in the composer and send it through the daemon, returning the frame sent.
fn send(daemon: &mut Daemon, app: &mut App, text: &str) -> ClientFrame {
    app.handle_key(key(KeyCode::Char('i')));
    type_text(app, text);
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter sends");
    };
    daemon.perform(app, &frame).expect("sent");
    frame
}

fn ada_entries(app: &App) -> usize {
    app.chats()
        .iter()
        .filter(|c| c.name == "Ada Lovelace")
        .count()
}

fn ops(requests: &[Value]) -> Vec<(String, String)> {
    requests
        .iter()
        .map(|op| {
            (
                op["op"].as_str().unwrap_or_default().to_owned(),
                op["jid"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

// --- F1: the daemon restarts with the merge already done --------------------------------------

#[test]
fn a_draft_kept_in_the_chat_list_goes_to_the_lid_chat_after_a_restart_that_merged_it() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (ready, go) = mpsc::channel::<()>();
    let server_path = path.clone();
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener, false);
        assert_eq!(first.serve()["op"], "messages");
        go.recv().expect("the TUI is ready");
        let listener = restart(listener, first, &server_path);
        let mut second = Life::accept(&listener, true);
        // status, chats, messages of the selected chat, then the send.
        (0..4).map(|_| second.serve()).collect::<Vec<Value>>()
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "for ada only");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.pane(), Pane::Chats);
    assert_eq!(app.composer(), "for ada only");
    ready.send(()).expect("server");

    drain_until(&mut daemon, &mut app, "the reconnect", |a| {
        a.connected() && a.selected_jid() != Some(ADA_PN)
    });
    assert_eq!(
        app.selected_jid(),
        Some(ADA_LID),
        "the selection followed Ada"
    );
    assert_eq!(ada_entries(&app), 1);
    assert_eq!(app.composer(), "for ada only", "the draft is kept");
    let frame = send(&mut daemon, &mut app, "");
    assert!(
        matches!(&frame, ClientFrame::Send { jid, text, .. } if jid == ADA_LID && text == "for ada only"),
        "the draft went to {frame:?}"
    );

    let requests = server.join().expect("the fake daemon ran its script");
    assert_eq!(
        ops(&requests),
        pairs(&[
            ("status", ""),
            ("chats", ""),
            // Asked at the reconnect for the chat then selected; answered as the LID chat.
            ("messages", ADA_PN),
            ("send", ADA_LID)
        ])
    );
    let _ = std::fs::remove_file(&path);
}

// --- F4: the chat Ada's number became is not listed ------------------------------------------

const BOB: &str = "15551230003@s.whatsapp.net";
const ADA_LID_OLD: &str = "100000000000009@lid";
const NEW_LID: &str = "100000000000002@lid";

/// Ada's number chat open from `chats` (selected at `index`) with a draft written in Compose; the
/// daemon restarts serving `listed` as the chat list and Ada's number as the chat `lid`. Enter
/// must send nothing: the chat it became is not listed, and the selection's old index is now
/// another contact.
fn restart_unlisted(chats: Vec<Chat>, index: usize, listed: Value, lid: &'static str) {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (ready, go) = mpsc::channel::<()>();
    let (served, seen) = mpsc::channel::<Vec<Value>>();
    let (finished, done) = mpsc::channel::<()>();
    let server_path = path.clone();
    let server = thread::spawn(move || {
        let mut first = Life::accept(&listener, false);
        assert_eq!(first.serve()["op"], "messages");
        go.recv().expect("the TUI is ready");
        let listener = restart(listener, first, &server_path);
        let mut second = Life::accept(&listener, true).serving(listed, lid);
        // status, chats, then the messages of the selected chat.
        let requests = (0..3).map(|_| second.serve()).collect::<Vec<Value>>();
        served.send(requests).expect("the test");
        // Stay up until the test is done, so the TUI stays connected.
        let _ = done.recv();
    });

    let mut daemon = connect(&path);
    let mut app = app_with(chats);
    while app.selected() < index {
        app.handle_key(key(KeyCode::Char('j')));
    }
    assert_eq!(app.selected_jid(), Some(ADA_PN));
    open(&mut daemon, &mut app);
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "for ada only");
    assert_eq!(app.pane(), Pane::Compose);
    ready.send(()).expect("server");

    drain_until(&mut daemon, &mut app, "the reconnect", |a| {
        a.connected() && a.selected_jid() != Some(ADA_PN)
    });
    let requests = seen
        .recv_timeout(Duration::from_secs(5))
        .expect("the fake daemon ran its script");
    assert_eq!(
        ops(&requests),
        pairs(&[("status", ""), ("chats", ""), ("messages", ADA_PN)])
    );
    assert_eq!(app.pane(), Pane::Compose);
    let action = app.handle_key(key(KeyCode::Enter));
    assert!(
        !matches!(&action, Some(Action::Request(ClientFrame::Send { .. }))),
        "the draft went to {action:?}"
    );
    assert_eq!(action, None, "Enter sends nothing");
    assert_eq!(app.selected_jid(), Some(lid), "the selection followed Ada");
    assert_eq!(app.composer(), "for ada only", "the draft is intact");
    assert!(render(&mut app).contains("merged"));
    finished.send(()).expect("server");
    server.join().expect("the fake daemon");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_draft_never_goes_to_another_contact_when_the_lid_chat_is_not_listed_after_a_restart() {
    // More than CHAT_LIMIT chats: Ada's LID chat is too old to be listed.
    restart_unlisted(
        vec![
            chat(GRACE, "Grace Hopper"),
            chat(BOB, "Bob"),
            chat(ADA_PN, "Ada Lovelace"),
        ],
        2,
        json!([chat_json(GRACE, "Grace Hopper"), chat_json(BOB, "Bob")]),
        ADA_LID,
    );
}

#[test]
fn a_draft_never_goes_to_a_recycled_numbers_old_lid_chat_after_a_restart() {
    // Ada's number now belongs to a new LID; its previous owner's LID chat is listed, unaliased.
    restart_unlisted(
        vec![chat(GRACE, "Grace Hopper"), chat(ADA_PN, "Ada Lovelace")],
        1,
        json!([
            chat_json(GRACE, "Grace Hopper"),
            chat_json(ADA_LID_OLD, "Ada Lovelace")
        ]),
        NEW_LID,
    );
}

// --- F2: a live merge -------------------------------------------------------------------------

#[test]
fn a_live_merge_moves_the_open_chat_to_the_lid_chat_with_its_reply_and_later_sends() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (ready, go) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut life = Life::accept(&listener, false);
        assert_eq!(life.serve()["op"], "messages");
        go.recv().expect("the TUI is ready");
        life.merged = true;
        let reply = message_json("3EB0R1", "reply from ada", 35);
        life.push(&json!({"v":1,"op":"event_chat","chat":ada_lid_json(reply.clone())}));
        life.push(&json!({"v":1,"op":"event_message","jid":ADA_LID,"message":reply}));
        // The reply is read at once in the open chat: mark_read, then the send.
        let mark_read = life.serve();
        let send = life.serve();
        life.push(&json!({"v":1,"op":"event_message","jid":ADA_LID,
            "message":mine_json("3EB0S1", "see you there", "t-1")}));
        [mark_read, send]
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    ready.send(()).expect("server");
    drain_until(&mut daemon, &mut app, "the merge", |a| {
        a.chats().iter().any(|c| c.jid == ADA_LID)
    });
    assert_eq!(ada_entries(&app), 1, "one Ada: {:?}", app.chats());
    assert_eq!(app.selected_jid(), Some(ADA_LID));
    assert_eq!(app.pane(), Pane::Messages, "the chat stays open");
    let screen = render(&mut app);
    assert!(screen.contains("the newest one"), "{screen}");
    // Once in the list (its newest message) and once in the open conversation.
    assert_eq!(screen.matches("reply from ada").count(), 2, "{screen}");

    let frame = send(&mut daemon, &mut app, "see you there");
    assert!(
        matches!(&frame, ClientFrame::Send { jid, .. } if jid == ADA_LID),
        "{frame:?}"
    );
    drain_until(&mut daemon, &mut app, "the send's event", |a| {
        a.chats()
            .iter()
            .any(|c| c.last_message.as_ref().is_some_and(|m| m.id == "3EB0S1"))
    });
    let screen = render(&mut app);
    assert_eq!(screen.matches("see you there").count(), 2, "{screen}");
    assert_eq!(ada_entries(&app), 1);
    let requests = server.join().expect("the fake daemon ran its script");
    assert_eq!(
        ops(&requests),
        pairs(&[("mark_read", ADA_LID), ("send", ADA_LID)])
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_send_to_the_number_answered_before_the_merge_events_lands_in_the_open_lid_chat() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let mut life = Life::accept(&listener, false);
        assert_eq!(life.serve()["op"], "messages");
        // The send is what teaches the daemon the LID: it merges, publishes, then answers.
        let send = life.read();
        assert_eq!(send["jid"], ADA_PN);
        let mine = mine_json("3EB0S1", "lunch?", "t-1");
        life.push(&json!({"v":1,"op":"event_chat","chat":ada_lid_json(mine.clone())}));
        life.push(&json!({"v":1,"op":"event_message","jid":ADA_LID,"message":mine}));
        life.reply(&sent_json(&send["client_id"], "3EB0S1"));
    });

    let mut daemon = connect(&path);
    let mut app = app();
    open(&mut daemon, &mut app);
    send(&mut daemon, &mut app, "lunch?");
    drain_until(&mut daemon, &mut app, "the merge", |a| {
        a.chats().iter().any(|c| c.jid == ADA_LID)
    });
    server.join().expect("the fake daemon ran its script");
    assert_eq!(ada_entries(&app), 1, "one Ada: {:?}", app.chats());
    assert_eq!(app.selected_jid(), Some(ADA_LID));
    let screen = render(&mut app);
    assert!(screen.contains("the newest one"), "{screen}");
    assert_eq!(screen.matches("lunch?").count(), 2, "{screen}");
    let _ = std::fs::remove_file(&path);
}

// --- the reducer alone ------------------------------------------------------------------------

fn lid_with_alias() -> Chat {
    Chat {
        aliases: vec![ADA_PN.to_owned()],
        ..chat(ADA_LID, "Ada Lovelace")
    }
}

/// The `messages` reply for `jid`: one message, "loaded before".
fn messages_reply(jid: &str) -> DaemonFrame {
    let message =
        serde_json::from_value(message_json("3EB0L", "loaded before", 20)).expect("message");
    DaemonFrame::Messages {
        v: 1,
        jid: jid.to_owned(),
        messages: vec![message],
    }
}

/// Ada's number chat open, its messages loaded, with `draft` written and the pane left in `pane`.
fn ada_open_with_draft(draft: &str, pane: Pane) -> App {
    let mut app = app();
    let Some(Action::Request(request)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter opens the chat");
    };
    app.apply_reply(&request, &messages_reply(ADA_PN));
    assert_eq!(
        app.pane(),
        Pane::Compose,
        "Enter opens the chat ready to write"
    );
    type_text(&mut app, draft);
    app.handle_key(key(KeyCode::Esc));
    if pane == Pane::Chats {
        app.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(app.pane(), pane);
    app
}

#[test]
fn a_chat_list_without_alias_info_never_moves_a_draft_from_the_chat_list_to_another_chat() {
    let mut app = ada_open_with_draft("for ada only", Pane::Chats);
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace Hopper"), chat(ADA_LID, "Ada Lovelace")],
    });
    assert_ne!(
        app.selected_jid(),
        Some(GRACE),
        "the draft would go to Grace"
    );
    assert_eq!(
        app.selected_jid(),
        Some(ADA_PN),
        "the vanished chat stays selected"
    );
    app.handle_key(key(KeyCode::Char('i')));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        None,
        "Enter sends nothing"
    );
    assert!(render(&mut app).contains("merged"));
    assert_eq!(app.composer(), "for ada only");
}

#[test]
fn without_a_draft_a_vanished_chat_in_the_chat_list_moves_the_selection_as_before() {
    let mut app = ada_open_with_draft("", Pane::Chats);
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace Hopper")],
    });
    assert_eq!(app.selected_jid(), Some(GRACE));
}

#[test]
fn an_event_chat_naming_the_vanished_chat_as_its_alias_moves_the_draft_there() {
    let mut app = ada_open_with_draft("for ada only", Pane::Messages);
    app.handle_key(key(KeyCode::Char('i')));
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace Hopper")],
    });
    assert_eq!(app.selected_jid(), Some(ADA_PN), "the orphan is kept");
    app.apply(&DaemonFrame::EventChat {
        v: 1,
        chat: lid_with_alias(),
    });
    assert_eq!(app.selected_jid(), Some(ADA_LID));
    assert!(
        !render(&mut app).contains("merged"),
        "the notice is cleared"
    );
    let sent = app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(&sent, Some(Action::Request(ClientFrame::Send { jid, text, .. }))
            if jid == ADA_LID && text == "for ada only"),
        "{sent:?}"
    );
}

#[test]
fn a_number_listed_as_its_own_chat_is_never_adopted_even_if_another_chat_names_it() {
    // A recycled number: the daemon lists it as its own chat. Its alias on the old contact's chat
    // (which the daemon does not list) must not move the open chat either.
    let mut app = ada_open_with_draft("for the new owner", Pane::Messages);
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![
            lid_with_alias(),
            chat(ADA_PN, "Someone new"),
            chat(GRACE, "Grace Hopper"),
        ],
    });
    assert_eq!(app.selected_jid(), Some(ADA_PN));
    assert_eq!(app.chats().len(), 3);
    let screen = render(&mut app);
    assert!(
        screen.contains("loaded before"),
        "the open chat kept its conversation:\n{screen}"
    );
    app.handle_key(key(KeyCode::Char('i')));
    let sent = app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(&sent, Some(Action::Request(ClientFrame::Send { jid, .. })) if jid == ADA_PN),
        "{sent:?}"
    );
}

#[test]
fn an_alias_named_later_moves_a_conversation_the_tui_only_holds_in_the_background() {
    // Ada's number chat was loaded, then the user moved to Grace, and a list without alias info
    // dropped Ada's number. The event naming it as an alias moves her loaded conversation.
    let mut app = ada_open_with_draft("", Pane::Chats);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_jid(), Some(GRACE));
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace Hopper")],
    });
    app.apply(&DaemonFrame::EventChat {
        v: 1,
        chat: lid_with_alias(),
    });
    assert_eq!(
        app.selected_jid(),
        Some(GRACE),
        "the selection stays on Grace"
    );
    assert_eq!(app.chats().len(), 2);
    let ada = app
        .chats()
        .iter()
        .position(|c| c.jid == ADA_LID)
        .expect("Ada is listed");
    while app.selected() != ada {
        app.handle_key(key(if app.selected() < ada {
            KeyCode::Char('j')
        } else {
            KeyCode::Char('k')
        }));
    }
    let Some(Action::Request(ClientFrame::Messages { jid, .. })) =
        app.handle_key(key(KeyCode::Enter))
    else {
        panic!("Enter opens the chat");
    };
    assert_eq!(jid, ADA_LID);
    let screen = render(&mut app);
    assert!(
        screen.contains("loaded before"),
        "her conversation moved with her:\n{screen}"
    );
}

#[test]
fn enter_on_a_vanished_chat_in_the_chat_list_opens_the_composer_but_sends_nothing() {
    let mut app = ada_open_with_draft("for ada only", Pane::Chats);
    app.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace Hopper"), chat(ADA_LID, "Ada Lovelace")],
    });
    assert_eq!(
        app.selected_jid(),
        Some(ADA_PN),
        "the vanished chat stays selected"
    );
    // Enter on the chat list opens the vanished chat with the composer focused, as on any chat.
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(ADA_PN, 200)))
    );
    assert_eq!(app.pane(), Pane::Compose);
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        None,
        "Enter sends nothing"
    );
    assert_eq!(app.composer(), "for ada only");
    assert!(render(&mut app).contains("merged"));
}
