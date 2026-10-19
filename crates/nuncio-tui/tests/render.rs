//! The machine-checkable half of S9's acceptance test: the TUI renders inside a
//! `TestBackend` buffer at 80×24 and at an odd size, with no terminal and no daemon.

use nuncio_client::{Chat, Message, Palette, Session, Snapshot};
use nuncio_tui::{App, Pane, Source, fixture_snapshot};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

/// Render `app` at the given size and return the screen as lines of text.
fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");

    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");

    let buffer: &Buffer = terminal.backend().buffer();
    assert_eq!(
        buffer.area.width, width,
        "buffer width is what we asked for"
    );
    assert_eq!(
        buffer.area.height, height,
        "buffer height is what we asked for"
    );

    let mut screen = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                screen.push_str(cell.symbol());
            }
        }
        screen.push('\n');
    }
    screen
}

fn line_with<'a>(screen: &'a str, needle: &str) -> &'a str {
    screen
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?} in:\n{screen}"))
}

fn fixture_app() -> App {
    App::new(fixture_snapshot(), Source::Fixture, Palette::default())
}

#[test]
fn the_fixture_chat_list_renders_at_eighty_by_twenty_four() {
    let mut app = fixture_app();
    let screen = render(&mut app, 80, 24);

    // The chat list: name, unread badge, pinned and archived markers.
    assert!(
        screen.contains("Ada Lovelace (2)"),
        "the chat name and its unread badge sit together:\n{screen}"
    );
    assert!(screen.contains('★'), "the pinned marker:\n{screen}");
    assert!(screen.contains('▤'), "the archived marker:\n{screen}");
    assert!(
        screen.contains("Grace Hopper"),
        "the second chat:\n{screen}"
    );
    assert!(screen.contains("Omarchy devs"), "the group chat:\n{screen}");
    assert!(
        screen.contains("Omarchy devs (17)"),
        "the group chat's unread badge:\n{screen}"
    );

    // The last-message snippet and its time.
    assert!(
        screen.contains("See you at 18:00 · 17:55"),
        "the last-message snippet and time:\n{screen}"
    );

    // It says the data is fixture data.
    assert!(
        screen.contains("FIXTURE DATA"),
        "the fixture-data notice:\n{screen}"
    );

    // The message pane shows the selected chat's last message.
    assert!(
        screen.contains("Ada Lovelace · 17:55"),
        "the message pane header:\n{screen}"
    );

    // The key hints.
    assert!(screen.contains("q quit"), "the footer:\n{screen}");
}

#[test]
fn the_layout_survives_an_odd_size() {
    let mut app = fixture_app();
    let screen = render(&mut app, 37, 11);

    // 37 columns leaves the chat list too narrow for the badge, so this only asserts that
    // both panes rendered, at the right size, without panicking.
    assert!(
        screen.contains("Chats (3)"),
        "the chat pane is still there at 37x11:\n{screen}"
    );
    assert!(
        line_with(&screen, "★").contains("Ada"),
        "the first chat is still listed at 37x11:\n{screen}"
    );
    assert!(
        screen.contains("Ada Lovelace · 17:55"),
        "the message pane is still there at 37x11:\n{screen}"
    );
}

#[test]
fn a_terminal_too_small_to_draw_does_not_panic() {
    let mut app = fixture_app();
    for (width, height) in [(12, 4), (8, 3), (1, 1), (80, 2), (5, 24)] {
        let screen = render(&mut app, width, height);
        assert_eq!(
            screen.lines().count(),
            usize::from(height),
            "one line per buffer row at {width}x{height}"
        );
    }
}

#[test]
fn navigation_moves_the_selection_and_the_message_pane_follows() {
    let mut app = fixture_app();
    assert_eq!(app.selected(), 0);

    // j selects Grace Hopper, whose last message is our own.
    app.select_next();
    assert_eq!(app.selected(), 1);
    let screen = render(&mut app, 80, 24);
    assert!(
        screen.contains("you · 16:02"),
        "the message pane followed the selection:\n{screen}"
    );
    assert!(
        screen.contains("The compiler is done"),
        "the selected chat's message text:\n{screen}"
    );

    // k goes back.
    app.select_previous();
    assert_eq!(app.selected(), 0);

    // ...and the third chat has no messages at all.
    app.select_next();
    app.select_next();
    assert_eq!(app.selected(), 2);
    let screen = render(&mut app, 80, 24);
    assert!(
        screen.contains("(no messages)"),
        "a chat with no messages says so:\n{screen}"
    );
}

#[test]
fn selection_stops_at_both_ends() {
    let mut app = fixture_app();
    app.select_previous();
    assert_eq!(app.selected(), 0, "no selection before the first chat");
    for _ in 0..10 {
        app.select_next();
    }
    assert_eq!(
        app.selected(),
        app.chats().len() - 1,
        "no selection after the last chat"
    );
}

#[test]
fn tab_switches_panes_and_back() {
    let mut app = fixture_app();
    assert_eq!(app.pane(), Pane::Chats);
    app.toggle_pane();
    assert_eq!(app.pane(), Pane::Messages);
    app.toggle_pane();
    assert_eq!(app.pane(), Pane::Chats);
}

#[test]
fn a_daemon_snapshot_does_not_claim_to_be_fixture_data() {
    let snapshot = Snapshot {
        daemon: "nunciod/0.1.0".to_owned(),
        linked: false,
        session: Session::Unlinked,
        unread: 2,
        chats: vec![Chat {
            jid: "15551230001@s.whatsapp.net".to_owned(),
            name: "Ada Lovelace".to_owned(),
            unread: 2,
            archived: false,
            pinned: true,
            muted: false,
            last_message: Some(Message {
                id: "3EB0AAA1".to_owned(),
                kind: "text".to_owned(),
                text: "See you at 18:00".to_owned(),
                from_me: false,
                timestamp: "2026-10-18T17:55:04Z".to_owned(),
                media: None,
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
            }),
        }],
    };
    let mut app = App::new(snapshot, Source::Daemon, Palette::default());
    let screen = render(&mut app, 80, 24);

    assert!(
        screen.contains("nunciod/0.1.0"),
        "the daemon version:\n{screen}"
    );
    assert!(
        !screen.contains("FIXTURE DATA"),
        "a real daemon snapshot must not be labelled as fixture data:\n{screen}"
    );
}

#[test]
fn an_empty_chat_list_renders_at_eighty_by_twenty_four() {
    let snapshot = Snapshot {
        daemon: "nunciod/0.1.0".to_owned(),
        linked: false,
        session: Session::Unlinked,
        unread: 0,
        chats: Vec::new(),
    };
    let mut app = App::new(snapshot, Source::Daemon, Palette::default());
    let screen = render(&mut app, 80, 24);

    assert!(screen.contains("Chats (0)"), "the empty count:\n{screen}");
    assert_eq!(app.selected(), 0, "no chat is selected");
}

#[test]
fn an_inline_image_renders_without_panicking() {
    let image =
        image::load_from_memory(include_bytes!("fixtures/test.png")).expect("the test PNG decodes");
    let picker = ratatui_image::picker::Picker::halfblocks();
    let protocol = picker.new_resize_protocol(image);

    let mut app = fixture_app().with_image(protocol);
    let screen = render(&mut app, 80, 24);

    assert!(
        screen.contains("Ada Lovelace"),
        "the chat list is still there next to the image:\n{screen}"
    );
}
