//! Group senders (wire Message `sender`, M4): a group message's header names who wrote it, through
//! `App`'s public interface and a `TestBackend` buffer.

use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};

const GROUP: &str = "120363000000000001@g.us";

fn message(id: &str, text: &str, from_me: bool, minute: u32, sender: Option<&str>) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "kind": "text", "text": text, "from_me": from_me,
        "timestamp": format!("2026-10-18T17:{minute:02}:00Z"), "sender": sender,
    }))
    .expect("a message")
}

fn draw(app: &mut App) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    terminal.backend().buffer().clone()
}

fn render(app: &mut App) -> String {
    let buffer = draw(app);
    (0..24)
        .map(|y| {
            (0..80)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()))
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn row_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> u16 {
    (0..24)
        .find(|&y| {
            (0..80)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()))
                .collect::<String>()
                .contains(needle)
        })
        .unwrap_or_else(|| panic!("{needle:?} is not on screen"))
}

#[test]
fn a_group_names_its_sender_once_per_run_and_a_chat_never_names_anyone() {
    let chat: Chat = serde_json::from_value(serde_json::json!({
        "jid": GROUP, "name": "Climbing", "unread": 0, "archived": false, "pinned": false,
    }))
    .expect("a chat");
    let snapshot = Snapshot {
        daemon: "nunciod/0.2.1".to_owned(),
        linked: true,
        session: Session::Linked,
        unread: 0,
        chats: vec![chat],
        stop_reason: None,
    };
    let mut app = App::new(snapshot, Source::Daemon, Palette::default());
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(Action::Request(ClientFrame::messages(GROUP, 200)))
    );
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: GROUP.to_owned(),
        messages: vec![
            message("G3", "mine", true, 3, None),
            message("G2", "take the red one", false, 2, Some("Ana Ruiz")),
            message("G1", "rope?", false, 1, Some("Ana Ruiz")),
        ],
    });
    let buffer = draw(&mut app);
    let screen = render(&mut app);
    // One run of Ana names her once, at the top of its first bubble, in the accent and bold,
    // and only by her first name: the surname would eat the slab's width.
    assert_eq!(
        screen.matches("Ana").count(),
        1,
        "one name per run:\n{screen}"
    );
    assert!(
        !screen.contains("Ruiz"),
        "the sender line is the first name only:\n{screen}"
    );
    let y = row_of(&buffer, "Ana");
    let (x, y) = (33..80)
        .find(|&x| buffer[(x, y)].symbol().starts_with('A'))
        .map(|x| (x, y))
        .unwrap_or_else(|| panic!("the name:\n{screen}"));
    assert_eq!(buffer[(x, y)].symbol(), "A");
    assert_eq!(
        row_of(&buffer, "rope?"),
        y + 1,
        "the sender line is one row, and the text follows it:\n{screen}"
    );
    let accent = Palette::default().accent();
    assert_eq!(buffer[(x, y)].fg, Color::Rgb(accent.r, accent.g, accent.b));
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::BOLD),
        "the name is bold"
    );
    // The header rule says `Climbing · group`; over the bubbles there is no author header. Only
    // the conversation columns count.
    let split = screen
        .lines()
        .nth(1)
        .and_then(|line| line.chars().position(|symbol| symbol == '┬'))
        .expect("the line between the panes");
    let body: String = screen
        .lines()
        .skip(2)
        .map(|line| line.chars().skip(split + 1).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("Climbing") && !body.contains("you ·"),
        "no author header over a bubble:\n{screen}"
    );

    // A one-to-one chat names nobody: the side says whose a message is.
    let mut app = App::new(
        Snapshot {
            daemon: "nunciod/0.2.1".to_owned(),
            linked: true,
            session: Session::Linked,
            unread: 0,
            chats: vec![
                serde_json::from_value(serde_json::json!({
                    "jid": "15551230001@s.whatsapp.net", "name": "Ada", "unread": 0,
                    "archived": false, "pinned": false,
                }))
                .expect("a chat"),
            ],
            stop_reason: None,
        },
        Source::Daemon,
        Palette::default(),
    );
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: "15551230001@s.whatsapp.net".to_owned(),
        // The daemon may name the sender even here; a one-to-one chat still names nobody.
        messages: vec![message("A1", "hello", false, 1, Some("Ada"))],
    });
    let screen = render(&mut app);
    // The body's conversation columns only: the chat list and the pane's rule also say `Ada`.
    let conversation = screen
        .lines()
        .skip(3)
        .map(|line| line.chars().skip(33).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(conversation.contains("hello"), "{screen}");
    assert!(
        !conversation.contains("Ada"),
        "no name in a chat:\n{screen}"
    );
}

fn reply(
    id: &str,
    text: &str,
    from_me: bool,
    minute: u32,
    sender: Option<&str>,
    to: &str,
) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "kind": "text", "text": text, "from_me": from_me,
        "timestamp": format!("2026-10-18T17:{minute:02}:00Z"), "sender": sender,
        "reply_to": to,
    }))
    .expect("a message")
}

#[test]
fn a_quotes_prefix_names_its_author_and_says_you_for_mine() {
    let group = || -> App {
        let chat: Chat = serde_json::from_value(serde_json::json!({
            "jid": GROUP, "name": "Climbing", "unread": 0, "archived": false, "pinned": false,
        }))
        .expect("a chat");
        let mut app = App::new(
            Snapshot {
                daemon: "nunciod/0.2.1".to_owned(),
                linked: true,
                session: Session::Linked,
                unread: 0,
                chats: vec![chat],
                stop_reason: None,
            },
            Source::Daemon,
            Palette::default(),
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app
    };
    // One of mine, quoted by a member: the prefix says `you`.
    let mut app = group();
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: GROUP.to_owned(),
        messages: vec![
            reply("G2", "mine", false, 2, Some("Ana Ruiz"), "G1"),
            message("G1", "rope?", true, 1, None),
        ],
    });
    let screen = render(&mut app);
    assert!(screen.contains("▌ you: rope?"), "{screen}");
    // One of hers, quoted by me: the prefix is her first name.
    let mut app = group();
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: GROUP.to_owned(),
        messages: vec![
            reply("G2", "mine too", true, 2, None, "G1"),
            message("G1", "rope?", false, 1, Some("Ana Ruiz")),
        ],
    });
    let screen = render(&mut app);
    assert!(screen.contains("▌ Ana: rope?"), "{screen}");
    assert!(!screen.contains("Ruiz"), "{screen}");
}
