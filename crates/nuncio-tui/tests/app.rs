//! M2's TUI core: the conversation view, the composer, key handling and the reducer of wire
//! events, all through `App`'s public interface and a `TestBackend` buffer, plus one round trip
//! through `Daemon` against a scripted fake daemon on a Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::wire::MessageState;
use nuncio_client::{
    Chat, ClientFrame, DaemonFrame, Message, Palette, SendState, Session, Snapshot,
};
use nuncio_tui::{Action, App, Daemon, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const ADA: &str = "15551230001@s.whatsapp.net";
const GRACE: &str = "15551230002@s.whatsapp.net";
const FAMILY: &str = "15551230003@s.whatsapp.net";

fn message(id: &str, text: &str, from_me: bool, minute: u32) -> Message {
    Message {
        id: id.to_owned(),
        kind: "text".to_owned(),
        text: text.to_owned(),
        from_me,
        timestamp: format!("2026-10-18T17:{:02}:00Z", minute % 60),
        media: None,
        state: None,
        client_id: None,
        reply_to: None,
        reactions: Vec::new(),
        thumb: None,
        thumb_mime: None,
        width: None,
        height: None,
        animated: false,
        sender: None,
    }
}

fn chat(jid: &str, name: &str, unread: u32) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: name.to_owned(),
        unread,
        archived: false,
        pinned: false,
        muted: false,
        aliases: Vec::new(),
        phone: None,
        last_message: None,
    }
}

fn app_with(chats: Vec<Chat>) -> App {
    let unread = chats.iter().map(|chat| chat.unread).sum();
    let snapshot = Snapshot {
        daemon: "nunciod/0.1.0".to_owned(),
        linked: true,
        session: Session::Linked,
        unread,
        chats,
        stop_reason: None,
    };
    App::new(snapshot, Source::Daemon, Palette::default())
}

fn two_chats() -> App {
    app_with(vec![
        chat(ADA, "Ada Lovelace", 2),
        chat(GRACE, "Grace Hopper", 0),
    ])
}

/// The `messages` reply for `jid`, newest first as on the wire.
fn messages_reply(jid: &str, oldest_first: Vec<Message>) -> DaemonFrame {
    let mut messages = oldest_first;
    messages.reverse();
    DaemonFrame::Messages {
        v: 1,
        jid: jid.to_owned(),
        messages,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        assert_eq!(app.handle_key(key(KeyCode::Char(c))), None);
    }
}

struct Screen {
    rows: Vec<Vec<String>>,
}

impl Screen {
    fn text(&self) -> String {
        self.rows
            .iter()
            .map(|row| row.concat() + "\n")
            .collect::<String>()
    }

    /// The text of columns `from..to` on every row.
    fn columns(&self, from: usize, to: usize) -> String {
        self.rows
            .iter()
            .map(|row| row[from..to.min(row.len())].concat() + "\n")
            .collect()
    }

    /// The column of the one line between the panes (it meets the header rule with `┬`).
    fn split(&self) -> usize {
        self.rows[1]
            .iter()
            .position(|cell| cell == "┬")
            .expect("the line between the panes")
    }

    /// The chat pane, the line after it included.
    fn chats(&self) -> String {
        self.columns(0, self.split() + 1)
    }

    /// The conversation pane.
    fn conversation(&self) -> String {
        self.columns(self.split() + 1, usize::MAX)
    }

    fn cell(&self, x: u16, y: u16) -> &str {
        &self.rows[usize::from(y)][usize::from(x)]
    }
}

fn render(app: &mut App, width: u16, height: u16) -> Screen {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    let buffer = terminal.backend().buffer();
    let rows = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()).to_owned())
                .collect()
        })
        .collect();
    Screen { rows }
}

/// Open the first chat and load `oldest_first` into it, then leave the composer Enter focused for
/// the conversation (Esc).
fn open_first(app: &mut App, jid: &str, oldest_first: Vec<Message>) {
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(jid, 200))),
        "Enter on a chat asks the daemon for its messages"
    );
    assert_eq!(app.pane(), Pane::Compose, "ready to write");
    let unread = app.chats()[app.selected()].unread;
    // An unread chat is marked read once its messages are in (S8).
    let mark_read = (unread > 0).then(|| Action::Request(ClientFrame::mark_read(jid)));
    assert_eq!(app.apply(&messages_reply(jid, oldest_first)), mark_read);
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(app.pane(), Pane::Messages);
}

#[test]
fn a_conversation_renders_at_eighty_by_twenty_four_and_at_two_forty_by_sixty() {
    for (width, height) in [(80, 24), (240, 60)] {
        let mut app = two_chats();
        open_first(
            &mut app,
            ADA,
            vec![
                message("A1", "Are we still on?", false, 1),
                message("A2", "Yes, see you at 18:00", true, 2),
            ],
        );
        let screen = render(&mut app, width, height).text();
        for needle in [
            "Ada Lovelace (2)",
            "Grace Hopper",
            "Are we still on?",
            "Yes, see you at 18:00",
            "you · 17:02",
            "Ada Lovelace · 17:01",
            "? help",
        ] {
            assert!(
                screen.contains(needle),
                "{needle:?} at {width}x{height}:\n{screen}"
            );
        }
        // Newest at the bottom: the reply is drawn below the question.
        let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
        assert!(row_of("Are we still on?") < row_of("Yes, see you at 18:00"));
    }
}

#[test]
fn chats_show_pin_mute_and_archive_markers_and_archived_chats_sit_last() {
    let mut archived = chat(FAMILY, "Old group", 0);
    archived.archived = true;
    let mut muted = chat(GRACE, "Grace Hopper", 0);
    muted.muted = true;
    let mut pinned = chat(ADA, "Ada Lovelace", 0);
    pinned.pinned = true;
    let mut app = app_with(vec![archived, muted, pinned]);

    let names: Vec<&str> = app.chats().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Grace Hopper", "Ada Lovelace", "Old group"]);
    let screen = render(&mut app, 80, 24).chats();
    let row = |needle: &str| {
        screen
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?}:\n{screen}"))
            .to_owned()
    };
    assert!(row("Grace Hopper").contains('⊘'), "muted:\n{screen}");
    assert!(row("Ada Lovelace").contains('★'), "pinned:\n{screen}");
    assert!(row("Old group").contains('▤'), "archived:\n{screen}");
    assert!(!row("Grace Hopper").contains('★'));
}

#[test]
fn emoji_zwj_cjk_and_rtl_text_keep_the_frame_intact() {
    let family = "👨‍👩‍👧‍👦";
    let mut app = app_with(vec![
        chat(ADA, &format!("{family} Family"), 3),
        chat(GRACE, "李小龍 漢字テスト", 0),
        chat(FAMILY, "שלום עולם", 1),
    ]);
    let long = format!("{family}{family} 中文字符很宽 مرحبا بالعالم שלום ").repeat(12);
    open_first(
        &mut app,
        ADA,
        vec![
            message("A1", &long, false, 1),
            message("A2", "مرحبا 👋🏽 שלום", true, 2),
            quoting_with_reactions("A3", "🏳️‍🌈 漢字", "A1"),
        ],
    );
    for (width, height) in [(80u16, 24u16), (240, 60), (37, 11)] {
        let screen = render(&mut app, width, height);
        // The line between the panes and the conversation's free last column stay in their
        // columns on every body row: no wide character pushed one out.
        let chat_right = screen.split() as u16;
        for y in 2..height - 2 {
            assert!(
                ["│", "├"].contains(&screen.cell(chat_right, y)),
                "the line between the panes at row {y}, {width}x{height}:\n{}",
                screen.text()
            );
            assert!(
                [" ", "─"].contains(&screen.cell(width - 1, y)),
                "the last column at row {y}, {width}x{height}:\n{}",
                screen.text()
            );
        }
        if width >= 80 {
            let text = screen.text();
            assert!(text.contains("Family"), "{text}");
            // Drawn without their joiners and skin tones (see `plain` in lib.rs).
            assert!(text.contains("👋"), "{text}");
            assert!(text.contains("> 👨"), "the quote of A1:\n{text}");
            assert!(text.contains('❤'), "the reaction:\n{text}");
        }
    }
}

/// A message quoting `reply_to`, with my ❤️ and their family-emoji reaction on it.
fn quoting_with_reactions(id: &str, text: &str, reply_to: &str) -> Message {
    let mut quoting = message(id, text, false, 3);
    quoting.reply_to = Some(reply_to.to_owned());
    quoting.reactions = vec![
        nuncio_client::Reaction {
            emoji: "❤️".to_owned(),
            from_me: true,
        },
        nuncio_client::Reaction {
            emoji: "👨‍👩‍👧‍👦".to_owned(),
            from_me: false,
        },
    ];
    quoting
}

fn two_hundred() -> Vec<Message> {
    (0..200)
        .map(|n| {
            message(
                &format!("M{n:03}"),
                &format!("message {n:03}"),
                n % 3 == 0,
                n,
            )
        })
        .collect()
}

/// The CPU time this thread has used: the time it ran, not the time it waited to run.
fn thread_cpu_time() -> Duration {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid, writable timespec, and the clock id is a constant.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    assert_eq!(status, 0, "the thread CPU clock is readable");
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}

#[test]
fn two_hundred_messages_scroll_to_the_top_and_back_quickly() {
    let mut app = two_chats();
    open_first(&mut app, ADA, two_hundred());
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("message 199"), "newest shown:\n{screen}");
    assert!(!screen.contains("message 000"));

    // CPU time of this thread, not wall-clock: on a busy machine (a parallel workspace run)
    // a step waits to be scheduled, and its wall clock was measured past 250 ms while its
    // CPU time stayed under 17 ms (5-14 ms on an idle machine). The bound is 15 times the
    // worst step measured, so only a step that does an order of magnitude more work fails.
    let bound = Duration::from_millis(250);
    let timed = |app: &mut App, code: KeyCode| {
        let start = thread_cpu_time();
        app.handle_key(key(code));
        let screen = render(app, 80, 24).text();
        let took = thread_cpu_time() - start;
        assert!(took < bound, "{code:?} + render took {took:?} of CPU");
        screen
    };

    // Select the newest (it stays in view), then page by page, then all the way.
    let after_k = timed(&mut app, KeyCode::Char('k'));
    assert!(after_k.contains("▌ Ada Lovelace ·") && after_k.contains("message 199"));
    timed(&mut app, KeyCode::PageUp);
    let top = timed(&mut app, KeyCode::Char('g'));
    assert!(top.contains("message 000"), "top:\n{top}");
    assert!(!top.contains("message 199"), "top:\n{top}");
    // Past the top stays at the top.
    let still_top = timed(&mut app, KeyCode::PageUp);
    assert!(still_top.contains("message 000"), "{still_top}");

    timed(&mut app, KeyCode::PageDown);
    let bottom = timed(&mut app, KeyCode::Char('G'));
    assert!(bottom.contains("message 199"), "bottom:\n{bottom}");
    assert!(!bottom.contains("message 000"));
    // Past the bottom stays at the bottom.
    let still = timed(&mut app, KeyCode::PageDown);
    assert!(still.contains("message 199"), "{still}");
}

/// Select the second chat, open it (the composer is focused), write "on my way" and send it.
fn write_and_send(app: &mut App) -> String {
    assert_eq!(app.handle_key(key(KeyCode::Char('j'))), None);
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(GRACE, 200)))
    );
    app.apply(&messages_reply(
        GRACE,
        vec![message("G1", "Ship it?", false, 5)],
    ));
    assert_eq!(app.pane(), Pane::Compose);
    type_text(app, "on my wayy");
    app.handle_key(key(KeyCode::Backspace));
    let Some(Action::Request(ClientFrame::Send {
        jid,
        text,
        client_id,
        ..
    })) = app.handle_key(key(KeyCode::Enter))
    else {
        panic!("Enter in the composer sends");
    };
    assert_eq!(jid, GRACE);
    assert_eq!(text, "on my way");
    assert!(!client_id.is_empty());
    client_id
}

#[test]
fn the_keyboard_flow_sends_one_message_that_goes_from_queued_to_sent() {
    let mut app = two_chats().with_client_prefix("t");
    let client_id = write_and_send(&mut app);
    assert_eq!(client_id, "t-1");

    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("on my way"), "{screen}");
    assert!(screen.contains("sending"), "before the reply:\n{screen}");
    assert_eq!(app.composer(), "", "the composer is cleared");

    // Offline: the daemon queued it.
    app.apply(&DaemonFrame::Sent {
        v: 1,
        client_id: client_id.clone(),
        state: SendState::Queued,
        id: None,
        timestamp: None,
    });
    let screen = render(&mut app, 80, 24).text();
    assert!(
        screen.contains("you · 17:05 · queued") || screen.contains("· queued"),
        "{screen}"
    );

    // The queued event names its WhatsApp id; the drained one comes without state.
    let mut queued = message("3EB0SEND", "on my way", true, 6);
    queued.state = Some(MessageState::Queued);
    queued.client_id = Some(client_id.clone());
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: GRACE.to_owned(),
        message: queued,
    });
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: GRACE.to_owned(),
        message: message("3EB0SEND", "on my way", true, 7),
    });
    let screen = render(&mut app, 80, 24).conversation();
    assert_eq!(
        screen.matches("on my way").count(),
        1,
        "one message:\n{screen}"
    );
    assert!(screen.contains("you · 17:07"), "{screen}");
    assert!(
        !screen.contains("queued") && !screen.contains("sending"),
        "{screen}"
    );

    // Esc leaves the composer, then the conversation.
    app.handle_key(key(KeyCode::Char('i')));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Messages);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Chats);
    assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
}

#[test]
fn a_send_answered_sent_or_failed_shows_that_state() {
    let mut app = two_chats();
    let first = write_and_send(&mut app);
    app.apply(&DaemonFrame::Sent {
        v: 1,
        client_id: first,
        state: SendState::Sent,
        id: Some("3EB0OK".to_owned()),
        timestamp: Some("2026-10-18T17:09:00Z".to_owned()),
    });
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("you · 17:09"), "{screen}");
    assert!(!screen.contains("sending"), "{screen}");

    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "second");
    let Some(Action::Request(ClientFrame::Send { client_id, .. })) =
        app.handle_key(key(KeyCode::Enter))
    else {
        panic!("sends");
    };
    app.apply(&DaemonFrame::SendFailed {
        v: 1,
        client_id,
        message: "not accepted".to_owned(),
    });
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("· not sent"), "{screen}");
    // The sent one kept its state.
    assert_eq!(screen.matches("not sent").count(), 1, "{screen}");
}

#[test]
fn an_empty_composer_sends_nothing() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane(), Pane::Compose);
    type_text(&mut app, "   ");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
}

#[test]
fn unread_message_connection_and_focus_events_update_the_model() {
    let mut app = two_chats();
    open_first(&mut app, ADA, vec![message("A1", "first", false, 1)]);

    app.apply(&DaemonFrame::EventUnread {
        v: 1,
        unread: 9,
        jid: GRACE.to_owned(),
        chat_unread: 7,
    });
    assert_eq!(app.chats()[1].unread, 7);
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("Grace Hopper (7)"), "{screen}");
    assert!(screen.contains("9 unread"), "{screen}");

    // A new message in the open chat appears at the bottom; one in another chat updates its
    // snippet.
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA.to_owned(),
        message: message("A2", "second", false, 2),
    });
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: GRACE.to_owned(),
        message: message("G9", "news from Grace", false, 3),
    });
    // The same event twice is one message.
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA.to_owned(),
        message: message("A2", "second", false, 2),
    });
    let rendered = render(&mut app, 80, 24);
    assert!(
        rendered.chats().contains("news from Grace"),
        "{}",
        rendered.text()
    );
    let screen = rendered.conversation();
    assert_eq!(screen.matches("second").count(), 1, "{screen}");
    let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
    assert!(row_of("first") < row_of("second"), "{screen}");

    app.apply(&DaemonFrame::EventConnection {
        v: 1,
        session: Session::Offline,
        stop_reason: None,
    });
    assert!(render(&mut app, 80, 24).text().contains("offline"));

    // Focus selects the chat and loads it.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(
        app.apply(&DaemonFrame::EventFocus {
            v: 1,
            jid: GRACE.to_owned()
        }),
        Some(Action::Request(ClientFrame::messages(GRACE, 200)))
    );
    assert_eq!(app.selected(), 1);
    assert_eq!(app.pane(), Pane::Messages);

    // A message from a chat that is not listed yet adds it.
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: FAMILY.to_owned(),
        message: message("F1", "hi all", false, 4),
    });
    assert_eq!(app.chats().len(), 3);
    assert_eq!(app.selected(), 1, "the selection stays on Grace");
}

#[test]
fn question_mark_shows_the_key_help() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('?')));
    let screen = render(&mut app, 80, 24).text();
    for needle in ["Enter", "open", "send", "scroll", "quit"] {
        assert!(screen.contains(needle), "{needle:?}:\n{screen}");
    }
    app.handle_key(key(KeyCode::Char('?')));
    assert!(!render(&mut app, 80, 24).text().contains("Keys"));
}

// --- one round trip through a fake daemon ------------------------------------------------

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A short socket path under /tmp (macOS limits a socket path to about 104 bytes).
fn socket_path() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ntui-{}-{n}.sock", std::process::id()));
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

fn frame_line(value: serde_json::Value) -> String {
    format!("{value}\n")
}

/// The fake daemon's side of the outbox connection the TUI opens at its first send or retry,
/// handshaken: sends and retries are answered there, never on the request connection.
fn accept_outbox(listener: &UnixListener, hello_ok: &str) -> (BufReader<UnixStream>, UnixStream) {
    let (stream, _) = listener.accept().expect("outbox connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("hello");
    assert!(line.contains("\"hello\""), "{line}");
    writer.write_all(hello_ok.as_bytes()).expect("w");
    (reader, writer)
}

/// Whether a send of the open chat still waits for its answer.
fn sending(app: &App) -> bool {
    let model = app.model();
    model
        .loaded()
        .iter()
        .any(|m| model.transcript().is_sending(m))
}

/// Tick (drain) until no send of the open chat waits for its answer any more.
fn settle_sends(daemon: &mut Daemon, app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while sending(app) {
        assert!(Instant::now() < deadline, "a send was never answered");
        daemon.drain_events(app).expect("drain");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_daemon_driver_sends_and_follows_events() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let (shown, go) = std::sync::mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let hello_ok = frame_line(serde_json::json!(
            {"v":1,"op":"hello_ok","daemon":"fake","session":"offline","unread":0}
        ));
        let mut line = String::new();
        // Connection 1 carries requests: hello first.
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        // Connection 2 carries events: hello, then subscribe.
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        assert!(line.contains("subscribe"), "{line}");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");

        // The send comes on the outbox connection.
        let (mut reader, mut writer) = accept_outbox(&listener, &hello_ok);
        line.clear();
        reader.read_line(&mut line).expect("send");
        let send: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(send["op"], "send");
        assert_eq!(send["jid"], ADA);
        assert_eq!(send["text"], "hello there");
        let client_id = send["client_id"].as_str().expect("client_id").to_owned();
        writer
            .write_all(
                frame_line(serde_json::json!(
                    {"v":1,"op":"sent","client_id":client_id,"state":"queued"}
                ))
                .as_bytes(),
            )
            .expect("w");
        // As the daemon does: the queued send is published, then its drained outcome (once the
        // TUI has shown the queued reply).
        go.recv().expect("the TUI showed the reply");
        for message in [
            serde_json::json!({"id":"3EB0X","kind":"text","text":"hello there","from_me":true,
                "timestamp":"2026-10-18T17:29:00Z","state":"queued","client_id":client_id}),
            serde_json::json!({"id":"3EB0X","kind":"text","text":"hello there","from_me":true,
                "timestamp":"2026-10-18T17:30:00Z"}),
        ] {
            events_writer
                .write_all(
                    frame_line(serde_json::json!(
                        {"v":1,"op":"event_message","jid":ADA,"message":message}
                    ))
                    .as_bytes(),
                )
                .expect("w");
        }
        thread::sleep(Duration::from_millis(300));
    });

    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "hello there");
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("sends");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    settle_sends(&mut daemon, &mut app);
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("· queued"), "{screen}");
    shown.send(()).expect("the fake daemon waits");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        daemon.drain_events(&mut app).expect("drain");
        let screen = render(&mut app, 80, 24).text();
        if screen.contains("you · 17:30") {
            assert!(!screen.contains("queued"), "{screen}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the event never arrived:\n{screen}"
        );
    }
    server.join().expect("the fake daemon ran its script");
    let _ = std::fs::remove_file(&path);
}

// --- U3: select a message, reply, react, retry, search, media ----------------------------

fn shift(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

fn ada_conversation(app: &mut App) {
    open_first(
        app,
        ADA,
        vec![
            message("A1", "Are we still on?", false, 1),
            message("A2", "Yes, see you at 18:00", true, 2),
        ],
    );
}

#[test]
fn a_reply_quotes_the_selected_message_and_the_transcript_shows_the_quote() {
    let mut app = two_chats().with_client_prefix("t");
    ada_conversation(&mut app);
    // K selects the newest message, then one older each time.
    assert_eq!(app.handle_key(key(KeyCode::Char('k'))), None);
    assert_eq!(app.handle_key(key(KeyCode::Char('k'))), None);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("▌ Ada Lovelace · 17:01"), "{screen}");
    assert_eq!(app.handle_key(key(KeyCode::Char('r'))), None);
    assert_eq!(app.pane(), Pane::Compose);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("replying to: Are we still on?"), "{screen}");
    type_text(&mut app, "yes");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::send_reply(
            ADA, "yes", "t-1", "A1"
        )))
    );
    // The next message is a plain one again.
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "and");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::send(ADA, "and", "t-2")))
    );

    // The daemon sends it, and its event carries the quote too.
    app.apply(&DaemonFrame::Sent {
        v: 1,
        client_id: "t-1".to_owned(),
        state: SendState::Sent,
        id: Some("3EB0R".to_owned()),
        timestamp: Some("2026-10-18T17:03:00Z".to_owned()),
    });
    let mut reply = message("3EB0R", "yes", true, 3);
    reply.reply_to = Some("A1".to_owned());
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA.to_owned(),
        message: reply,
    });
    let screen = render(&mut app, 80, 24).conversation();
    let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
    let quote = row_of("> Are we still on?").unwrap_or_else(|| panic!("the quote:\n{screen}"));
    assert_eq!(row_of("you · 17:03"), Some(quote - 1), "{screen}");
    assert_eq!(
        screen.matches("> ").count(),
        1,
        "only the reply quotes:\n{screen}"
    );
}

#[test]
fn react_sends_the_chosen_emoji_and_reactions_render_under_the_message() {
    let mut app = two_chats();
    ada_conversation(&mut app);
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.handle_key(key(KeyCode::Char('e'))), None);
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("👍") && screen.contains("🙏"), "{screen}");
    assert_eq!(
        app.handle_key(key(KeyCode::Char('1'))),
        Some(Action::Request(ClientFrame::react(ADA, "A1", "👍")))
    );
    app.apply(&DaemonFrame::Reacted {
        v: 1,
        jid: ADA.to_owned(),
        id: "A1".to_owned(),
        emoji: "👍".to_owned(),
    });
    let screen = render(&mut app, 80, 24).conversation();
    let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
    assert_eq!(
        row_of("👍"),
        Some(row_of("Are we still on?").expect("A1") + 1),
        "{screen}"
    );

    // Their reaction arrives with the message published again; x removes mine.
    let mut again = message("A1", "Are we still on?", false, 1);
    again.reactions = vec![nuncio_client::Reaction {
        emoji: "😮".to_owned(),
        from_me: false,
    }];
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA.to_owned(),
        message: again,
    });
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("😮") && !screen.contains("👍"), "{screen}");
    app.handle_key(key(KeyCode::Char('e')));
    assert_eq!(
        app.handle_key(key(KeyCode::Char('x'))),
        Some(Action::Request(ClientFrame::react(ADA, "A1", "")))
    );
    // Esc closes the picker without sending.
    app.handle_key(key(KeyCode::Char('e')));
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    assert!(!render(&mut app, 80, 24).text().contains("🙏"));
}

#[test]
fn retry_sends_retry_with_the_client_id_of_a_send_that_is_not_sent() {
    let mut app = two_chats().with_client_prefix("t");
    let client_id = write_and_send(&mut app);
    // While it is queued, R does nothing: only a send that ended not sent is retried.
    app.apply(&DaemonFrame::Sent {
        v: 1,
        client_id: client_id.clone(),
        state: SendState::Queued,
        id: None,
        timestamp: None,
    });
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.handle_key(shift('R')), None);
    app.apply(&DaemonFrame::SendFailed {
        v: 1,
        client_id: client_id.clone(),
        message: "not accepted".to_owned(),
    });
    // nunciod publishes the send it refused under the WhatsApp id it stored it with.
    let mut refused = message("3EB0R1", "on my way", true, 6);
    refused.state = Some(MessageState::NotSent);
    refused.client_id = Some(client_id.clone());
    app.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: GRACE.to_owned(),
        message: refused,
    });
    // R on a message that was received does nothing.
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.handle_key(shift('R')), None);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(
        app.handle_key(shift('R')),
        Some(Action::Request(ClientFrame::retry(client_id)))
    );
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("sending"), "{screen}");
}

#[test]
fn search_sends_search_lists_results_newest_first_and_esc_closes() {
    let mut app = two_chats();
    ada_conversation(&mut app);
    assert_eq!(app.handle_key(key(KeyCode::Char('/'))), None);
    type_text(&mut app, "on");
    assert!(render(&mut app, 80, 24).text().contains("Search: on"));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::search(ADA, "on", 50)))
    );
    // The reply comes newest first, as on the wire.
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: ADA.to_owned(),
        messages: vec![
            message("A2", "Yes, see you at 18:00", true, 2),
            message("A1", "Are we still on?", false, 1),
        ],
    });
    let screen = render(&mut app, 80, 24).text();
    let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
    let newest = row_of("▌ you · 17:02 Yes, see").unwrap_or_else(|| panic!("{screen}"));
    assert_eq!(
        row_of("Ada Lovelace · 17:01 Are we"),
        Some(newest + 1),
        "{screen}"
    );
    // Enter on a result selects that message in the conversation.
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
    let screen = render(&mut app, 80, 24).text();
    assert!(!screen.contains("Search"), "{screen}");
    assert!(screen.contains("▌ Ada Lovelace · 17:01"), "{screen}");
    assert_eq!(app.pane(), Pane::Messages);
    // The conversation was not replaced by the results.
    app.handle_key(key(KeyCode::Char('/')));
    type_text(&mut app, "zzz");
    app.handle_key(key(KeyCode::Enter));
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: ADA.to_owned(),
        messages: Vec::new(),
    });
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("no matches"), "{screen}");
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    let screen = render(&mut app, 80, 24).text();
    assert!(!screen.contains("Search"), "{screen}");
    assert!(
        screen.contains("Are we still on?") && screen.contains("Yes, see you"),
        "{screen}"
    );
}

/// A fake daemon: both handshakes and the subscribe, then each request line on the request
/// connection is answered by `answer`, until the client hangs up.
fn fake_daemon(
    path: PathBuf,
    answer: impl FnMut(serde_json::Value) -> serde_json::Value + Send + 'static,
) -> thread::JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(&path).expect("bind");
    thread::spawn(move || {
        let hello_ok = frame_line(serde_json::json!(
            {"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}
        ));
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");
        // The request connection, and every outbox connection after it, answered by `answer`.
        let answer = Arc::new(Mutex::new(answer));
        let ops = Arc::new(Mutex::new(Vec::new()));
        let serve = |mut reader: BufReader<UnixStream>, mut writer: UnixStream| {
            let (answer, ops) = (Arc::clone(&answer), Arc::clone(&ops));
            thread::spawn(move || {
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let request: serde_json::Value = serde_json::from_str(&line).expect("json");
                    let op = request["op"].as_str().unwrap_or_default().to_owned();
                    ops.lock().expect("lock").push(op);
                    let reply = (answer.lock().expect("lock"))(request);
                    writer.write_all(frame_line(reply).as_bytes()).expect("w");
                    line.clear();
                }
            })
        };
        let requests = serve(reader, writer);
        listener.set_nonblocking(true).expect("nonblocking");
        let mut outboxes = Vec::new();
        while !requests.is_finished() {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking");
                    let mut writer = stream.try_clone().expect("clone");
                    let mut reader = BufReader::new(stream);
                    line.clear();
                    reader.read_line(&mut line).expect("hello");
                    writer.write_all(hello_ok.as_bytes()).expect("w");
                    outboxes.push(serve(reader, writer));
                }
                Err(_) => thread::sleep(Duration::from_millis(2)),
            }
        }
        for outbox in outboxes {
            outbox.join().expect("outbox");
        }
        requests.join().expect("requests");
        Arc::try_unwrap(ops)
            .expect("served")
            .into_inner()
            .expect("lock")
    })
}

#[test]
fn a_refused_reaction_shows_the_daemons_error_and_changes_nothing() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(path.clone(), |request| {
        assert_eq!(request["op"], "react");
        serde_json::json!({"v":1,"op":"error","code":"internal",
            "message":"the WhatsApp session is offline, so the reaction was not sent"})
    });
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats();
    ada_conversation(&mut app);
    app.handle_key(key(KeyCode::Char('k')));
    let before = render(&mut app, 120, 24).conversation();
    app.handle_key(key(KeyCode::Char('e')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('2'))) else {
        panic!("2 reacts");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    let screen = render(&mut app, 120, 24);
    let footer = screen.text().lines().last().unwrap_or_default().to_owned();
    assert!(footer.contains("reaction was not sent"), "{footer}");
    let body = |conversation: String| conversation.lines().take(23).collect::<Vec<_>>().join("\n");
    assert_eq!(body(screen.conversation()), body(before));
    drop(daemon);
    assert_eq!(server.join().expect("fake daemon"), ["react"]);
    let _ = std::fs::remove_file(&path);
}

fn media_message(id: &str) -> Message {
    let mut photo = message(id, "the view", false, 4);
    photo.kind = "image".to_owned();
    photo.media = Some(nuncio_client::Media {
        mime: "image/png".to_owned(),
        size: 2048,
        file_name: String::new(),
    });
    photo
}

#[test]
fn o_downloads_the_media_and_opens_it_exactly_once_and_nothing_opens_without_it() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(path.clone(), |request| match request["op"].as_str() {
        Some("download_media") => {
            assert_eq!(request["id"], "IMG1");
            serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"IMG1",
                "path":"/nonexistent/nuncio-media/IMG1.png","mime":"image/png"})
        }
        _ => serde_json::json!({"v":1,"op":"messages","jid":ADA,"messages":[]}),
    });
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::<PathBuf>::new()));
    let record = opened.clone();
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test")
        .expect("connect")
        .with_opener(move |file: &std::path::Path| {
            record.lock().expect("lock").push(file.to_owned());
            Ok(())
        });
    let mut app = two_chats();
    open_first(&mut app, ADA, vec![media_message("IMG1")]);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("[image] the view"), "{screen}");

    // Other requests, events and a stray media_ready open nothing.
    daemon
        .perform(&mut app, &ClientFrame::messages(ADA, 200))
        .expect("perform");
    daemon.drain_events(&mut app).expect("drain");
    let stray = DaemonFrame::MediaReady {
        v: 1,
        jid: ADA.to_owned(),
        id: "IMG1".to_owned(),
        path: "/nonexistent/x.png".to_owned(),
        mime: "image/png".to_owned(),
    };
    assert_eq!(app.apply(&stray), None);
    assert!(opened.lock().expect("lock").is_empty());

    app.apply(&messages_reply(ADA, vec![media_message("IMG1")]));
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o downloads");
    };
    assert_eq!(frame, ClientFrame::download_media(ADA, "IMG1"));
    daemon.perform(&mut app, &frame).expect("perform");
    assert_eq!(
        *opened.lock().expect("lock"),
        [PathBuf::from("/nonexistent/nuncio-media/IMG1.png")]
    );
    // The same reply again (no new key) opens nothing more.
    assert_eq!(app.apply(&stray), None);
    assert_eq!(opened.lock().expect("lock").len(), 1);
    drop(daemon);
    assert_eq!(
        server.join().expect("fake daemon"),
        ["messages", "download_media"]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_search_result_far_up_the_conversation_is_scrolled_into_view() {
    let mut app = two_chats();
    open_first(&mut app, ADA, two_hundred());
    app.handle_key(key(KeyCode::Char('/')));
    type_text(&mut app, "005");
    app.handle_key(key(KeyCode::Enter));
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: ADA.to_owned(),
        messages: vec![message("M005", "message 005", false, 5)],
    });
    app.handle_key(key(KeyCode::Enter));
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("▌ Ada Lovelace · 17:05"), "{screen}");
    assert!(screen.contains("message 005"), "{screen}");
    assert!(!screen.contains("message 199"), "{screen}");
}

#[test]
fn a_refused_download_shows_the_error_and_a_later_media_ready_opens_nothing() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(path.clone(), |request| {
        assert_eq!(request["op"], "download_media");
        serde_json::json!({"v":1,"op":"error","code":"internal",
            "message":"the WhatsApp session is offline, so the media was not downloaded"})
    });
    let opened = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let record = opened.clone();
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test")
        .expect("connect")
        .with_opener(move |_: &std::path::Path| {
            *record.lock().expect("lock") += 1;
            Ok(())
        });
    let mut app = two_chats();
    open_first(&mut app, ADA, vec![media_message("IMG1")]);
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o downloads");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    let screen = render(&mut app, 120, 24).text();
    let footer = screen.lines().last().unwrap_or_default();
    assert!(footer.contains("media was not downloaded"), "{footer}");
    let unsolicited = DaemonFrame::MediaReady {
        v: 1,
        jid: ADA.to_owned(),
        id: "IMG1".to_owned(),
        path: "/nonexistent/IMG1.png".to_owned(),
        mime: "image/png".to_owned(),
    };
    assert_eq!(app.apply(&unsolicited), None);
    drop(daemon);
    server.join().expect("fake daemon");
    assert_eq!(*opened.lock().expect("lock"), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_refused_search_ends_searching_shows_the_error_and_stays_open_for_editing() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(path.clone(), |request| {
        assert_eq!(request["op"], "search");
        serde_json::json!({"v":1,"op":"error","code":"bad_frame",
            "message":"query over 1024 bytes"})
    });
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats();
    ada_conversation(&mut app);
    app.handle_key(key(KeyCode::Char('/')));
    type_text(&mut app, "on");
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter searches");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    let screen = render(&mut app, 120, 24).text();
    assert!(!screen.contains("searching"), "{screen}");
    assert!(
        screen.contains("Search: on"),
        "the overlay stays open:\n{screen}"
    );
    let footer = screen.lines().last().unwrap_or_default();
    assert!(footer.contains("query over 1024 bytes"), "{footer}");
    // The query can be edited again, and a later chat load is not taken as results.
    type_text(&mut app, "e");
    assert!(render(&mut app, 120, 24).text().contains("Search: one"));
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    app.apply(&messages_reply(ADA, vec![message("A9", "fresh", false, 9)]));
    assert!(render(&mut app, 120, 24).conversation().contains("fresh"));
    drop(daemon);
    server.join().expect("fake daemon");
    let _ = std::fs::remove_file(&path);
}

// --- Fixes from the first run against nuncio-demo in a real terminal ----------------------

fn at(id: &str, text: &str, from_me: bool, timestamp: &str) -> Message {
    let mut m = message(id, text, from_me, 0);
    m.timestamp = timestamp.to_owned();
    m
}

#[test]
fn message_times_are_shown_in_the_local_time_zone() {
    let offset = time::UtcOffset::from_hms(-5, 0, 0).expect("a valid offset");
    let mut ada = chat(ADA, "Ada Lovelace", 0);
    ada.last_message = Some(at("A2", "late", true, "2026-10-20T03:10:00Z"));
    let mut app = app_with(vec![ada, chat(GRACE, "Grace Hopper", 0)]).with_utc_offset(offset);
    let screen = render(&mut app, 80, 24);
    assert!(screen.chats().contains("late · 22:10"), "{}", screen.text());
    open_first(
        &mut app,
        ADA,
        vec![
            at("A1", "Are we still on?", false, "2026-10-19T20:36:00Z"),
            at("A2", "late", true, "2026-10-20T03:10:00Z"),
        ],
    );
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("Ada Lovelace · 15:36"), "{screen}");
    assert!(screen.contains("you · 22:10"), "{screen}");
    // With no offset given, times stay UTC (what every other test renders).
    let mut utc = two_chats();
    open_first(
        &mut utc,
        ADA,
        vec![at("A1", "Are we still on?", false, "2026-10-19T20:36:00Z")],
    );
    let screen = render(&mut utc, 80, 24).conversation();
    assert!(screen.contains("Ada Lovelace · 20:36"), "{screen}");
}

#[test]
fn a_long_preview_is_cut_with_an_ellipsis_and_its_time_stays_whole() {
    for (width, height) in [(80u16, 24u16), (240, 60)] {
        let mut ascii = chat(ADA, "", 0);
        ascii.last_message = Some(message(
            "A1",
            &"Is this the right number for the demo? ".repeat(6),
            false,
            36,
        ));
        let mut wide = chat(GRACE, "Grace Hopper", 0);
        wide.last_message = Some(message("G1", &"漢字".repeat(80), false, 37));
        let mut short = chat(FAMILY, "Family", 0);
        short.last_message = Some(message("F1", "See you", false, 38));
        let mut app = app_with(vec![ascii, wide, short]);
        let screen = render(&mut app, width, height);
        let chats = screen.chats();
        for (start, end) in [("Is this", "… · 17:36"), ("漢", "… · 17:37")] {
            let row = chats
                .lines()
                .find(|l| l.contains(start))
                .unwrap_or_else(|| panic!("no {start:?} row at {width}x{height}:\n{chats}"));
            assert!(
                row.trim_end_matches([' ', '│', '┃']).ends_with(end),
                "{start:?} at {width}x{height}: {row:?}"
            );
        }
        assert!(chats.contains("See you · 17:38"), "{chats}");
        assert!(!chats.contains("See you…"), "{chats}");
    }
}

#[test]
fn emoji_sequences_render_as_cells_every_terminal_draws_at_the_same_width() {
    let family = "👨‍👩‍👧‍👦";
    let flag = "🏳️‍🌈";
    let mut named = chat(ADA, &format!("{family} Family"), 0);
    named.last_message = Some(message("A3", "Hi 👋🏽", false, 3));
    let mut preview = chat(GRACE, "Grace", 0);
    preview.last_message = Some(message("G1", &format!("Bring the {flag} flag!"), false, 4));
    let mut app = app_with(vec![named, preview]);
    let mut quoting = message("A3", "Hi 👋🏽", false, 3);
    quoting.reply_to = Some("A1".to_owned());
    quoting.reactions = vec![
        nuncio_client::Reaction {
            emoji: "❤️".to_owned(),
            from_me: true,
        },
        nuncio_client::Reaction {
            emoji: family.to_owned(),
            from_me: false,
        },
    ];
    open_first(
        &mut app,
        ADA,
        vec![
            message("A1", &format!("Bring the {flag} flag!"), false, 1),
            message(
                "A2",
                &format!("the whole family is coming {family}"),
                true,
                2,
            ),
            quoting,
        ],
    );
    let joiners = [
        '\u{200d}',
        '\u{fe0f}',
        '\u{1f3fb}',
        '\u{1f3fc}',
        '\u{1f3fd}',
        '\u{1f3fe}',
        '\u{1f3ff}',
    ];
    for (width, height) in [(80u16, 24u16), (240, 60)] {
        let screen = render(&mut app, width, height);
        for (y, row) in screen.rows.iter().enumerate() {
            for (x, cell) in row.iter().enumerate() {
                assert!(
                    !cell.contains(joiners),
                    "cell ({x},{y}) {cell:?} keeps a joiner at {width}x{height}:\n{}",
                    screen.text()
                );
                let per_char: usize = cell
                    .chars()
                    .map(|c| ratatui::text::Span::raw(c.to_string()).width())
                    .sum();
                assert_eq!(
                    ratatui::text::Span::raw(cell.as_str()).width(),
                    per_char,
                    "cell ({x},{y}) {cell:?} at {width}x{height}"
                );
            }
        }
        let text = screen.text();
        for needle in ["Family", "flag!", "🌈", "👨", "👋", "> Bring the", "❤"] {
            assert!(
                text.contains(needle),
                "{needle:?} at {width}x{height}:\n{text}"
            );
        }
    }
}

#[test]
fn a_message_whatsapp_does_not_have_cannot_be_reacted_to_or_replied_to() {
    let footer = |app: &mut App| {
        let screen = render(app, 120, 24).text();
        screen.lines().last().unwrap_or_default().to_owned()
    };
    for state in [MessageState::NotSent, MessageState::Queued] {
        let mut app = two_chats().with_client_prefix("t");
        let mut failed = message("DEMOS1", "fail this", true, 3);
        failed.state = Some(state.clone());
        failed.client_id = Some("t-9".to_owned());
        open_first(
            &mut app,
            ADA,
            vec![
                message("A1", "Are we still on?", false, 1),
                message("A2", "Yes", true, 2),
                failed,
            ],
        );
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.handle_key(key(KeyCode::Char('e'))), None, "{state:?}");
        assert!(
            footer(&mut app).contains("that message was not sent, so it cannot be reacted to"),
            "{state:?}: {}",
            footer(&mut app)
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('1'))),
            None,
            "no picker: {state:?}"
        );
        assert_eq!(app.handle_key(key(KeyCode::Char('r'))), None);
        assert_eq!(app.pane(), Pane::Messages, "no reply composer: {state:?}");
        assert!(
            footer(&mut app).contains("that message was not sent, so it cannot be replied to"),
            "{state:?}: {}",
            footer(&mut app)
        );
        // A sent message of mine and a received one still can.
        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('1'))),
            Some(Action::Request(ClientFrame::react(ADA, "A2", "👍")))
        );
        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Char('r')));
        assert_eq!(app.pane(), Pane::Compose);
    }
    // A send still waiting for the daemon (no id yet) says so too.
    let mut app = two_chats().with_client_prefix("t");
    write_and_send(&mut app);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.handle_key(key(KeyCode::Char('e'))), None);
    assert!(
        footer(&mut app).contains("that message was not sent, so it cannot be reacted to"),
        "{}",
        footer(&mut app)
    );
}

// --- U8: opening a chat marks it read here and on WhatsApp (S8) ---------------------------

fn marked_read(jid: &str, unread: u32) -> DaemonFrame {
    DaemonFrame::MarkedRead {
        v: 1,
        jid: jid.to_owned(),
        unread,
    }
}

fn event_unread(jid: &str, unread: u32, chat_unread: u32) -> DaemonFrame {
    DaemonFrame::EventUnread {
        v: 1,
        unread,
        jid: jid.to_owned(),
        chat_unread,
    }
}

fn event_message(jid: &str, message: Message) -> DaemonFrame {
    DaemonFrame::EventMessage {
        v: 1,
        jid: jid.to_owned(),
        message,
    }
}

#[test]
fn opening_an_unread_chat_sends_one_mark_read_and_marked_read_clears_the_counts() {
    // The reply and the chat's event_unread may come in either order.
    for event_first in [false, true] {
        let mut app = app_with(vec![
            chat(ADA, "Ada Lovelace", 2),
            chat(GRACE, "Grace Hopper", 3),
        ]);
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Action::Request(ClientFrame::messages(ADA, 200)))
        );
        let reply = messages_reply(ADA, vec![message("A1", "first", false, 1)]);
        assert_eq!(
            app.apply(&reply),
            Some(Action::Request(ClientFrame::mark_read(ADA)))
        );
        // A later load of the same chat (a resync) sends no second one.
        assert_eq!(app.apply(&reply), None);
        if event_first {
            app.apply(&event_unread(ADA, 3, 0));
        }
        assert_eq!(app.apply(&marked_read(ADA, 3)), None);
        if !event_first {
            app.apply(&event_unread(ADA, 3, 0));
        }
        assert_eq!(app.chats()[0].unread, 0, "event_first: {event_first}");
        assert_eq!(app.chats()[1].unread, 3, "other chats keep theirs");
        let screen = render(&mut app, 80, 24).text();
        assert!(screen.contains("3 unread"), "{screen}");
        assert!(!screen.contains("Ada Lovelace (2)"), "{screen}");
        assert!(screen.contains("Grace Hopper (3)"), "{screen}");
    }
}

#[test]
fn opening_a_chat_with_nothing_unread_sends_no_mark_read() {
    let mut app = two_chats();
    app.select_next();
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(GRACE, 200)))
    );
    assert_eq!(
        app.apply(&messages_reply(GRACE, vec![message("G1", "hi", false, 1)])),
        None
    );
}

#[test]
fn the_daemon_driver_marks_an_opened_chat_read_and_a_refusal_keeps_the_count() {
    for refuse in [false, true] {
        let path = socket_path();
        let _socket = Socket(path.clone());
        let server = fake_daemon(path.clone(), move |request| match request["op"].as_str() {
            Some("messages") => serde_json::json!({"v":1,"op":"messages","jid":ADA,
                "messages":[{"id":"A1","kind":"text","text":"first","from_me":false,
                "timestamp":"2026-10-18T17:01:00Z"}]}),
            Some("mark_read") if refuse => serde_json::json!({"v":1,"op":"error",
                "code":"internal",
                "message":"the WhatsApp session is offline, so the chat was not marked read"}),
            Some("mark_read") => {
                assert_eq!(request["jid"], ADA);
                serde_json::json!({"v":1,"op":"marked_read","jid":ADA,"unread":0})
            }
            other => panic!("unexpected request {other:?}"),
        });
        let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
        let mut app = two_chats();
        let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
            panic!("Enter opens the chat");
        };
        daemon.perform(&mut app, &frame).expect("perform");
        let screen = render(&mut app, 120, 24).text();
        let footer = screen.lines().last().unwrap_or_default().to_owned();
        if refuse {
            assert_eq!(
                app.chats()[0].unread,
                2,
                "a refused mark_read keeps the count"
            );
            assert!(screen.contains("2 unread"), "{screen}");
            assert!(
                footer.contains("the WhatsApp session is offline, so the chat was not marked read"),
                "{footer}"
            );
        } else {
            assert_eq!(app.chats()[0].unread, 0);
            assert!(screen.contains("0 unread"), "{screen}");
            assert!(!footer.contains("not marked read"), "{footer}");
        }
        // Nothing more happens on its own: no retry.
        for _ in 0..3 {
            daemon.drain_events(&mut app).expect("drain");
        }
        drop(daemon);
        assert_eq!(
            server.join().expect("fake daemon"),
            ["messages", "mark_read"],
            "refuse: {refuse}"
        );
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn a_new_message_in_the_open_chat_is_marked_read_one_at_a_time() {
    let mut app = two_chats();
    app.select_next();
    app.handle_key(key(KeyCode::Enter));
    app.apply(&messages_reply(GRACE, vec![message("G1", "hi", false, 1)]));
    assert_eq!(
        app.apply(&event_message(
            GRACE,
            message("G2", "are you there?", false, 2)
        )),
        Some(Action::Request(ClientFrame::mark_read(GRACE)))
    );
    // At most one in flight: the next message waits for the answer.
    assert_eq!(
        app.apply(&event_message(GRACE, message("G3", "hello?", false, 3))),
        None
    );
    app.apply(&event_unread(GRACE, 4, 2));
    app.apply(&marked_read(GRACE, 2));
    assert_eq!(app.chats()[1].unread, 0);
    assert_eq!(
        app.apply(&event_message(GRACE, message("G4", "ok", false, 4))),
        Some(Action::Request(ClientFrame::mark_read(GRACE)))
    );
}

#[test]
fn a_new_message_in_another_chat_sends_no_mark_read() {
    let mut app = two_chats();
    // Ada's conversation is loaded (and read), then Grace's chat is the open one.
    open_first(&mut app, ADA, vec![message("A1", "first", false, 1)]);
    app.apply(&marked_read(ADA, 0));
    app.handle_key(key(KeyCode::Esc));
    app.select_next();
    app.handle_key(key(KeyCode::Enter));
    app.apply(&messages_reply(GRACE, vec![message("G1", "hi", false, 1)]));
    assert_eq!(
        app.apply(&event_message(ADA, message("A7", "news", false, 7))),
        None
    );
    let screen = render(&mut app, 80, 24).conversation();
    assert!(
        !screen.contains("news"),
        "Grace's conversation stays open: {screen}"
    );
}

// --- event_chat and not_confirmed (S9 (6), (7)) ------------------------------------------

#[test]
fn event_chat_updates_the_chats_markers_and_resorts_the_list_keeping_the_selection() {
    let mut app = two_chats();
    app.select_next();
    assert_eq!(app.selected_jid(), Some(GRACE));

    let mut ada = chat(ADA, "Ada Lovelace", 2);
    ada.last_message = Some(message("A1", "Are we still on?", false, 10));
    ada.archived = true;
    ada.pinned = true;
    ada.muted = true;
    assert_eq!(
        app.apply(&DaemonFrame::EventChat {
            v: 1,
            chat: ada.clone()
        }),
        None
    );
    let names: Vec<&str> = app.chats().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["Grace Hopper", "Ada Lovelace"],
        "archived sits last"
    );
    assert_eq!(
        app.selected_jid(),
        Some(GRACE),
        "the selection follows the chat"
    );
    let screen = render(&mut app, 80, 24).chats();
    let ada_row = screen
        .lines()
        .find(|l| l.contains("Ada Lovelace"))
        .unwrap_or_else(|| panic!("{screen}"))
        .to_owned();
    for marker in ['★', '⊘', '▤'] {
        assert!(ada_row.contains(marker), "{marker} in {ada_row:?}");
    }

    // Unarchived, unpinned and unmuted on the phone: the markers go and the chat comes back up,
    // above Grace, whose chat has no newer message (the daemon's order: newest message first).
    ada.archived = false;
    ada.pinned = false;
    ada.muted = false;
    app.apply(&DaemonFrame::EventChat { v: 1, chat: ada });
    let names: Vec<&str> = app.chats().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "Grace Hopper"]);
    assert_eq!(app.selected_jid(), Some(GRACE));
    let screen = render(&mut app, 80, 24).chats();
    for marker in ['★', '⊘', '▤'] {
        assert!(!screen.contains(marker), "{marker} left in:\n{screen}");
    }

    // A chat the list does not show yet (pinned on the phone before its first message) is added.
    let mut family = chat(FAMILY, "Old group", 0);
    family.pinned = true;
    app.apply(&DaemonFrame::EventChat { v: 1, chat: family });
    assert_eq!(app.chats().len(), 3);
    assert!(app.chats().iter().any(|c| c.jid == FAMILY && c.pinned));
}

/// A send whose acknowledgement timed out is shown as not confirmed, never as not sent; `R`
/// retries it under its `client_id`, which reuses its WhatsApp id, so it cannot become a second
/// message (the M1 semantics); a send refused for another reason still shows as not sent.
#[test]
fn a_not_confirmed_send_says_so_and_r_retries_it_under_the_same_client_id() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let answers = std::sync::Mutex::new(0);
    let server = fake_daemon(path.clone(), move |request| {
        let mut n = answers.lock().expect("lock");
        *n += 1;
        match (*n, request["op"].as_str()) {
            (1, Some("send")) => serde_json::json!({"v":1,"op":"error","code":"not_confirmed",
                "message":"not confirmed: WhatsApp did not acknowledge in time; it may have been delivered"}),
            (2, Some("retry")) => serde_json::json!({"v":1,"op":"sent",
                "client_id":request["client_id"],"state":"sent","id":"3EB0F1","timestamp":"2026-10-18T17:06:00Z"}),
            (3, Some("send")) => serde_json::json!({"v":1,"op":"error","code":"internal",
                "message":"the store is closed"}),
            other => panic!("unexpected request {other:?}: {request}"),
        }
    });
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats().with_client_prefix("t");
    let client_id = write_and_send(&mut app);
    daemon
        .perform(
            &mut app,
            &ClientFrame::send(GRACE, "on my way", client_id.clone()),
        )
        .expect("perform");
    settle_sends(&mut daemon, &mut app);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(!screen.contains("not sent"), "{screen}");

    app.handle_key(key(KeyCode::Char('k')));
    let retry = app.handle_key(shift('R'));
    assert_eq!(
        retry,
        Some(Action::Request(ClientFrame::retry(client_id.clone())))
    );
    daemon
        .perform(&mut app, &ClientFrame::retry(client_id))
        .expect("perform");
    settle_sends(&mut daemon, &mut app);
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 80, 24).conversation();
    assert!(!screen.contains("not confirmed"), "{screen}");
    assert!(screen.contains("you · "), "{screen}");

    // Any other refusal of a send is still not sent.
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "again");
    let Some(Action::Request(send)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter sends");
    };
    daemon.perform(&mut app, &send).expect("perform");
    settle_sends(&mut daemon, &mut app);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("· not sent"), "{screen}");
    assert_eq!(screen.matches("not confirmed").count(), 0, "{screen}");

    drop(daemon);
    assert_eq!(
        server.join().expect("fake daemon"),
        ["send", "retry", "send"]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_not_confirmed_send_stays_not_confirmed_after_its_event_message() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let hello_ok = frame_line(
            serde_json::json!({"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}),
        );
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");
        let (mut reader, mut writer) = accept_outbox(&listener, &hello_ok);
        line.clear();
        reader.read_line(&mut line).expect("send");
        let request: serde_json::Value = serde_json::from_str(&line).expect("json");
        // What wa/send.go attempt does: publish not_sent first, then answer not_confirmed.
        events_writer.write_all(frame_line(serde_json::json!({"v":1,"op":"event_message","jid":GRACE,
            "message":{"id":"3EB0F1","kind":"text","text":"on my wayy","from_me":true,
            "timestamp":"2026-10-18T17:06:00Z","state":"not_sent","client_id":request["client_id"]}})).as_bytes()).expect("w");
        writer.write_all(frame_line(serde_json::json!({"v":1,"op":"error","code":"not_confirmed",
            "message":"not confirmed: WhatsApp did not acknowledge in time; it may have been delivered"})).as_bytes()).expect("w");
        thread::sleep(Duration::from_millis(300));
    });
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats().with_client_prefix("t");
    let client_id = write_and_send(&mut app);
    daemon
        .perform(&mut app, &ClientFrame::send(GRACE, "on my wayy", client_id))
        .expect("perform");
    settle_sends(&mut daemon, &mut app);
    thread::sleep(Duration::from_millis(100));
    daemon.drain_events(&mut app).expect("drain");
    let screen = render(&mut app, 80, 24).conversation();
    let _ = server.join();
    let _ = std::fs::remove_file(&path);
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(!screen.contains("not sent"), "{screen}");
}

/// Either order of the not_sent event and the not_confirmed reply keeps the mark; a later queued
/// state (a retry while offline) clears it, so a drain that then fails shows not sent.
#[test]
fn the_not_confirmed_mark_survives_either_order_and_a_queued_state_clears_it() {
    let mut app = two_chats().with_client_prefix("t");
    let client_id = write_and_send(&mut app);
    let event = |state: MessageState| {
        let mut m = message("3EB0F1", "on my way", true, 6);
        m.state = Some(state);
        m.client_id = Some(client_id.clone());
        DaemonFrame::EventMessage {
            v: 1,
            jid: GRACE.to_owned(),
            message: m,
        }
    };
    app.apply(&event(MessageState::NotSent));
    app.send_not_confirmed(&client_id);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(!screen.contains("not sent"), "{screen}");

    app.apply(&event(MessageState::Queued));
    app.apply(&event(MessageState::NotSent));
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("· not sent"), "{screen}");
    assert!(!screen.contains("not confirmed"), "{screen}");
}

// --- U15: a newer daemon's unknown ops (L15) -----------------------------------------------

/// A fake daemon whose request connection answers each request with the next of `replies`, whose
/// outbox connection (when `sends` is not empty) answers each send with the next of `sends`, and
/// whose event connection pushes `events` once subscribed, then stays open a while.
fn newer_daemon(
    replies: Vec<serde_json::Value>,
    sends: Vec<serde_json::Value>,
    events: Vec<serde_json::Value>,
) -> (PathBuf, thread::JoinHandle<()>) {
    let path = socket_path();
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let hello_ok = frame_line(serde_json::json!(
            {"v":1,"op":"hello_ok","daemon":"newer","session":"linked","unread":0}
        ));
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        let (subscribed, _) = listener.accept().expect("event connection");
        let mut events_writer = subscribed.try_clone().expect("clone");
        let mut events_reader = BufReader::new(subscribed);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");
        for event in events {
            events_writer
                .write_all(frame_line(event).as_bytes())
                .expect("w");
        }
        if !sends.is_empty() {
            let (mut reader, mut writer) = accept_outbox(&listener, &hello_ok);
            for reply in sends {
                line.clear();
                reader.read_line(&mut line).expect("send");
                writer.write_all(frame_line(reply).as_bytes()).expect("w");
            }
        }
        for reply in replies {
            line.clear();
            reader.read_line(&mut line).expect("request");
            writer.write_all(frame_line(reply).as_bytes()).expect("w");
        }
        thread::sleep(Duration::from_millis(500));
    });
    (path, server)
}

#[test]
fn an_event_this_tui_does_not_know_is_ignored_and_the_event_connection_stays() {
    let (path, server) = newer_daemon(
        Vec::new(),
        Vec::new(),
        vec![
            serde_json::json!({"v":1,"op":"event_typing","jid":ADA,"typing":true}),
            serde_json::json!({"v":1,"op":"event_message","jid":ADA,"message":
                {"id":"3EB0NEW","kind":"text","text":"after the unknown one","from_me":false,
                 "timestamp":"2026-10-18T17:40:00Z"}}),
        ],
    );
    let _socket = Socket(path.clone());
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        daemon.drain_events(&mut app).expect("drain");
        let screen = render(&mut app, 80, 24).text();
        assert!(
            !screen.contains("connection lost") && !screen.contains("reconnecting"),
            "an unknown event dropped the event connection:\n{screen}"
        );
        if screen.contains("after the unknown one") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the next event never arrived:\n{screen}"
        );
    }
    server.join().expect("the fake daemon ran its script");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_reply_this_tui_does_not_know_is_a_footer_error_and_the_send_is_not_confirmed() {
    let (path, server) = newer_daemon(
        vec![serde_json::json!({"v":1,"op":"subscribed"})],
        vec![serde_json::json!({"v":1,"op":"sent_later","client_id":"t-1"})],
        Vec::new(),
    );
    let _socket = Socket(path.clone());
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats().with_client_prefix("t");
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "hello there");
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("sends");
    };
    daemon
        .perform(&mut app, &frame)
        .expect("an unknown reply keeps the connection");
    settle_sends(&mut daemon, &mut app);
    let screen = render(&mut app, 120, 24).text();
    assert!(
        screen.contains("unknown reply `sent_later`"),
        "the footer names the unknown reply:\n{screen}"
    );
    // It may have been sent: not confirmed, and `R` retries it under the same WhatsApp id.
    assert!(screen.contains("· not confirmed"), "{screen}");
    assert!(!screen.contains("sending"), "{screen}");
    // The request connection still works.
    daemon
        .perform(&mut app, &ClientFrame::subscribe())
        .expect("the next request is answered");
    server.join().expect("the fake daemon ran its script");
    let _ = std::fs::remove_file(&path);
}

// --- U15: the chat sort matches the daemon's `chats` order (L15) ---------------------------

/// The daemon lists chats newest message first and, on the same time, by JID (`ORDER BY
/// COALESCE(MAX(m.ts), 0) DESC, c.jid`); a chat with no message sorts last. The TUI's resort after
/// an `event_chat` gives the same order, whatever order the chats were in before.
#[test]
fn chats_whose_last_messages_share_a_time_sort_by_jid_as_the_daemon_lists_them() {
    let at = |jid: &str, name: &str, minute: Option<u32>| {
        let mut c = chat(jid, name, 0);
        c.last_message = minute.map(|m| message(&format!("{name}-1"), "hi", false, m));
        c
    };
    let mut app = app_with(vec![
        at("15551230009@s.whatsapp.net", "Empty B", None),
        at("15551230008@s.whatsapp.net", "Empty A", None),
        at(FAMILY, "Family", Some(7)),
        at(GRACE, "Grace", Some(7)),
        at(ADA, "Ada", Some(7)),
        at("15551230004@s.whatsapp.net", "Newest", Some(9)),
    ]);
    let mut grace = at(GRACE, "Grace", Some(7));
    grace.pinned = true;
    app.apply(&DaemonFrame::EventChat { v: 1, chat: grace });
    let names: Vec<&str> = app.chats().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["Newest", "Ada", "Grace", "Family", "Empty A", "Empty B"]
    );
}

// --- U15: the multi-line composer (S10 (10)) ----------------------------------------------

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Draw `app` and return the screen plus where the terminal cursor was put.
fn render_with_cursor(app: &mut App, width: u16, height: u16) -> (Screen, (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    let cursor = terminal.get_cursor_position().expect("a cursor position");
    let buffer = terminal.backend().buffer();
    let rows = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()).to_owned())
                .collect()
        })
        .collect();
    (Screen { rows }, (cursor.x, cursor.y))
}

/// The row and column of the first cell holding `symbol`, searching from the right pane.
fn find_cell(screen: &Screen, symbol: &str) -> (u16, u16) {
    let from = screen.split() + 1;
    for (y, row) in screen.rows.iter().enumerate() {
        if let Some(x) = row.iter().skip(from).position(|cell| cell == symbol) {
            return ((from + x) as u16, y as u16);
        }
    }
    panic!("{symbol:?} is not on screen:\n{}", screen.text());
}

#[test]
fn alt_enter_and_ctrl_j_insert_a_newline_and_enter_sends_the_text_with_its_newlines() {
    let mut app = two_chats().with_client_prefix("t");
    app.handle_key(key(KeyCode::Char('i')));
    type_text(&mut app, "first");
    assert_eq!(app.handle_key(alt(KeyCode::Enter)), None);
    type_text(&mut app, "second");
    assert_eq!(app.handle_key(ctrl('j')), None);
    type_text(&mut app, "third");
    assert_eq!(app.composer(), "first\nsecond\nthird");
    assert_eq!(app.pane(), Pane::Compose, "a newline does not send");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::send(
            ADA,
            "first\nsecond\nthird",
            "t-1"
        )))
    );
    assert_eq!(app.composer(), "");
    // The transcript draws each line of the message on its own row: the first after its author
    // and time, the others from the pane's first column.
    let screen = render(&mut app, 80, 24).conversation();
    let rows: Vec<&str> = screen
        .lines()
        .filter(|l| l.contains("first") || l.contains("second") || l.contains("third"))
        .collect();
    assert_eq!(rows.len(), 3, "{screen}");
    assert!(rows[0].starts_with("you · "), "{screen}");
    assert!(rows[0].contains("  first "), "{screen}");
    assert!(rows[1].starts_with("second "), "{screen}");
    assert!(rows[2].starts_with("third "), "{screen}");
}

#[test]
fn left_right_home_end_backspace_and_delete_edit_at_the_cursor_by_grapheme() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('i')));
    // The family is one grapheme of seven code points.
    type_text(&mut app, "ab👨\u{200d}👩\u{200d}👧\u{200d}👦c");
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(app.composer(), "a👨\u{200d}👩\u{200d}👧\u{200d}👦c");
    app.handle_key(key(KeyCode::Delete));
    assert_eq!(app.composer(), "ac", "Delete takes the whole family");
    app.handle_key(key(KeyCode::Home));
    type_text(&mut app, "x");
    app.handle_key(key(KeyCode::End));
    type_text(&mut app, "y");
    assert_eq!(app.composer(), "xacy");
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    type_text(&mut app, "z");
    assert_eq!(app.composer(), "xacyz", "Right stops at the end");
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(
        app.composer(),
        "xacyz",
        "Left and Backspace stop at the start"
    );
    app.handle_key(key(KeyCode::Delete));
    assert_eq!(app.composer(), "acyz");

    // Home and End work on the cursor's line of a multi-line draft.
    app.handle_key(key(KeyCode::End));
    app.handle_key(alt(KeyCode::Enter));
    type_text(&mut app, "two");
    app.handle_key(key(KeyCode::Home));
    type_text(&mut app, ">");
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Home));
    type_text(&mut app, "<");
    assert_eq!(app.composer(), "<acyz\n>two");
    app.handle_key(key(KeyCode::End));
    type_text(&mut app, "!");
    assert_eq!(app.composer(), "<acyz!\n>two");
    // Backspace at the start of a line joins it to the line above.
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(app.composer(), "<acyz!two");
}

#[test]
fn the_cursor_is_drawn_after_wide_characters_at_their_display_width() {
    for (width, height) in [(80, 24), (240, 60)] {
        let mut app = two_chats();
        app.handle_key(key(KeyCode::Char('i')));
        type_text(&mut app, "日本語");
        app.handle_key(key(KeyCode::Left));
        let (screen, cursor) = render_with_cursor(&mut app, width, height);
        let (x, y) = find_cell(&screen, "日");
        assert_eq!(cursor, (x + 4, y), "after 日本 at {width}x{height}");

        app.handle_key(key(KeyCode::End));
        type_text(&mut app, " a👍b");
        app.handle_key(key(KeyCode::Left));
        let (screen, cursor) = render_with_cursor(&mut app, width, height);
        let (x, y) = find_cell(&screen, "日");
        assert_eq!(
            cursor,
            (x + 6 + 1 + 1 + 2, y),
            "after 👍 at {width}x{height}"
        );
    }
}

#[test]
fn the_composer_grows_to_five_rows_then_scrolls_with_the_cursor() {
    for (width, height) in [(80, 24), (240, 60)] {
        let mut app = two_chats();
        app.handle_key(key(KeyCode::Char('i')));
        type_text(&mut app, "line1");
        let (_, one) = render_with_cursor(&mut app, width, height);
        app.handle_key(alt(KeyCode::Enter));
        type_text(&mut app, "line2");
        let (screen, two) = render_with_cursor(&mut app, width, height);
        assert_eq!(
            two.1, one.1,
            "the composer grew upwards at {width}x{height}"
        );
        assert_eq!(find_cell(&screen, "›").1 + 1, two.1, "{}", screen.text());
        for n in 3..=7 {
            app.handle_key(alt(KeyCode::Enter));
            type_text(&mut app, &format!("line{n}"));
        }
        let (screen, cursor) = render_with_cursor(&mut app, width, height);
        let text = screen.conversation();
        for n in 3..=7 {
            assert!(text.contains(&format!("line{n}")), "line{n}:\n{text}");
        }
        for n in 1..=2 {
            assert!(
                !text.contains(&format!("line{n}")),
                "line{n} scrolled off:\n{text}"
            );
        }
        assert_eq!(cursor.1, one.1, "the cursor stays on the last row");
        let line7 = screen.rows[usize::from(cursor.1)].concat();
        assert!(line7.contains("line7"), "{line7}");

        // Moving the cursor up to the first line scrolls the composer back to it.
        for _ in 0..50 {
            app.handle_key(key(KeyCode::Left));
        }
        let (screen, cursor) = render_with_cursor(&mut app, width, height);
        let text = screen.conversation();
        assert!(text.contains("line1"), "{text}");
        assert!(!text.contains("line7"), "{text}");
        let row = screen.rows[usize::from(cursor.1)].concat();
        assert!(row.contains("› line1"), "{row}");
        assert_eq!(cursor.0, find_cell(&screen, "›").0 + 2);
    }
}

#[test]
fn a_draft_longer_than_the_pane_wraps_and_the_cursor_follows_it() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('i')));
    let long = "word ".repeat(20);
    type_text(&mut app, long.trim_end());
    let (screen, cursor) = render_with_cursor(&mut app, 80, 24);
    let prompt = find_cell(&screen, "›");
    assert!(cursor.1 > prompt.1, "the draft wrapped:\n{}", screen.text());
    // The conversation's last column is kept free: the cursor stays before it.
    let last = screen.rows[usize::from(cursor.1)].len() - 1;
    assert!(
        usize::from(cursor.0) < last,
        "the cursor is inside the pane"
    );
    // Every word is on screen: nothing was cut off.
    let text = screen.conversation();
    assert_eq!(text.matches("word").count(), 20, "{text}");
}

#[test]
fn the_help_footer_and_keys_name_the_composer_keys() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('i')));
    let footer = render(&mut app, 80, 24).rows[23].concat();
    assert!(footer.contains("Alt-Enter newline"), "{footer}");
    let keys: Vec<String> = nuncio_tui::KEYS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(k, what)| format!("{k} {what}"))
        .collect();
    assert!(keys.iter().any(|k| k.contains("Alt-Enter")), "{keys:?}");
    assert!(keys.iter().any(|k| k.contains("Delete")), "{keys:?}");
    // The whole list fits the help screen at 80x24, down to the last key.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('?')));
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("Ctrl-C quit"), "{screen}");
    assert!(
        screen.contains("Shift+drag selects text"),
        "the last key:\n{screen}"
    );
    assert!(screen.contains("Alt-Enter C-j"), "{screen}");
}

// One chat per contact (ADR 0001): the daemon answers a request naming a contact's phone-number
// JID with her LID chat. Fake numbers and LIDs.
const ADA_PN: &str = "15551230001@s.whatsapp.net";
const ADA_LID: &str = "100000000000001@lid";

#[test]
fn a_messages_reply_with_the_canonical_jid_fills_the_open_alias_chat() {
    let mut a = app_with(vec![chat(ADA_PN, "Ada", 0)]);
    let Some(Action::Request(request)) = a.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter asks for the messages");
    };
    // The reply to that very request names her LID chat.
    a.apply_reply(
        &request,
        &messages_reply(ADA_LID, vec![message("A1", "hello from ada", false, 1)]),
    );
    assert!(render(&mut a, 80, 24).text().contains("hello from ada"));
    assert_eq!(a.selected_jid(), Some(ADA_LID));
    assert_eq!(a.chats().len(), 1, "the alias is renamed, not duplicated");
}

#[test]
fn a_resync_that_drops_the_open_alias_chat_never_sends_the_draft_elsewhere() {
    let mut a = app_with(vec![chat(GRACE, "Grace", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.pane(), Pane::Compose);
    type_text(&mut a, "for ada only");
    a.set_connected(false);
    a.set_connected(true);
    let resync = a.resync();
    a.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace", 0), chat(ADA_LID, "Ada", 0)],
    });
    // Until the daemon says which chat it became, nothing can be sent from the vanished chat.
    assert_eq!(a.pane(), Pane::Compose, "the user stays where they were");
    assert_eq!(
        a.handle_key(key(KeyCode::Enter)),
        None,
        "Enter sends nothing"
    );
    assert!(render(&mut a, 80, 24).text().contains("merged"));
    assert_eq!(a.composer(), "for ada only", "the draft is kept unsent");
    let request = resync
        .iter()
        .find(|f| matches!(f, ClientFrame::Messages { .. }))
        .expect("the resync asks for the open chat's messages");
    a.apply_reply(request, &messages_reply(ADA_LID, vec![]));
    let sent = a.handle_key(key(KeyCode::Enter));
    if let Some(Action::Request(f)) = &sent {
        assert!(
            !format!("{f:?}").contains(GRACE),
            "the draft went to Grace: {f:?}"
        );
    }
    // The reply named the chat it became, so the draft is sent there, and only there.
    assert!(
        matches!(&sent, Some(Action::Request(ClientFrame::Send { jid, text, .. }))
            if jid == ADA_LID && text == "for ada only"),
        "Enter = {sent:?}"
    );
}

#[test]
fn a_chat_dropped_with_no_reply_yet_keeps_the_draft_and_sends_nothing() {
    let mut a = app_with(vec![chat(GRACE, "Grace", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Char('i')));
    type_text(&mut a, "for ada only");
    a.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace", 0), chat(ADA_LID, "Ada", 0)],
    });
    assert_eq!(a.pane(), Pane::Compose);
    assert_eq!(a.handle_key(key(KeyCode::Enter)), None);
    assert_eq!(a.composer(), "for ada only");
    // Esc leaves the composer, and choosing a chat again is what lets the draft go out.
    a.handle_key(key(KeyCode::Esc));
    a.handle_key(key(KeyCode::Esc));
    a.select_next();
    assert_eq!(a.selected_jid(), Some(ADA_LID));
}

#[test]
fn after_a_live_merge_the_open_alias_chat_follows_its_lid_chat() {
    let mut a = app_with(vec![chat(ADA_PN, "Ada", 1)]);
    open_first(
        &mut a,
        ADA_PN,
        vec![message("A1", "before the merge", false, 1)],
    );
    // The daemon merged the chat while mark_read(PN) was in flight.
    a.apply(&DaemonFrame::EventChat {
        v: 1,
        chat: chat(ADA_LID, "Ada", 1),
    });
    // The reply to mark_read(PN) names her LID chat.
    a.apply_reply(
        &ClientFrame::mark_read(ADA_PN),
        &DaemonFrame::MarkedRead {
            v: 1,
            jid: ADA_LID.to_owned(),
            unread: 0,
        },
    );
    assert_eq!(a.selected_jid(), Some(ADA_LID));
    assert_eq!(
        a.chats().len(),
        1,
        "the ghost phone-number entry is dropped"
    );
    let next = a.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA_LID.to_owned(),
        message: message("A2", "after the merge", false, 2),
    });
    // marking cleared: the new message in the open chat is marked read again.
    assert_eq!(next, Some(Action::Request(ClientFrame::mark_read(ADA_LID))));
    let screen = render(&mut a, 80, 24).text();
    assert!(screen.contains("before the merge") && screen.contains("after the merge"));
}

#[test]
fn a_send_published_under_the_lid_lands_in_the_open_alias_chat() {
    let mut a = app_with(vec![chat(ADA_PN, "Ada", 0)]);
    open_first(&mut a, ADA_PN, vec![]);
    a.handle_key(key(KeyCode::Enter));
    type_text(&mut a, "hi");
    let Some(Action::Request(ClientFrame::Send { client_id, .. })) =
        a.handle_key(key(KeyCode::Enter))
    else {
        panic!("Enter sends");
    };
    let mut sent = message("S1", "hi", true, 3);
    sent.client_id = Some(client_id);
    sent.state = Some(MessageState::Queued);
    a.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA_LID.to_owned(),
        message: sent,
    });
    assert_eq!(a.selected_jid(), Some(ADA_LID));
    let screen = render(&mut a, 80, 24).text();
    assert!(
        screen.contains("queued"),
        "the send shows its queued state: {screen}"
    );
    assert_eq!(a.chats().len(), 1);
}

#[test]
fn a_mark_read_in_flight_for_the_alias_counts_for_its_lid_chat() {
    let mut a = app_with(vec![chat(ADA_PN, "Ada", 1)]);
    open_first(&mut a, ADA_PN, vec![message("A1", "hello", false, 1)]);
    a.resync();
    a.apply_reply(
        &ClientFrame::messages(ADA_PN, 200),
        &messages_reply(ADA_LID, vec![message("A1", "hello", false, 1)]),
    );
    // mark_read(PN) is still unanswered: a new message must not send a second one.
    let next = a.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA_LID.to_owned(),
        message: message("A2", "again", false, 2),
    });
    assert_eq!(next, None);
    a.apply(&DaemonFrame::MarkedRead {
        v: 1,
        jid: ADA_LID.to_owned(),
        unread: 0,
    });
    let next = a.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA_LID.to_owned(),
        message: message("A3", "and again", false, 3),
    });
    assert_eq!(next, Some(Action::Request(ClientFrame::mark_read(ADA_LID))));
}

const BOB_LID: &str = "100000000000002@lid";

/// The chat list, as JIDs.
fn chat_jids(app: &App) -> Vec<String> {
    app.chats().iter().map(|c| c.jid.clone()).collect()
}

// R1: a request that failed leaves nothing pending, so a later reply about another chat is never
// taken as an answer for it.
#[test]
fn a_stale_open_request_never_pairs_with_another_chats_reply() {
    let mut a = app_with(vec![chat(BOB_LID, "Bob", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter)); // messages(Ada) ...
    a.request_failed(&ClientFrame::messages(ADA_PN, 200), "not connected"); // ... never answered
    a.handle_key(key(KeyCode::Esc));
    a.select_previous(); // Bob, selected only
    a.set_connected(false);
    a.set_connected(true);
    a.resync();
    a.apply(&messages_reply(
        BOB_LID,
        vec![message("B1", "from bob", false, 1)],
    ));
    assert_eq!(chat_jids(&a), vec![BOB_LID.to_owned(), ADA_PN.to_owned()]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter));
    assert!(!render(&mut a, 80, 24).conversation().contains("from bob"));
}

// R2: a reply about Bob while Ada's request is pending is Bob's, never Ada's.
#[test]
fn a_reply_about_another_chat_never_renames_the_pending_one() {
    let mut a = app_with(vec![chat(BOB_LID, "Bob", 0), chat(ADA_PN, "Ada", 0)]);
    a.handle_key(key(KeyCode::Enter)); // messages(Bob)
    a.handle_key(key(KeyCode::Esc));
    a.select_next();
    a.handle_key(key(KeyCode::Enter)); // messages(Ada), before Bob's reply
    a.apply(&messages_reply(
        BOB_LID,
        vec![message("B1", "from bob", false, 1)],
    ));
    assert_eq!(chat_jids(&a), vec![BOB_LID.to_owned(), ADA_PN.to_owned()]);
    assert_eq!(a.selected_jid(), Some(ADA_PN));
    assert!(!render(&mut a, 80, 24).conversation().contains("from bob"));
}

#[test]
fn a_reacted_reply_about_another_chat_never_renames_the_pending_one() {
    let mut a = app_with(vec![chat(BOB_LID, "Bob", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter));
    a.apply(&messages_reply(
        ADA_PN,
        vec![message("A1", "hello", false, 1)],
    ));
    a.handle_key(key(KeyCode::Esc));
    a.handle_key(key(KeyCode::Char('k')));
    a.handle_key(key(KeyCode::Char('e')));
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('1'))),
        Some(Action::Request(ClientFrame::React { .. }))
    ));
    a.apply(&DaemonFrame::Reacted {
        v: 1,
        jid: BOB_LID.to_owned(),
        id: "B1".to_owned(),
        emoji: "👍".to_owned(),
    });
    assert_eq!(chat_jids(&a), vec![BOB_LID.to_owned(), ADA_PN.to_owned()]);
    assert_eq!(a.selected_jid(), Some(ADA_PN));
}

#[test]
fn a_late_reply_after_the_user_chose_another_chat_never_renames_that_chat() {
    let mut a = app_with(vec![chat(GRACE, "Grace", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter));
    a.apply(&messages_reply(
        ADA_PN,
        vec![message("A1", "hello", false, 1)],
    ));
    a.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace", 0), chat(ADA_LID, "Ada", 0)],
    });
    a.handle_key(key(KeyCode::Esc));
    a.select_previous();
    a.handle_key(key(KeyCode::Enter)); // opens Grace
    a.apply(&messages_reply(
        ADA_LID,
        vec![message("A1", "hello", false, 1)],
    ));
    assert_eq!(chat_jids(&a), vec![GRACE.to_owned(), ADA_LID.to_owned()]);
    assert_eq!(a.selected_jid(), Some(GRACE));
    assert_eq!(a.chats()[0].name, "Grace");
}

// R3: the open chat vanishes while the user types; every key keeps editing the draft, and Enter
// sends nothing until the user chooses the chat again.
#[test]
fn typing_through_a_merge_never_sends_the_draft_to_another_chat() {
    let mut a = app_with(vec![chat(GRACE, "Grace", 0), chat(ADA_PN, "Ada", 0)]);
    a.select_next();
    a.handle_key(key(KeyCode::Enter));
    a.apply(&messages_reply(ADA_PN, vec![]));
    assert_eq!(a.pane(), Pane::Compose);
    type_text(&mut a, "for ada");
    a.apply(&DaemonFrame::Chats {
        v: 1,
        chats: vec![chat(GRACE, "Grace", 0), chat(ADA_LID, "Ada", 0)],
    });
    for c in " is it".chars() {
        let action = a.handle_key(key(KeyCode::Char(c)));
        assert!(
            !matches!(action, Some(Action::Request(_)) | Some(Action::Quit)),
            "typing {c:?} did {action:?}"
        );
    }
    let enter = a.handle_key(key(KeyCode::Enter));
    assert!(
        !format!("{enter:?}").contains(GRACE) && !matches!(enter, Some(Action::Request(_))),
        "Enter = {enter:?}"
    );
    assert_eq!(a.pane(), Pane::Compose);
    assert_eq!(a.composer(), "for ada is it");
    assert!(render(&mut a, 80, 24).text().contains("choose it again"));
}

// The driver pairs a request with its own reply: messages(PN) answered with the LID chat adopts it.
#[test]
fn the_daemon_driver_adopts_the_chat_its_own_reply_names() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let listener = UnixListener::bind(&path).expect("bind");
    let server = thread::spawn(move || {
        let hello_ok = frame_line(serde_json::json!(
            {"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}
        ));
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");
        line.clear();
        reader.read_line(&mut line).expect("messages");
        let asked: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(asked["op"], "messages");
        assert_eq!(asked["jid"], ADA_PN);
        writer
            .write_all(
                frame_line(
                    serde_json::json!({"v":1,"op":"messages","jid":ADA_LID,"messages":[
                    {"id":"A1","kind":"text","text":"served as her lid chat","from_me":false,
                     "timestamp":"2026-10-18T17:30:00Z"}]}),
                )
                .as_bytes(),
            )
            .expect("w");
        thread::sleep(Duration::from_millis(200));
    });

    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = app_with(vec![chat(ADA_PN, "Ada", 0)]);
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Enter)) else {
        panic!("Enter asks for the messages");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    assert_eq!(app.selected_jid(), Some(ADA_LID));
    assert_eq!(chat_jids(&app), vec![ADA_LID.to_owned()]);
    assert!(
        render(&mut app, 80, 24)
            .text()
            .contains("served as her lid chat")
    );
    server.join().expect("the fake daemon ran its script");
    let _ = std::fs::remove_file(&path);
}

// A mark_read lost with the connection is never answered: it must not block the next one.
#[test]
fn a_mark_read_lost_with_the_connection_does_not_block_the_next_one() {
    let mut a = two_chats();
    open_first(&mut a, ADA, vec![message("A1", "hi", false, 1)]);
    a.set_connected(false);
    a.set_connected(true);
    let next = a.apply(&DaemonFrame::EventMessage {
        v: 1,
        jid: ADA.to_owned(),
        message: message("A2", "still there?", false, 2),
    });
    assert_eq!(next, Some(Action::Request(ClientFrame::mark_read(ADA))));
}

#[test]
fn a_reacted_reply_to_my_own_react_adopts_the_lid_chat() {
    let mut a = app_with(vec![chat(ADA_PN, "Ada", 0)]);
    open_first(&mut a, ADA_PN, vec![message("A1", "hello", false, 1)]);
    a.handle_key(key(KeyCode::Char('k')));
    a.handle_key(key(KeyCode::Char('e')));
    let Some(Action::Request(request)) = a.handle_key(key(KeyCode::Char('1'))) else {
        panic!("1 reacts");
    };
    a.apply_reply(
        &request,
        &DaemonFrame::Reacted {
            v: 1,
            jid: ADA_LID.to_owned(),
            id: "A1".to_owned(),
            emoji: "👍".to_owned(),
        },
    );
    assert_eq!(chat_jids(&a), vec![ADA_LID.to_owned()]);
    assert!(render(&mut a, 80, 24).conversation().contains("👍"));
}

// R9 (S8): each chat keeps its own draft; switching chats never carries one to another chat and
// never loses one.
#[test]
fn switching_chats_keeps_each_chats_own_draft() {
    let mut a = app_with(vec![chat(ADA, "Ada", 0), chat(GRACE, "Grace", 0)]);
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.pane(), Pane::Compose);
    type_text(&mut a, "meant for ada");
    a.handle_key(key(KeyCode::Esc));
    a.handle_key(key(KeyCode::Tab));
    assert_eq!(a.pane(), Pane::Chats);
    a.handle_key(key(KeyCode::Char('j')));
    assert_eq!(a.selected_jid(), Some(GRACE));
    assert_eq!(a.composer(), "", "Ada's draft never shows in Grace's chat");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(
        a.handle_key(key(KeyCode::Enter)),
        None,
        "Enter in Grace's empty composer sends nothing"
    );
    type_text(&mut a, "for grace");
    a.handle_key(key(KeyCode::Esc));
    a.handle_key(key(KeyCode::Tab));
    a.handle_key(key(KeyCode::Char('k')));
    assert_eq!(a.selected_jid(), Some(ADA));
    assert_eq!(a.composer(), "meant for ada", "Ada's draft is back");
    assert!(
        render(&mut a, 80, 24).text().contains("meant for ada"),
        "and drawn in her composer"
    );
    a.handle_key(key(KeyCode::Char('j')));
    a.handle_key(key(KeyCode::Enter));
    let sent = a.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(&sent, Some(Action::Request(ClientFrame::Send { jid, text, .. }))
            if jid == GRACE && text == "for grace"),
        "{sent:?}"
    );
    assert_eq!(a.composer(), "");
    // A send leaves the composer for the conversation; Esc goes back to the chat list.
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.pane(), Pane::Chats);
    a.handle_key(key(KeyCode::Char('k')));
    assert_eq!(a.selected_jid(), Some(ADA));
    assert_eq!(
        a.composer(),
        "meant for ada",
        "a send in Grace's chat left Ada's"
    );
}
