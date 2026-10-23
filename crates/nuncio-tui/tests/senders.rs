//! Group senders (wire Message `sender`, M4): a group message's header names who wrote it, through
//! `App`'s public interface and a `TestBackend` buffer.

use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const GROUP: &str = "120363000000000001@g.us";

fn message(id: &str, text: &str, from_me: bool, minute: u32, sender: Option<&str>) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "kind": "text", "text": text, "from_me": from_me,
        "timestamp": format!("2026-10-18T17:{minute:02}:00Z"), "sender": sender,
    }))
    .expect("a message")
}

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    let buffer = terminal.backend().buffer();
    (0..24)
        .map(|y| {
            (0..80)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()))
                .collect::<String>()
                + "\n"
        })
        .collect()
}

#[test]
fn a_group_message_header_names_its_sender_and_falls_back_to_the_group() {
    let chat: Chat = serde_json::from_value(serde_json::json!({
        "jid": GROUP, "name": "Climbing", "unread": 0, "archived": false, "pinned": false,
    }))
    .expect("a chat");
    let snapshot = Snapshot {
        daemon: "nunciod/0.1.0".to_owned(),
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
            message("G2", "an old one", false, 2, None),
            message("G1", "rope?", false, 1, Some("Ana")),
        ],
    });
    let screen = render(&mut app);
    assert!(screen.contains("Ana · 17:01"), "the sender:\n{screen}");
    assert!(
        screen.contains("Climbing · 17:02"),
        "no sender: the group's name, as before:\n{screen}"
    );
    assert!(screen.contains("you · 17:03"), "mine:\n{screen}");
    assert!(
        !screen.contains("Climbing · 17:01"),
        "the group's name instead of the sender:\n{screen}"
    );
}
