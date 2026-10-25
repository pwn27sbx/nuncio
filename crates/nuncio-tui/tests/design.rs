//! The terminal's half of the redesign (design plan §9): one line between panes and no boxes,
//! the palette roles for the tones, the accent only for the unread dot, the selection bar and the
//! one primary hint, one line per message with day labels, a one-line footer, a scrim behind the
//! overlays, and one-line empty states. Checked on `TestBackend` buffers at 80x24 and 240x60.

use nuncio_client::{Chat, DaemonFrame, Message, Palette, Session, Snapshot};
use nuncio_tui::images::Graphics;
use nuncio_tui::{App, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};

const ADA: &str = "15551230001@s.whatsapp.net";
const GRACE: &str = "15551230002@s.whatsapp.net";
const GROUP: &str = "120363000000000001@g.us";

/// The two sizes the plan checks the TUI at (§10 P5).
const SIZES: [(u16, u16); 2] = [(80, 24), (240, 60)];

/// Box corners and every heavy or double line: none of them is drawn anywhere (§9.1).
const BOXES: &[&str] = &[
    "┌", "┐", "└", "┘", "┏", "┓", "┗", "┛", "━", "┃", "╔", "╗", "╚", "╝", "═", "║", "╭", "╮", "╰",
    "╯", "┍", "┑", "┕", "┙", "┎", "┒", "┖", "┚",
];

fn message(id: &str, text: &str, from_me: bool, timestamp: &str) -> Message {
    Message {
        id: id.to_owned(),
        kind: "text".to_owned(),
        text: text.to_owned(),
        from_me,
        timestamp: timestamp.to_owned(),
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

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Open the first chat (Enter) and load `oldest_first` as its messages; the composer has the
/// focus, as after Enter.
fn open(app: &mut App, jid: &str, oldest_first: Vec<Message>) {
    assert!(app.handle_key(key(KeyCode::Enter)).is_some());
    let mut messages = oldest_first;
    messages.reverse();
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: jid.to_owned(),
        messages,
    });
}

/// A conversation with Ada: two of hers, one of mine, the next day one more of hers.
fn conversation() -> Vec<Message> {
    vec![
        message("A1", "hello there", false, "2026-10-17T17:01:00Z"),
        message("A2", "how are you", false, "2026-10-17T17:02:00Z"),
        message("M1", "fine, thanks", true, "2026-10-17T17:03:00Z"),
        message("A3", "good morning", false, "2026-10-18T09:15:00Z"),
    ]
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    terminal.backend().buffer().clone()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

fn text(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| row(buffer, y) + "\n")
        .collect()
}

/// The first row holding `needle`.
fn row_of(buffer: &Buffer, needle: &str) -> u16 {
    (0..buffer.area.height)
        .find(|&y| row(buffer, y).contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{}", text(buffer)))
}

/// The cell where `needle` starts.
fn find(buffer: &Buffer, needle: &str) -> (u16, u16) {
    let y = row_of(buffer, needle);
    let cells: Vec<&str> = (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect();
    let first = needle.chars().next().expect("a needle").to_string();
    let x = (0..cells.len())
        .find(|&x| cells[x] == first && cells[x..].concat().starts_with(needle))
        .expect("the needle's first cell");
    (x as u16, y)
}

fn colour(rgb: nuncio_client::Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// The column of the one line between the panes: the `│` on the first conversation row.
fn separator(buffer: &Buffer) -> u16 {
    (0..buffer.area.width)
        .find(|&x| buffer[(x, 3)].symbol() == "│")
        .unwrap_or_else(|| panic!("no pane separator:\n{}", text(buffer)))
}

/// Every screen the TUI shows, at `width` x `height`: the list focused, the conversation with a
/// selected message, the composer with a draft, the help, the reaction picker and the search.
fn screens(width: u16, height: u16) -> Vec<(&'static str, Buffer)> {
    let mut shots = Vec::new();
    let mut app = two_chats();
    shots.push(("chat list", draw(&mut app, width, height)));
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.pane(), Pane::Messages);
    shots.push(("conversation", draw(&mut app, width, height)));
    app.handle_key(key(KeyCode::Char('i')));
    for c in "a draft".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    shots.push(("composer", draw(&mut app, width, height)));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('?')));
    shots.push(("help", draw(&mut app, width, height)));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('e')));
    shots.push(("picker", draw(&mut app, width, height)));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('/')));
    shots.push(("search", draw(&mut app, width, height)));
    shots
}

#[test]
fn no_pane_is_boxed_and_no_heavy_or_double_line_is_drawn() {
    for (width, height) in SIZES {
        for (name, buffer) in screens(width, height) {
            let boxes: Vec<(u16, u16, &str)> = buffer
                .content()
                .iter()
                .enumerate()
                .filter(|(_, cell)| BOXES.contains(&cell.symbol()))
                .map(|(i, cell)| {
                    let (x, y) = buffer.pos_of(i);
                    (x, y, cell.symbol())
                })
                .collect();
            assert!(
                boxes.is_empty(),
                "{name} at {width}x{height} draws {boxes:?}:\n{}",
                text(&buffer)
            );
        }
    }
}

#[test]
fn one_single_line_separates_the_panes_from_the_header_rule_to_the_footer() {
    for (width, height) in SIZES {
        for (name, buffer) in screens(width, height)
            .into_iter()
            .filter(|(name, _)| ["chat list", "conversation", "composer"].contains(name))
        {
            let x = separator(&buffer);
            // Row 0 is the header, row 1 the header rule, the last row the footer.
            assert_eq!(buffer[(x, 1)].symbol(), "┬", "{name}:\n{}", text(&buffer));
            for y in 2..height - 1 {
                assert!(
                    ["│", "├"].contains(&buffer[(x, y)].symbol()),
                    "{name} at {width}x{height}, row {y}:\n{}",
                    text(&buffer)
                );
            }
            // Nothing of the list spills over the line, nothing of the conversation under it.
            assert_eq!(
                (0..height)
                    .filter(|&y| buffer[(x, y)].symbol() == "─")
                    .count(),
                0,
                "{name}"
            );
            // The header rule is one row of `─` on both sides of the line.
            assert_eq!(buffer[(x - 1, 1)].symbol(), "─", "{name}");
            assert_eq!(buffer[(width - 1, 1)].symbol(), "─", "{name}");
        }
    }
}

#[test]
fn the_accent_marks_only_the_unread_dot_the_selection_bar_and_the_one_primary_hint() {
    let accent = colour(Palette::default().accent());
    for (width, height) in SIZES {
        for (name, buffer) in screens(width, height) {
            let footer = height - 1;
            let mut hint = String::new();
            let mut header = String::new();
            for y in 0..height {
                for x in 0..width {
                    let cell = &buffer[(x, y)];
                    if cell.fg != accent && cell.bg != accent {
                        continue;
                    }
                    if y == footer {
                        hint.push_str(cell.symbol());
                        continue;
                    }
                    // The header's unread count is the other accent text (Q6).
                    if y == 0 {
                        header.push_str(cell.symbol());
                        continue;
                    }
                    assert!(
                        ["●", "▌"].contains(&cell.symbol()) && cell.bg != accent,
                        "{name} at {width}x{height}: {:?} at ({x}, {y}) is in the accent:\n{}",
                        cell.symbol(),
                        text(&buffer)
                    );
                }
            }
            // The footer's accent is one key, the primary one where the focus is.
            let primary = if name == "picker" { "1-6" } else { "Enter" };
            assert_eq!(hint, primary, "{name} at {width}x{height}");
            assert_eq!(header, "2 unread", "{name} at {width}x{height}");
        }
    }
    // And they are drawn: the unread chat's dot and the selected chat's bar.
    let buffer = draw(&mut two_chats(), 80, 24);
    let (x, y) = find(&buffer, "Ada Lovelace (2)");
    let row: Vec<_> = (0..x).map(|x| buffer[(x, y)].clone()).collect();
    assert!(
        row.iter().any(|c| c.symbol() == "●" && c.fg == accent),
        "the unread dot:\n{}",
        text(&buffer)
    );
    assert!(
        row.iter().any(|c| c.symbol() == "▌" && c.fg == accent),
        "the selection bar:\n{}",
        text(&buffer)
    );
    // A read chat has no dot.
    let (x, y) = find(&buffer, "Grace Hopper");
    assert!((0..x).all(|x| buffer[(x, y)].symbol() != "●"));
}

#[test]
fn the_tones_are_the_palette_roles() {
    let palette = Palette::default();
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.pane(), Pane::Chats);
    let buffer = draw(&mut app, 80, 24);
    let sep = separator(&buffer);
    // The selected chat (Ada) on `surface_active`, the other on `surface_chrome`.
    // Ada is the first chat, on the rows above Grace (her name is in the header rule too).
    let (_, grace) = find(&buffer, "Grace Hopper");
    let ada = grace - 2;
    assert!(row(&buffer, ada).contains("Ada Lovelace"));
    for x in 1..sep {
        assert_eq!(
            buffer[(x, ada)].bg,
            colour(palette.surface_active()),
            "x {x}"
        );
        assert_eq!(buffer[(x, ada + 1)].bg, colour(palette.surface_active()));
        assert_eq!(
            buffer[(x, grace)].bg,
            colour(palette.surface_chrome()),
            "x {x}"
        );
    }
    // The bar runs down both rows of the selected chat.
    assert_eq!(buffer[(0, ada)].symbol(), "▌");
    assert_eq!(buffer[(0, ada + 1)].symbol(), "▌");
    // The transcript is on `surface`, the line between the panes in `border`.
    let (x, y) = find(&buffer, "good morning");
    assert_eq!(buffer[(x, y)].bg, colour(palette.surface()));
    assert_eq!(
        buffer[(width_of(&buffer) - 2, y)].bg,
        colour(palette.surface())
    );
    assert_eq!(buffer[(sep, y)].fg, colour(palette.border()));
}

fn width_of(buffer: &Buffer) -> u16 {
    buffer.area.width
}

#[test]
fn each_message_is_one_line_with_its_author_and_time_muted_then_its_text() {
    let palette = Palette::default();
    let muted = colour(palette.text_muted());
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        for (meta, said) in [
            ("Ada Lovelace · 17:01", "hello there"),
            ("Ada Lovelace · 17:02", "how are you"),
            ("you · 17:03", "fine, thanks"),
            ("Ada Lovelace · 09:15", "good morning"),
        ] {
            let (x, y) = find(&buffer, meta);
            let (text_x, text_y) = find(&buffer, said);
            assert_eq!(
                text_y,
                y,
                "{meta} and {said} share a row:\n{}",
                text(&buffer)
            );
            assert!(text_x > x);
            // The name in bold, the time dim, the text neither.
            assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD), "{meta}");
            let time = find(&buffer, meta).0 + meta.find('·').expect("a dot") as u16;
            let time = (time..x + meta.chars().count() as u16)
                .find(|&x| buffer[(x, y)].symbol().chars().all(|c| c.is_ascii_digit()))
                .expect("the time");
            assert_eq!(buffer[(time, y)].fg, muted, "{meta}");
            assert_eq!(buffer[(text_x, y)].fg, colour(palette.text()));
            assert!(!buffer[(text_x, y)].modifier.contains(Modifier::BOLD));
        }
    }
}

#[test]
fn an_authors_run_has_no_blank_line_and_the_next_author_starts_after_one() {
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let sep = separator(&buffer);
        let blank = |y: u16| (sep + 1..width).all(|x| buffer[(x, y)].symbol() == " ");
        let first = row_of(&buffer, "hello there");
        let second = row_of(&buffer, "how are you");
        let mine = row_of(&buffer, "fine, thanks");
        assert_eq!(
            second,
            first + 1,
            "one author, no blank line:\n{}",
            text(&buffer)
        );
        assert_eq!(mine, second + 2, "a new author after one blank line");
        assert!(blank(second + 1), "{}", text(&buffer));
    }
}

#[test]
fn each_day_starts_with_its_label_in_muted_caps_after_a_blank_line() {
    let palette = Palette::default();
    let muted = colour(palette.text_muted());
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let sep = separator(&buffer);
        let blank = |y: u16| (sep + 1..width).all(|x| buffer[(x, y)].symbol() == " ");
        let (x, saturday) = find(&buffer, "SAT 17 OCT 2026");
        assert_eq!(saturday + 1, row_of(&buffer, "hello there"));
        assert_eq!(buffer[(x, saturday)].fg, muted);
        let sunday = row_of(&buffer, "SUN 18 OCT 2026");
        assert!(
            blank(sunday - 1),
            "a blank line before the label:\n{}",
            text(&buffer)
        );
        assert_eq!(sunday - 2, row_of(&buffer, "fine, thanks"));
        assert_eq!(sunday + 1, row_of(&buffer, "good morning"));
    }
    // The day is the local one: 01:00 UTC is still Saturday three hours west of Greenwich.
    let mut app = two_chats().with_utc_offset(time::UtcOffset::from_hms(-3, 0, 0).expect("-3h"));
    open(
        &mut app,
        ADA,
        vec![message("L1", "late", false, "2026-10-18T01:00:00Z")],
    );
    let buffer = draw(&mut app, 80, 24);
    assert_eq!(
        row_of(&buffer, "SAT 17 OCT 2026") + 1,
        row_of(&buffer, "late")
    );
}

#[test]
fn senders_are_not_colour_coded() {
    let mut app = app_with(vec![chat(GROUP, "Omarchy devs", 0)]);
    let mut one = message("G1", "first", false, "2026-10-18T17:01:00Z");
    one.sender = Some("Ada".to_owned());
    let mut two = message("G2", "second", false, "2026-10-18T17:02:00Z");
    two.sender = Some("Grace".to_owned());
    let mine = message("G3", "third", true, "2026-10-18T17:03:00Z");
    open(&mut app, GROUP, vec![one, two, mine]);
    let buffer = draw(&mut app, 80, 24);
    let fg = |needle: &str| {
        let (x, y) = find(&buffer, needle);
        (buffer[(x, y)].fg, buffer[(x, y)].modifier)
    };
    assert_eq!(fg("Ada ·"), fg("Grace ·"), "{}", text(&buffer));
    assert_eq!(fg("Ada ·"), fg("you ·"));
}

#[test]
fn the_footer_is_one_line_of_muted_keys_with_the_session_state_on_the_right() {
    let muted = colour(Palette::default().text_muted());
    for (width, height) in SIZES {
        let mut app = two_chats().with_graphics(Graphics::halfblocks());
        let buffer = draw(&mut app, width, height);
        let footer = height - 1;
        let shown = row(&buffer, footer);
        assert!(shown.trim_end().ends_with("linked"), "{shown:?}");
        assert!(
            !row(&buffer, footer - 1).contains("all keys"),
            "one line only:\n{}",
            text(&buffer)
        );
        // Every key is muted but the primary one.
        let (x, _) = find(&buffer, "? all keys");
        assert_eq!(find(&buffer, "? all keys").1, footer);
        assert_eq!(buffer[(x, footer)].fg, muted);
        // The old dump is gone: no key twice, and the picture protocol is not a footer datum.
        for gone in ["q quit", "Ctrl-C quit", "Tab switch pane", "pictures:"] {
            assert!(!shown.contains(gone), "{gone:?} in {shown:?}");
        }
    }
    // The protocol and the version moved into `?`.
    let mut app = two_chats().with_graphics(Graphics::halfblocks());
    app.handle_key(key(KeyCode::Char('?')));
    let buffer = draw(&mut app, 240, 60);
    assert!(
        text(&buffer).contains("pictures: half blocks"),
        "{}",
        text(&buffer)
    );
    assert!(text(&buffer).contains("nunciod/"), "{}", text(&buffer));
}

#[test]
fn the_help_sits_on_a_solid_surface_scrim_with_its_keys_right_aligned_and_muted() {
    let palette = Palette::default();
    let muted = colour(palette.text_muted());
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('?')));
    let buffer = draw(&mut app, 240, 60);
    // The scrim hides the panes behind and is the solid `surface`.
    assert!(!text(&buffer).contains("Grace Hopper"), "{}", text(&buffer));
    assert_eq!(buffer[(0, 10)].bg, colour(palette.surface()));
    assert_eq!(buffer[(0, 10)].symbol(), " ");
    // One key per row, right-aligned, muted, after what it does.
    let y = row_of(&buffer, "this help");
    let shown = row(&buffer, y);
    assert!(shown.trim_end().ends_with('?'), "{shown:?}");
    let x = shown.trim_end().chars().count() as u16 - 1;
    assert_eq!(buffer[(x, y)].fg, muted);
    // At 80x24 every key still fits (wrapped), on the same scrim.
    let buffer = draw(&mut app, 80, 24);
    assert!(text(&buffer).contains("this help"), "{}", text(&buffer));
    assert!(text(&buffer).contains("selects text"), "{}", text(&buffer));
    assert!(!text(&buffer).contains("Grace Hopper"));
}

#[test]
fn empty_states_are_one_muted_line_with_the_key_hint() {
    let muted = colour(Palette::default().text_muted());
    // No chats: the list says so, the conversation too.
    let mut app = app_with(Vec::new());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let (x, y) = find(&buffer, "No chats yet");
        assert_eq!(buffer[(x, y)].fg, muted);
    }
    // A chat with no messages.
    let mut app = two_chats();
    open(&mut app, ADA, Vec::new());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "No messages yet · i to write");
    assert_eq!(buffer[(x, y)].fg, muted);
}

#[test]
fn a_reply_shows_its_quote_between_its_header_and_its_text_inside_an_authors_run() {
    let mut app = two_chats();
    let mut reply = message("A2", "line one\nline two", false, "2026-10-17T17:02:00Z");
    reply.reply_to = Some("A1".to_owned());
    open(
        &mut app,
        ADA,
        vec![
            message("A1", "the question", false, "2026-10-17T17:01:00Z"),
            reply,
            message("A3", "afterwards", false, "2026-10-17T17:03:00Z"),
        ],
    );
    app.handle_key(key(KeyCode::Esc));
    // Select the reply: k selects the newest, k once more the one before it.
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    let palette = Palette::default();
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let header = row_of(&buffer, "Ada Lovelace · 17:02");
        let quote = row_of(&buffer, "> the question");
        let one = row_of(&buffer, "line one");
        let two = row_of(&buffer, "line two");
        let next = row_of(&buffer, "Ada Lovelace · 17:03");
        assert_eq!(
            (quote, one, two, next),
            (header + 1, header + 2, header + 3, header + 4),
            "header, quote, text, then the next message of the run:\n{}",
            text(&buffer)
        );
        assert!(!row(&buffer, header).contains("line one"));
        // The quote is part of the selected reply, not of the next message.
        let sep = separator(&buffer);
        let active = colour(palette.surface_active());
        for y in [header, quote, one, two] {
            assert_eq!(buffer[(width - 2, y)].bg, active, "row {y}");
        }
        assert_ne!(buffer[(width - 2, next)].bg, active);
        assert!(sep < width);
    }
}

#[test]
fn with_no_chats_the_focused_conversation_still_shows_a_bold_title() {
    let muted = colour(Palette::default().text_muted());
    let mut app = app_with(Vec::new());
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.pane(), Pane::Messages);
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "Conversation");
    assert_eq!(y, 1, "on the header rule:\n{}", text(&buffer));
    assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD));
    assert_ne!(buffer[(x, y)].fg, muted);
    let (x, y) = find(&buffer, "Chats (0)");
    assert_eq!(buffer[(x, y)].fg, muted);
    app.handle_key(key(KeyCode::Tab));
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "Conversation");
    assert_eq!(buffer[(x, y)].fg, muted, "unfocused");
}

#[test]
fn the_selected_messages_reactions_row_is_on_the_active_surface_too() {
    let mut app = two_chats();
    let mut reacted = message("A1", "react to me", false, "2026-10-17T17:01:00Z");
    reacted.reactions = vec![nuncio_client::Reaction {
        emoji: "👍".to_owned(),
        from_me: false,
    }];
    open(&mut app, ADA, vec![reacted]);
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    let buffer = draw(&mut app, 80, 24);
    let y = row_of(&buffer, "👍");
    assert_eq!(y, row_of(&buffer, "react to me") + 1);
    assert_eq!(
        buffer[(78, y)].bg,
        colour(Palette::default().surface_active()),
        "{}",
        text(&buffer)
    );
}

/// WCAG 2.1 relative luminance: the same formula as `nuncio_client`'s `theme::roles`, private
/// there, so the test repeats it instead of widening the public API for a test.
fn relative_luminance(rgb: nuncio_client::Rgb) -> f64 {
    fn channel(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

fn contrast_ratio(a: nuncio_client::Rgb, b: nuncio_client::Rgb) -> f64 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    let (light, dark) = if a >= b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

#[test]
fn meta_is_the_muted_role_and_no_text_is_dimmed() {
    let palette = Palette::default();
    // The role keeps its own promise on both surfaces the TUI paints (R3). WCAG AA is 4.5:1.
    const AA: f64 = 4.5;
    assert!(contrast_ratio(palette.text_muted(), palette.surface()) >= AA);
    assert!(contrast_ratio(palette.text_muted(), palette.surface_chrome()) >= AA);
    let muted = colour(palette.text_muted());
    // `Modifier::DIM` is emulator-defined, so nothing may depend on it (R3).
    for (width, height) in SIZES {
        for (name, buffer) in screens(width, height) {
            for y in 0..height {
                for x in 0..width {
                    let cell = &buffer[(x, y)];
                    assert!(
                        !cell.modifier.contains(Modifier::DIM),
                        "{name} at {width}x{height}: {:?} at ({x}, {y}) is dimmed:\n{}",
                        cell.symbol(),
                        text(&buffer)
                    );
                }
            }
        }
    }
    // Meta is that role: a message's time, the day label, a footer key and an empty state.
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "17:01");
    assert_eq!(buffer[(x, y)].fg, muted, "a message's time");
    let (x, y) = find(&buffer, "SAT 17 OCT 2026");
    assert_eq!(buffer[(x, y)].fg, muted, "the day label");
    let (x, y) = find(&buffer, "? all keys");
    assert_eq!(buffer[(x, y)].fg, muted, "a footer key");
    let mut app = app_with(Vec::new());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "No chats yet");
    assert_eq!(buffer[(x, y)].fg, muted, "an empty state");
}
