//! M2's TUI core: the conversation view, the composer, key handling and the reducer of wire
//! events, all through `App`'s public interface and a `TestBackend` buffer, plus one round trip
//! through `Daemon` against a scripted fake daemon on a Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
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

    /// The column of the chat pane's right border.
    fn split(&self) -> usize {
        self.rows[1]
            .iter()
            .position(|cell| cell == "┐")
            .expect("the chat pane's top-right corner")
    }

    /// The chat pane, borders included.
    fn chats(&self) -> String {
        self.columns(0, self.split() + 1)
    }

    /// The conversation pane, borders included.
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

/// Open the first chat and load `oldest_first` into it.
fn open_first(app: &mut App, jid: &str, oldest_first: Vec<Message>) {
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(jid, 200))),
        "Enter on a chat asks the daemon for its messages"
    );
    assert_eq!(app.pane(), Pane::Messages);
    assert_eq!(app.apply(&messages_reply(jid, oldest_first)), None);
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
        // The chat pane's right border and the screen's right border stay in their columns on
        // every body row: no wide character pushed a border out.
        let chat_right = screen.rows[1]
            .iter()
            .position(|cell| cell == "┐")
            .expect("the chat pane's top-right corner") as u16;
        for y in 2..height - 2 {
            assert_eq!(
                screen.cell(chat_right, y),
                "│",
                "chat pane border at row {y}, {width}x{height}:\n{}",
                screen.text()
            );
            assert_eq!(
                screen.cell(width - 1, y),
                "│",
                "right border at row {y}, {width}x{height}:\n{}",
                screen.text()
            );
        }
        if width >= 80 {
            let text = screen.text();
            assert!(text.contains("Family"), "{text}");
            assert!(text.contains("👋🏽"), "{text}");
            assert!(text.contains("> 👨‍👩‍👧‍👦"), "the quote of A1:\n{text}");
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

#[test]
fn two_hundred_messages_scroll_to_the_top_and_back_quickly() {
    let mut app = two_chats();
    open_first(&mut app, ADA, two_hundred());
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("message 199"), "newest shown:\n{screen}");
    assert!(!screen.contains("message 000"));

    let bound = Duration::from_millis(250);
    let timed = |app: &mut App, code: KeyCode| {
        let start = Instant::now();
        app.handle_key(key(code));
        let screen = render(app, 80, 24).text();
        let took = start.elapsed();
        assert!(took < bound, "{code:?} + render took {took:?}");
        screen
    };

    // Line by line upward, then page by page, then all the way.
    let after_k = timed(&mut app, KeyCode::Char('k'));
    assert!(!after_k.contains("message 199") || after_k.contains("message 198"));
    timed(&mut app, KeyCode::PageUp);
    let top = timed(&mut app, KeyCode::Char('g'));
    assert!(top.contains("message 000"), "top:\n{top}");
    assert!(!top.contains("message 199"), "top:\n{top}");
    // Past the top stays at the top.
    let still_top = timed(&mut app, KeyCode::Char('k'));
    assert!(still_top.contains("message 000"), "{still_top}");

    timed(&mut app, KeyCode::PageDown);
    let bottom = timed(&mut app, KeyCode::Char('G'));
    assert!(bottom.contains("message 199"), "bottom:\n{bottom}");
    assert!(!bottom.contains("message 000"));
    // Past the bottom stays at the bottom.
    let still = timed(&mut app, KeyCode::Char('j'));
    assert!(still.contains("message 199"), "{still}");
}

/// Select the second chat, open it, write "on my way" and send it.
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
    assert_eq!(app.handle_key(key(KeyCode::Char('i'))), None);
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
    app.handle_key(key(KeyCode::Char('i')));
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

fn frame_line(value: serde_json::Value) -> String {
    format!("{value}\n")
}

#[test]
fn the_daemon_driver_sends_and_follows_events() {
    let path = socket_path();
    let listener = UnixListener::bind(&path).expect("bind");
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
        // As the daemon does: the queued send is published, then its drained outcome.
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
    let screen = render(&mut app, 80, 24).text();
    assert!(screen.contains("· queued"), "{screen}");

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
    assert_eq!(app.handle_key(shift('K')), None);
    assert_eq!(app.handle_key(shift('K')), None);
    let screen = render(&mut app, 80, 24).conversation();
    assert!(screen.contains("› Ada Lovelace · 17:01"), "{screen}");
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
    app.handle_key(shift('K'));
    app.handle_key(shift('K'));
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
    app.handle_key(shift('K'));
    assert_eq!(app.handle_key(shift('R')), None);
    app.handle_key(key(KeyCode::Esc));
    app.apply(&DaemonFrame::SendFailed {
        v: 1,
        client_id: client_id.clone(),
        message: "not accepted".to_owned(),
    });
    // R on a message that was received does nothing.
    app.handle_key(shift('K'));
    app.handle_key(shift('K'));
    assert_eq!(app.handle_key(shift('R')), None);
    app.handle_key(shift('J'));
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
    let newest = row_of("› you · 17:02 Yes, see").unwrap_or_else(|| panic!("{screen}"));
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
    assert!(screen.contains("› Ada Lovelace · 17:01"), "{screen}");
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
    mut answer: impl FnMut(serde_json::Value) -> serde_json::Value + Send + 'static,
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
        let mut ops = Vec::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return ops;
            }
            let request: serde_json::Value = serde_json::from_str(&line).expect("json");
            ops.push(request["op"].as_str().unwrap_or_default().to_owned());
            writer
                .write_all(frame_line(answer(request)).as_bytes())
                .expect("w");
        }
    })
}

#[test]
fn a_refused_reaction_shows_the_daemons_error_and_changes_nothing() {
    let path = socket_path();
    let server = fake_daemon(path.clone(), |request| {
        assert_eq!(request["op"], "react");
        serde_json::json!({"v":1,"op":"error","code":"internal",
            "message":"the WhatsApp session is offline, so the reaction was not sent"})
    });
    let mut daemon = Daemon::connect(&path, "nuncio-tui/test").expect("connect");
    let mut app = two_chats();
    ada_conversation(&mut app);
    let before = render(&mut app, 120, 24).conversation();
    app.handle_key(shift('K'));
    app.handle_key(key(KeyCode::Char('e')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('2'))) else {
        panic!("2 reacts");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    app.handle_key(shift('J'));
    app.handle_key(key(KeyCode::Esc));
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
    app.handle_key(shift('K'));
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
    assert!(screen.contains("› Ada Lovelace · 17:05"), "{screen}");
    assert!(screen.contains("message 005"), "{screen}");
    assert!(!screen.contains("message 199"), "{screen}");
}

#[test]
fn a_refused_download_shows_the_error_and_a_later_media_ready_opens_nothing() {
    let path = socket_path();
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
    app.handle_key(shift('K'));
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
