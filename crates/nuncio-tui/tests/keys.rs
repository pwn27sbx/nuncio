//! The key scheme and the mouse (S8), and the chat markers' column (Part 2.2), through `App`'s
//! public interface and a `TestBackend` buffer.

use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Session, Snapshot};
use nuncio_tui::{Action, App, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::{Color, Modifier};

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
        daemon: "nunciod/0.1.0".to_owned(),
        linked: true,
        session: Session::Linked,
        unread: 0,
        chats,
        stop_reason: None,
    };
    App::new(snapshot, Source::Daemon, Palette::default())
}

fn two_chats() -> App {
    app_with(vec![chat(ADA, "Ada Lovelace"), chat(GRACE, "Grace Hopper")])
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        assert_eq!(app.handle_key(key(KeyCode::Char(c))), None, "typing {c:?}");
    }
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

fn three() -> Vec<Message> {
    vec![
        message("A1", "first words", false, 1),
        message("A2", "second words", true, 2),
        message("A3", "third words", false, 3),
    ]
}

/// Open Ada with Enter (the composer is focused) and load `oldest_first` into her chat.
fn open_ada(app: &mut App, oldest_first: Vec<Message>) {
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(ADA, 200)))
    );
    app.apply(&messages_reply(ADA, oldest_first));
}

fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| row(buffer, y) + "\n")
        .collect()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol().to_owned())
        .collect()
}

/// The first cell `(x, y)` whose row contains `needle` starting at x, searching from the left.
fn find(buffer: &Buffer, needle: &str) -> (u16, u16) {
    for y in 0..buffer.area.height {
        let cells: Vec<&str> = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        for x in 0..cells.len() {
            if cells[x..].concat().starts_with(needle) {
                return (x as u16, y);
            }
        }
    }
    panic!("{needle:?} is not on screen:\n{}", text(buffer));
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, (column, row): (u16, u16)) -> Option<Action> {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row))
}

fn accent() -> Color {
    let rgb = Palette::default().accent;
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// The column of the one line between the panes (it meets the header rule with `┬`).
fn split(buffer: &Buffer) -> u16 {
    (0..buffer.area.width)
        .find(|&x| buffer[(x, 1)].symbol() == "┬")
        .expect("the line between the panes")
}

// --- keys -------------------------------------------------------------------------------

/// Whether the pane title starting at column `x` of the header rule is drawn as the focused
/// one: bold and not dim (the other pane's title is dim), never in the accent.
fn focused_title(screen: &Buffer, x: u16) -> bool {
    let cell = &screen[(x, 1)];
    assert_ne!(cell.fg, accent(), "a title is never in the accent");
    cell.modifier.contains(Modifier::BOLD) && !cell.modifier.contains(Modifier::DIM)
}

#[test]
fn tab_switches_focus_and_only_the_focused_pane_has_the_bold_title() {
    let mut app = two_chats();
    let screen = render(&mut app, 80, 24);
    let right = split(&screen);
    // The titles sit on the header rule: `─ Chats (2) ─…┬─ Ada Lovelace ─…`.
    let chats_title = 2;
    let conversation_title = right + 3;
    assert_eq!(screen[(chats_title, 1)].symbol(), "C", "{}", text(&screen));
    assert_eq!(screen[(conversation_title, 1)].symbol(), "A");
    assert!(focused_title(&screen, chats_title), "the focused list");
    assert!(
        !focused_title(&screen, conversation_title),
        "the other pane is not highlighted"
    );

    assert_eq!(app.handle_key(key(KeyCode::Tab)), None);
    assert_eq!(app.pane(), Pane::Messages);
    let screen = render(&mut app, 80, 24);
    assert!(!focused_title(&screen, chats_title));
    assert!(focused_title(&screen, conversation_title));

    assert_eq!(app.handle_key(key(KeyCode::Tab)), None);
    assert_eq!(app.pane(), Pane::Chats);
}

#[test]
fn the_composer_counts_as_the_conversation_pane_for_the_focus() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    assert_eq!(app.pane(), Pane::Compose);
    let screen = render(&mut app, 80, 24);
    let right = split(&screen);
    assert!(!focused_title(&screen, 2));
    assert!(focused_title(&screen, right + 3));
}

#[test]
fn enter_on_a_chat_opens_it_with_the_composer_focused_ready_to_type() {
    let mut app = two_chats().with_client_prefix("t");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(GRACE, 200))),
        "Enter asks for the chat's messages"
    );
    assert_eq!(
        app.pane(),
        Pane::Compose,
        "and the composer has the keyboard"
    );
    type_text(&mut app, "hi");
    assert_eq!(app.composer(), "hi");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::send(GRACE, "hi", "t-1")))
    );
}

#[test]
fn j_k_and_the_arrows_select_messages_in_the_conversation_and_shift_k_j_do_nothing() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(app.pane(), Pane::Messages);
    // The conversation row of the selected message, marked by the `▌` bar: its text.
    let selected = |app: &mut App| -> Option<String> {
        let screen = render(app, 80, 24);
        let right = split(&screen);
        let conversation = |y: u16| -> String {
            (right + 1..screen.area.width)
                .map(|x| screen[(x, y)].symbol())
                .collect()
        };
        (0..screen.area.height)
            .find(|&y| conversation(y).starts_with("▌ "))
            .map(conversation)
    };

    // Shift-K / Shift-J are gone: they select nothing.
    app.handle_key(shift('K'));
    app.handle_key(shift('J'));
    assert_eq!(selected(&mut app), None, "K/J select nothing");
    let selected = |app: &mut App| selected(app).expect("a message is selected");

    app.handle_key(key(KeyCode::Char('k')));
    assert!(
        selected(&mut app).contains("third words"),
        "k selects the newest"
    );
    app.handle_key(key(KeyCode::Up));
    assert!(selected(&mut app).contains("second words"), "↑ one older");
    app.handle_key(key(KeyCode::Char('k')));
    assert!(selected(&mut app).contains("first words"));
    app.handle_key(key(KeyCode::Char('j')));
    assert!(selected(&mut app).contains("second words"), "j one newer");
    app.handle_key(key(KeyCode::Down));
    assert!(selected(&mut app).contains("third words"), "↓ one newer");
    // The list's selection never moved.
    assert_eq!(app.selected(), 0);
}

#[test]
fn j_k_in_the_chat_list_move_between_chats() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected(), 1);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected(), 0);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected(), 1);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.selected(), 0);
}

#[test]
fn r_replies_to_the_message_selected_with_k() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('r')));
    assert_eq!(app.pane(), Pane::Compose);
    type_text(&mut app, "ok");
    let Some(Action::Request(ClientFrame::Send { reply_to, .. })) =
        app.handle_key(key(KeyCode::Enter))
    else {
        panic!("Enter sends the reply");
    };
    assert_eq!(reply_to.as_deref(), Some("A3"));
}

#[test]
fn esc_goes_from_the_composer_to_the_conversation_then_to_the_list() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(app.pane(), Pane::Messages);
    // Even with a message selected, the next Esc goes to the list.
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.handle_key(key(KeyCode::Esc)), None);
    assert_eq!(app.pane(), Pane::Chats);
}

#[test]
fn q_and_slash_are_text_in_the_composer_and_ctrl_c_quits_anywhere() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    type_text(&mut app, "q/?");
    assert_eq!(app.composer(), "q/?");
    assert_eq!(app.pane(), Pane::Compose);
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.handle_key(ctrl_c), Some(Action::Quit));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.handle_key(ctrl_c), Some(Action::Quit));
    assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
    let mut app = two_chats();
    assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
}

#[test]
fn the_footer_offers_the_panes_keys_and_drops_the_tail_before_the_primary_one() {
    let mut app = two_chats();
    for (width, height) in [(80, 24), (240, 60)] {
        let screen = render(&mut app, width, height);
        let footer = row(&screen, height - 1);
        assert!(footer.trim_start().starts_with("Enter open"), "{footer:?}");
        assert!(footer.contains("? all keys"), "{footer:?}");
        assert!(footer.trim_end().ends_with("linked"), "{footer:?}");
    }
    let footer = row(&render(&mut app, 240, 60), 59);
    assert!(footer.contains("/ search"), "the list's keys: {footer:?}");
    // Narrow: the tail drops, the primary key and `? all keys` stay, the state never moves.
    let footer = row(&render(&mut app, 40, 12), 11);
    assert!(footer.contains("Enter open"), "{footer:?}");
    assert!(footer.contains("? all keys"), "{footer:?}");
    assert!(!footer.contains("j/k"), "{footer:?}");
    assert!(footer.trim_end().ends_with("linked"), "{footer:?}");
    assert_eq!(footer.matches("Enter").count(), 1, "{footer:?}");

    // The conversation's keys replace the list's.
    app.handle_key(key(KeyCode::Tab));
    let footer = row(&render(&mut app, 240, 60), 59);
    assert!(footer.contains("Enter write"), "{footer:?}");
    assert!(footer.contains("j/k select"), "{footer:?}");
    assert!(!footer.contains("q quit"), "{footer:?}");
    // With a message selected, the message keys follow.
    let mut app = two_chats();
    open_ada(&mut app, three());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    let footer = row(&render(&mut app, 240, 60), 59);
    assert!(footer.contains("r reply · e react"), "{footer:?}");
    // In the composer, q and / are text: the hint says how to leave instead.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane(), Pane::Compose);
    let footer = row(&render(&mut app, 240, 60), 59);
    assert!(footer.contains("Enter send"), "{footer:?}");
    assert!(footer.contains("Esc conversation"), "{footer:?}");
    assert!(!footer.contains("q quit"), "{footer:?}");
}

#[test]
fn the_help_is_grouped_by_context_and_fits_at_eighty_by_twenty_four() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('?')));
    let screen = render(&mut app, 80, 24);
    let shown = text(&screen);
    let mut last = 0;
    for heading in [
        "Chats",
        "Conversation",
        "Composer",
        "Search",
        "Reactions",
        "Everywhere",
    ] {
        let (_, y) = find(&screen, heading);
        assert!(
            y > last || heading == "Chats",
            "{heading} in order:\n{shown}"
        );
        last = y;
    }
    for needle in ["Tab", "Enter", "send", "Ctrl-C", "Shift"] {
        assert!(shown.contains(needle), "{needle:?}:\n{shown}");
    }
    // The help's last key is on screen, above the footer: nothing was cut off.
    let (_, last_key) = find(&screen, "selects text");
    assert!(last_key < 23, "{shown}");
    let (_, reactions) = find(&screen, "Reactions");
    let (_, ctrl_c) = find(&screen, "Ctrl-C");
    assert!(ctrl_c > reactions, "{shown}");
    assert!(!shown.contains("K/J"), "K/J are gone:\n{shown}");
}

// --- mouse ------------------------------------------------------------------------------

#[test]
fn a_click_on_a_chat_row_selects_and_opens_it_with_the_composer_focused() {
    let mut app = two_chats();
    let screen = render(&mut app, 80, 24);
    // Either of the chat's two rows (its name, its last message).
    let (x, y) = find(&screen, "Grace Hopper");
    assert_eq!(
        click(&mut app, (x, y + 1)),
        Some(Action::Request(ClientFrame::messages(GRACE, 200)))
    );
    assert_eq!(app.selected(), 1);
    assert_eq!(app.pane(), Pane::Compose);

    // Below the last chat there is no chat to open.
    let mut app = two_chats();
    render(&mut app, 80, 24);
    assert_eq!(click(&mut app, (3, 15)), None);
    assert_eq!(app.selected(), 0);
    assert_eq!(app.pane(), Pane::Chats);
}

#[test]
fn a_click_in_the_conversation_focuses_it_and_selects_the_clicked_message() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    let screen = render(&mut app, 80, 24);
    let at = find(&screen, "second words");
    assert_eq!(click(&mut app, at), None);
    assert_eq!(app.pane(), Pane::Messages);
    let shown = text(&render(&mut app, 80, 24));
    assert!(
        shown.contains("▌ you · 17:02"),
        "the clicked message:\n{shown}"
    );
    // `r` now replies to it.
    app.handle_key(key(KeyCode::Char('r')));
    type_text(&mut app, "ok");
    let Some(Action::Request(ClientFrame::Send { reply_to, .. })) =
        app.handle_key(key(KeyCode::Enter))
    else {
        panic!("sends");
    };
    assert_eq!(reply_to.as_deref(), Some("A2"));

    // A click on a header row selects that message too.
    app.handle_key(key(KeyCode::Tab));
    let screen = render(&mut app, 80, 24);
    let at = find(&screen, "Ada Lovelace · 17:01");
    click(&mut app, at);
    assert_eq!(app.pane(), Pane::Messages);
    let shown = text(&render(&mut app, 80, 24));
    assert!(shown.contains("▌ Ada Lovelace · 17:01"), "{shown}");
}

#[test]
fn a_click_in_the_composer_focuses_it() {
    let mut app = two_chats();
    open_ada(&mut app, three());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Chats);
    let screen = render(&mut app, 80, 24);
    let at = find(&screen, "Enter to write");
    assert_eq!(click(&mut app, at), None);
    assert_eq!(app.pane(), Pane::Compose);
    type_text(&mut app, "x");
    assert_eq!(app.composer(), "x");
}

#[test]
fn the_wheel_scrolls_the_pane_under_the_pointer() {
    let many: Vec<Message> = (0..60)
        .map(|n| message(&format!("M{n:02}"), &format!("message {n:02}"), false, n))
        .collect();
    let mut app = two_chats();
    open_ada(&mut app, many);
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 80, 24);
    assert!(text(&screen).contains("message 59"));
    let right = split(&screen);
    let over_conversation = (right + 10, 10);
    for _ in 0..4 {
        app.handle_mouse(mouse(
            MouseEventKind::ScrollUp,
            over_conversation.0,
            over_conversation.1,
        ));
    }
    let shown = text(&render(&mut app, 80, 24));
    assert!(!shown.contains("message 59"), "scrolled up:\n{shown}");
    assert_eq!(app.selected(), 0, "the chat list did not move");
    for _ in 0..4 {
        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            over_conversation.0,
            over_conversation.1,
        ));
    }
    assert!(text(&render(&mut app, 80, 24)).contains("message 59"));

    // Over the chat list the wheel moves through the chats; the conversation stays.
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 3, 5));
    assert_eq!(app.selected(), 1);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 3, 5));
    assert_eq!(app.selected(), 0);
    assert!(text(&render(&mut app, 80, 24)).contains("message 59"));
}

#[test]
fn the_mouse_does_nothing_while_the_help_is_open() {
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('?')));
    render(&mut app, 80, 24);
    // The first column inside the chat list, on Ada's row, left of the help.
    assert_eq!(click(&mut app, (1, 2)), None);
    assert_eq!(app.pane(), Pane::Chats);
}

// --- chat markers ------------------------------------------------------------------------

#[test]
fn chat_markers_follow_the_name_in_their_own_order() {
    let family = "👨‍👩‍👧‍👦";
    let mut pinned = chat(ADA, &format!("{family} Family group with a long name"));
    pinned.pinned = true;
    pinned.unread = 3;
    let mut muted = chat(GRACE, "李小龍 漢字テスト");
    muted.muted = true;
    let mut archived = chat(FAMILY, "Bo");
    archived.archived = true;
    archived.pinned = true;
    archived.muted = true;
    for (width, height) in [(80u16, 24u16), (240, 60)] {
        let mut app = app_with(vec![pinned.clone(), muted.clone(), archived.clone()]);
        let screen = render(&mut app, width, height);
        let right = split(&screen);
        let rows: Vec<String> = (2..height - 2)
            .map(|y| (0..right).map(|x| screen[(x, y)].symbol()).collect())
            .collect();
        let shown = text(&screen);
        let row = |needle: &str| -> String {
            rows.iter()
                .find(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("no {needle:?} row at {width}x{height}:\n{shown}"))
                .clone()
        };
        // Only the markers the chat has, right after its name.
        let run =
            |row: &str| -> String { row.chars().filter(|c| "★⊘▤".contains(*c)).collect() };
        assert_eq!(run(&row("Family")), "★", "{width}x{height}:\n{shown}");
        assert_eq!(run(&row("Bo")), "★⊘▤", "{width}x{height}:\n{shown}");
        // The muted chat is the only row with a ⊘ and nothing else.
        let muted = rows
            .iter()
            .find(|row| row.contains('⊘'))
            .unwrap_or_else(|| panic!("no muted row:\n{shown}"));
        assert_eq!(run(muted), "⊘", "{width}x{height}:\n{shown}");
        // On the name row, never on the preview row.
        assert!(!row("(no messages)").contains('★'), "{width}x{height}");
    }
}

#[test]
fn the_conversation_scrolls_back_to_the_newest_after_a_send_or_another_chat() {
    let mut app = two_chats();
    let many: Vec<Message> = (0..30)
        .map(|n| message(&format!("M{n}"), &format!("line {n:02}"), false, n))
        .collect();
    open_ada(&mut app, many);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Messages);
    let scrolled_up = |app: &mut App| {
        app.handle_key(key(KeyCode::PageUp));
        app.handle_key(key(KeyCode::PageUp));
        let shown = text(&render(app, 80, 24));
        assert!(!shown.contains("line 29"), "scrolled up:\n{shown}");
    };
    scrolled_up(&mut app);
    // Another chat and back: Ada's conversation shows its newest message again.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('k')));
    let shown = text(&render(&mut app, 80, 24));
    assert!(shown.contains("line 29"), "back at the newest:\n{shown}");
    // A send goes back to the newest too, so it is seen.
    app.handle_key(key(KeyCode::Tab));
    scrolled_up(&mut app);
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "fresh words");
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::Send { .. }))
    ));
    let shown = text(&render(&mut app, 80, 24));
    assert!(
        shown.contains("fresh words"),
        "the send is on screen:\n{shown}"
    );
}

#[test]
fn in_a_single_pane_the_hidden_pane_takes_no_clicks() {
    let mut app = two_chats();
    let screen = render(&mut app, 80, 24);
    let (x, y) = find(&screen, "Grace Hopper");
    let _ = click(&mut app, (x, y + 1));
    assert_eq!(
        app.selected(),
        1,
        "a click opens the second chat at 80 columns"
    );
    // Leave the composer for the conversation, then shrink to one pane.
    app.handle_key(key(KeyCode::Esc));
    let _ = render(&mut app, 40, 12);
    assert_eq!(app.pane(), Pane::Messages);
    // The left half is where the chat list would be: it is not drawn, so it takes nothing.
    assert_eq!(click(&mut app, (3, 4)), None);
    assert_eq!(app.selected(), 1, "the hidden list took no click");
    assert_eq!(app.pane(), Pane::Messages);
}

#[test]
fn the_chat_list_filters_to_unread_and_archived() {
    // Ada has 2 unread, Grace none, nobody archived.
    let mut ada = chat(ADA, "Ada Lovelace");
    ada.unread = 2;
    let mut app = app_with(vec![ada, chat(GRACE, "Grace Hopper")]);
    let buffer = render(&mut app, 80, 24);
    assert!(text(&buffer).contains("Chats (2)"), "{}", text(&buffer));

    app.handle_key(key(KeyCode::Char(']')));
    let buffer = render(&mut app, 80, 24);
    let shown = text(&buffer);
    assert!(shown.contains("Chats · Unread (1)"), "{shown}");
    assert!(shown.contains("Ada Lovelace"), "{shown}");
    assert!(!shown.contains("Grace Hopper"), "{shown}");

    // A click on the row under the filter opens the chat it is drawn for.
    let (x, y) = find(&buffer, "Ada Lovelace");
    let _ = click(&mut app, (x, y + 1));
    assert_eq!(app.selected(), 0);

    // Nothing is archived: the list says why it is empty instead of pretending there are none.
    app.handle_key(key(KeyCode::Char(']')));
    let shown = text(&render(&mut app, 80, 24));
    assert!(shown.contains("Chats · Archived (0)"), "{shown}");
    assert!(shown.contains("No chats under this filter"), "{shown}");

    // Back around to All.
    app.handle_key(key(KeyCode::Char(']')));
    assert!(text(&render(&mut app, 80, 24)).contains("Chats (2)"));
    app.handle_key(key(KeyCode::Char('[')));
    assert!(text(&render(&mut app, 80, 24)).contains("Chats · Archived (0)"));
}

#[test]
fn the_filter_walks_up_and_down_without_landing_on_a_hidden_chat() {
    // In the model: B unread, A read, C unread.
    let mut b = chat("1@s.whatsapp.net", "Bea");
    b.unread = 1;
    let a = chat("2@s.whatsapp.net", "Ann");
    let mut c = chat("3@s.whatsapp.net", "Cyd");
    c.unread = 1;
    let mut app = app_with(vec![b, a, c]);
    app.handle_key(key(KeyCode::Char(']')));
    let shown = text(&render(&mut app, 80, 24));
    assert!(shown.contains("Chats · Unread (2)"), "{shown}");

    // `j` from the first unread (B) goes to the next unread (C), not through the read one.
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected(), 2, "{shown}");
    // `k` comes back to B, not stuck on C.
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected(), 0);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected(), 0, "it stops at the first listed chat");
}

#[test]
fn a_filter_that_hides_everything_selects_no_hidden_chat() {
    // A read, B unread, C read: under Unread only B is listed.
    let a = chat("1@s.whatsapp.net", "Ann");
    let mut b = chat("2@s.whatsapp.net", "Bea");
    b.unread = 1;
    let c = chat("3@s.whatsapp.net", "Cyd");
    let mut app = app_with(vec![a, b, c]);
    // Start on C, then filter to Unread.
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected(), 2);
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(app.selected(), 1, "the only listed chat, not index 0");

    // Archived lists nothing here: the selection stays, nothing hidden is chosen.
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(app.selected(), 1);
    let shown = text(&render(&mut app, 80, 24));
    assert!(shown.contains("No chats under this filter"), "{shown}");
}
