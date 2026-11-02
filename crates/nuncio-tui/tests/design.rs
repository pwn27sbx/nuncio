//! The terminal's half of the redesign (design plan §9): one line between panes and no boxes,
//! the palette roles for the tones, the accent only for the unread dot, the selection bar and the
//! one primary hint, one line per message with day labels, a one-line footer, a scrim behind the
//! overlays, and one-line empty states. Checked on `TestBackend` buffers at 80x24 and 240x60.

use nuncio_client::wire::MessageState;
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

/// The day the design tests call today, so `Today`/`Yesterday` never depend on the machine's
/// clock: the conversation fixture is October 2026's.
fn today() -> time::Date {
    time::Date::from_calendar_date(2026, time::Month::October, 25).expect("a date")
}

fn app_with(chats: Vec<Chat>) -> App {
    let unread = chats.iter().map(|chat| chat.unread).sum();
    let snapshot = Snapshot {
        daemon: "nunciod/0.2.1".to_owned(),
        linked: true,
        session: Session::Linked,
        unread,
        chats,
        stop_reason: None,
    };
    App::new(snapshot, Source::Daemon, Palette::default()).with_today(today())
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
fn the_accent_marks_only_the_unread_pill_and_the_one_primary_hint() {
    let palette = Palette::default();
    let accent = colour(palette.accent());
    let on_accent = colour(palette.accent_on());
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
                    // The composed rule's bar marks where the keys are (`▌`, accent text).
                    if cell.symbol() == "▌" && cell.bg != accent {
                        continue;
                    }
                    // The unread count is a pill: the accent as its background, `accent_on` for
                    // its text, never the accent as text on the row.
                    assert!(
                        cell.bg == accent && cell.fg == on_accent,
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
    // And the pill is drawn: two unread messages, one pill, on the second row of the chat.
    let buffer = draw(&mut two_chats(), 80, 24);
    // Ada is the first chat: her name, her preview (where the pill is), her rule, then Grace.
    let (_, grace) = find(&buffer, "Grace Hopper");
    let preview = grace - 2;
    let pill: String = (0..separator(&buffer))
        .filter(|&x| buffer[(x, preview)].bg == accent)
        .map(|x| buffer[(x, preview)].symbol())
        .collect();
    assert_eq!(pill, " 2 ", "the unread pill:\n{}", text(&buffer));
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
    // Ada is the first chat, on the three rows above Grace (a chat takes name, preview and rule;
    // Grace's name is in the header rule too).
    let (_, grace) = find(&buffer, "Grace Hopper");
    let ada = grace - 3;
    assert!(row(&buffer, ada).contains("Ada Lovelace"));
    let active = colour(palette.surface_active());
    let accent = colour(palette.accent());
    for x in 1..sep {
        assert_eq!(buffer[(x, ada)].bg, active, "x {x}");
        // The unread pill keeps its own background inside the fill.
        let preview = buffer[(x, ada + 1)].bg;
        assert!(preview == active || preview == accent, "x {x}");
        assert_eq!(buffer[(x, ada + 2)].bg, active);
        assert_eq!(
            buffer[(x, grace)].bg,
            colour(palette.surface_chrome()),
            "x {x}"
        );
    }
    // The rule between chats is the border role, inside the highlight too.
    assert_eq!(buffer[(2, ada + 2)].symbol(), "─");
    assert_eq!(buffer[(2, ada + 2)].fg, colour(palette.border()));
    // The transcript is on `surface`, an incoming bubble on its neutral tone, the line between
    // the panes in `border`, and the free last column still `surface`.
    let (x, y) = find(&buffer, "good morning");
    assert_eq!(
        buffer[(x, y)].bg,
        colour(palette.surface_chrome()),
        "an incoming bubble"
    );
    assert_eq!(
        buffer[(width_of(&buffer) - 2, y)].bg,
        colour(palette.surface())
    );
    assert_eq!(buffer[(sep, y)].fg, colour(palette.border()));
}

fn width_of(buffer: &Buffer) -> u16 {
    buffer.area.width
}

/// U2: each message is a bubble with its text and, inside it, its time in the faint role. No
/// author header is drawn: the side a bubble sits on says whose it is.
#[test]
fn a_bubble_carries_its_text_and_its_time_inside_it() {
    let palette = Palette::default();
    let faint = colour(palette.text_faint());
    let body = colour(palette.text());
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        for (said, time) in [
            ("hello there", "17:01"),
            ("how are you", "17:02"),
            ("fine, thanks", "17:03"),
            ("good morning", "09:15"),
        ] {
            let (text_x, text_y) = find(&buffer, said);
            let (x, y) = find(&buffer, time);
            assert_eq!(text_y, y, "the time is inside the {said:?} row");
            assert!(x > text_x, "{said:?} then its time:\n{}", text(&buffer));
            assert_eq!(buffer[(text_x, text_y)].fg, body, "{said:?}");
            assert_eq!(buffer[(x, y)].fg, faint, "the time of {said:?}");
            // The time is on the bubble, not on the pane behind it.
            assert_ne!(
                buffer[(x, y)].bg,
                colour(palette.surface()),
                "the time of {said:?} is outside its bubble:\n{}",
                text(&buffer)
            );
        }
    }
}

/// U2: an incoming bubble starts at the pane's first column, an outgoing one ends at its last
/// usable column (the free last column is the scroll gutter), and neither is wider than 65 % of
/// the pane.
#[test]
fn a_bubble_sits_on_its_side_and_takes_at_most_sixty_five_percent() {
    let palette = Palette::default();
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let sep = separator(&buffer);
        // The conversation pane, its free last column and the gutter before it.
        let left = sep + 1;
        let right = width - 1;
        let usable = (right - left - 1) as f64;
        let most = (usable * 0.65).ceil() as u16;
        // Incoming: the pane's first column is on the bubble.
        let (_, incoming) = find(&buffer, "hello there");
        assert_ne!(
            buffer[(left, incoming)].bg,
            colour(palette.surface()),
            "an incoming bubble starts at the first column:\n{}",
            text(&buffer)
        );
        let run = (left..right)
            .take_while(|&x| buffer[(x, incoming)].bg != colour(palette.surface()))
            .count();
        assert!((1..=usize::from(most)).contains(&run), "incoming run {run}");
        // Outgoing: it ends at the last usable column, and never wider than the most.
        let outgoing = colour(palette.bubble_outgoing());
        let (_, mine) = find(&buffer, "fine, thanks");
        assert_eq!(
            buffer[(right - 2, mine)].bg,
            outgoing,
            "an outgoing bubble ends at the last usable column:\n{}",
            text(&buffer)
        );
        assert_eq!(buffer[(right - 1, mine)].bg, colour(palette.surface()));
        let run = (left..=right - 2)
            .rev()
            .take_while(|&x| buffer[(x, mine)].bg == outgoing)
            .count();
        assert!((1..=usize::from(most)).contains(&run), "outgoing run {run}");
    }
}

/// U2: a message longer than a bubble wraps inside it, on rows that all belong to the bubble.
#[test]
fn a_long_message_wraps_inside_its_bubble() {
    let long = "the quick brown fox jumps over the lazy dog and keeps running ".repeat(4);
    let mut app = two_chats();
    open(
        &mut app,
        ADA,
        vec![message("L1", long.trim(), false, "2026-10-17T17:01:00Z")],
    );
    let buffer = draw(&mut app, 80, 24);
    let surface = colour(Palette::default().surface());
    let sep = separator(&buffer);
    let left = sep + 1;
    let (_, first) = find(&buffer, "the quick brown");
    let mut rows = 0;
    for y in first..24 {
        if (left..79).all(|x| buffer[(x, y)].bg == surface) {
            break;
        }
        rows += 1;
    }
    assert!(rows > 1, "the long message wraps:\n{}", text(&buffer));
    assert!(
        text(&buffer).contains("keeps running"),
        "the last words are still drawn:\n{}",
        text(&buffer)
    );
    // The bubble never takes more than 65 % of the pane (46 columns wide: at most 29).
    let run = (left..79)
        .take_while(|&x| buffer[(x, first)].bg != surface)
        .count();
    assert!(
        run <= 29,
        "a 46-column pane gives the bubble at most 29: {run}"
    );
    assert_eq!(run, 29, "the long message takes the whole share");
}

/// U2: no author header is repeated over the messages: the conversation never says `you`, `them`
/// or the chat's name again.
#[test]
fn the_author_is_never_repeated_over_the_bubbles() {
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let sep = separator(&buffer);
        let conversation = (3..height - 1)
            .map(|y| {
                (sep + 1..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for gone in ["you ·", "them ·", "Ada Lovelace ·", "Ada Lovelace"] {
            assert!(
                !conversation.contains(gone),
                "{gone:?} over a bubble:\n{conversation}"
            );
        }
    }
}

/// U2, correction §8.2: the outgoing state the protocol does have is drawn where the ticks would
/// go — a small clock while a send is queued, a `danger` `!` when it was not sent.
#[test]
fn a_queued_bubble_shows_a_clock_and_a_not_sent_one_a_bang() {
    let palette = Palette::default();
    let danger = colour(palette.danger());
    let mut app = two_chats();
    let mut queued = message("M1", "on my way", true, "2026-10-17T17:01:00Z");
    queued.state = Some(MessageState::Queued);
    let mut lost = message("M2", "did you get this", true, "2026-10-17T17:02:00Z");
    lost.state = Some(MessageState::NotSent);
    open(&mut app, ADA, vec![queued, lost]);
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "◷");
    assert_eq!(buffer[(x, y)].fg, colour(palette.text_faint()));
    // The queued bubble says it in words too, so the clock is never the only cue.
    assert_eq!(row_of(&buffer, "queued"), y, "{}", text(&buffer));
    let (x, y) = find(&buffer, "!");
    assert_eq!(buffer[(x, y)].fg, danger, "{}", text(&buffer));
    assert_eq!(row_of(&buffer, "not sent"), y, "{}", text(&buffer));
}

#[test]
fn one_authors_run_has_no_gap_and_the_next_author_starts_after_a_blank_row() {
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
            "one author's run has no gap:\n{}",
            text(&buffer)
        );
        assert_eq!(mine, second + 2, "the next author after one blank row");
        assert!(blank(second + 1), "{}", text(&buffer));
    }
}

#[test]
fn a_day_separator_is_centred_and_says_today_and_yesterday() {
    let muted = colour(Palette::default().text_muted());
    let mut app = app_with(vec![chat(ADA, "Ada Lovelace", 0)]).with_today(
        time::Date::from_calendar_date(2026, time::Month::October, 18).expect("a date"),
    );
    open(
        &mut app,
        ADA,
        vec![
            message("A1", "yesterday's news", false, "2026-10-17T17:01:00Z"),
            message("A2", "today's news", false, "2026-10-18T09:15:00Z"),
        ],
    );
    let buffer = draw(&mut app, 80, 24);
    let sep = separator(&buffer);
    for (label, said) in [("Yesterday", "yesterday's news"), ("Today", "today's news")] {
        let (x, y) = find(&buffer, label);
        assert_eq!(
            row_of(&buffer, said),
            y + 1,
            "the separator is above its day"
        );
        assert_eq!(buffer[(x, y)].fg, muted);
        // Centred: the columns left and right of it are the same, within one.
        let left = usize::from(x) - usize::from(sep + 1);
        let right = (80 - 1) - (usize::from(x) + label.chars().count());
        assert!(
            left.abs_diff(right) <= 1,
            "{label} sits {left} from the left and {right} from the right:\n{}",
            text(&buffer)
        );
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
    // Two members of one group, and one of mine: the side says whose, never a colour.
    let ink = |needle: &str| {
        let (x, y) = find(&buffer, needle);
        (buffer[(x, y)].fg, buffer[(x, y)].modifier)
    };
    assert_eq!(ink("first"), ink("second"), "{}", text(&buffer));
    assert_eq!(ink("first"), ink("third"), "{}", text(&buffer));
    // Both members' bubbles share the one incoming tone.
    let incoming = colour(Palette::default().surface_chrome());
    for who in ["first", "second"] {
        let (x, y) = find(&buffer, who);
        assert_eq!(buffer[(x, y)].bg, incoming, "no sender gets its own tone");
    }
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

/// An app that has no device linked at all (S10).
fn app_unlinked() -> App {
    App::new(
        Snapshot {
            daemon: "nunciod/0.2.1".to_owned(),
            linked: false,
            session: Session::Unlinked,
            unread: 0,
            chats: Vec::new(),
            stop_reason: None,
        },
        Source::Daemon,
        Palette::default(),
    )
}

// S10: an unlinked app says what is missing and points at nuncio, never at the command line.
#[test]
fn an_unlinked_app_says_no_device_is_linked_and_points_at_nuncio() {
    let muted = colour(Palette::default().text_muted());
    let mut app = app_unlinked();
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        // The list pane wraps the sentence in a narrow window: read that pane's own lines.
        let list = (0..height)
            .map(|y| {
                let line = row(&buffer, y);
                match line.split_once('│') {
                    Some((left, _)) => left.to_owned(),
                    None => line,
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            list.contains("No device linked · open nuncio to link it"),
            "an unlinked app must say what is missing: {list}"
        );
        let (x, y) = find(&buffer, "No device linked");
        assert_eq!(buffer[(x, y)].fg, muted);
        for forbidden in ["nuncio link", "systemctl", "--qr", "--consent"] {
            assert!(
                !text(&buffer).contains(forbidden),
                "an unlinked app must not name {forbidden}: {}",
                text(&buffer)
            );
        }
    }
}

#[test]
fn empty_states_are_one_muted_line_with_the_key_hint() {
    let muted = colour(Palette::default().text_muted());
    // No chats on a linked device: the list and the conversation say so, in muted text, and
    // nothing points at the command line any more (T8).
    let mut app = app_with(Vec::new());
    for (width, height) in SIZES {
        let buffer = draw(&mut app, width, height);
        let (x, y) = find(&buffer, "No chats yet");
        assert!(
            !row(&buffer, y).contains("nuncio link"),
            "{}",
            text(&buffer)
        );
        assert_eq!(buffer[(x, y)].fg, muted);
        let (x, y) = find(&buffer, "No chat open");
        assert_eq!(buffer[(x, y)].fg, muted);
    }
    // A chat with no messages.
    let mut app = two_chats();
    open(&mut app, ADA, Vec::new());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "No messages yet · i writes the first one");
    assert_eq!(buffer[(x, y)].fg, muted);
}

#[test]
fn a_reply_shows_its_quote_behind_a_coloured_bar_inside_the_bubble() {
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
        let quote = row_of(&buffer, "▌ the question");
        let one = row_of(&buffer, "line one");
        let two = row_of(&buffer, "line two");
        let next = row_of(&buffer, "afterwards");
        // The bar is the accent role and the quote is muted; the old `> ` is gone.
        let sep = separator(&buffer);
        let bar = (sep + 1..width)
            .find(|&x| buffer[(x, quote)].symbol() == "▌")
            .expect("the quote's bar");
        assert_eq!(buffer[(bar, quote)].fg, colour(palette.accent()));
        assert_eq!(buffer[(bar + 2, quote)].symbol(), "t");
        assert_eq!(buffer[(bar + 2, quote)].fg, colour(palette.text_muted()));
        assert!(
            !text(&buffer).contains("> the question"),
            "{}",
            text(&buffer)
        );
        assert_eq!(
            (one, two),
            (quote + 1, quote + 2),
            "quote, then the text:\n{}",
            text(&buffer)
        );
        assert_eq!(next, two + 1, "the same author's next message, no gap");
        assert!(!row(&buffer, quote).contains("line one"));
        // The quote is part of the selected reply, not of the next message.
        let active = colour(palette.surface_active());
        for y in [quote, one, two] {
            assert_eq!(buffer[(width - 2, y)].bg, active, "row {y}");
        }
        assert_ne!(buffer[(width - 2, next)].bg, active);
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
    // Meta is that role: the day label, a footer key and an empty state; a bubble's time is the
    // faint role (U2), which is a decoration inside the bubble, never body text.
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "17:01");
    assert_eq!(
        buffer[(x, y)].fg,
        colour(palette.text_faint()),
        "a message's time"
    );
    let (x, y) = find(&buffer, "SAT 17 OCT 2026");
    assert_eq!(buffer[(x, y)].fg, muted, "the day label");
    let (x, y) = find(&buffer, "? all keys");
    assert_eq!(buffer[(x, y)].fg, muted, "a footer key");
    let mut app = app_with(Vec::new());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "No chats yet");
    assert_eq!(buffer[(x, y)].fg, muted, "an empty state");
}

/// U5a: a chat with unread messages has its name and time in bold and its count in an accent
/// pill; the muted marker is the bell with a slash.
#[test]
fn an_unread_chat_is_bold_with_a_pill_and_a_muted_one_wears_the_bell() {
    let palette = Palette::default();
    let accent = colour(palette.accent());
    let on_accent = colour(palette.accent_on());
    let mut muted = chat(GRACE, "Grace Hopper", 0);
    muted.muted = true;
    muted.last_message = Some(message("G1", "bye", false, "2026-10-25T16:02:00Z"));
    let mut busy = chat(ADA, "Ada Lovelace", 4);
    busy.last_message = Some(message("A1", "hello", false, "2026-10-25T17:55:00Z"));
    let mut app = app_with(vec![busy, muted]);
    let buffer = draw(&mut app, 80, 24);
    // Ada is the first chat: name, preview (with the pill), rule.
    let (_, grace) = find(&buffer, "Grace Hopper");
    let name = grace - 3;
    let x = (0..separator(&buffer))
        .find(|&x| buffer[(x, name)].symbol() == "A")
        .expect("her name");
    assert!(
        buffer[(x, name)].modifier.contains(Modifier::BOLD),
        "her name:\n{}",
        text(&buffer)
    );
    let (x, y) = find(&buffer, "17:55");
    assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD), "her time");
    let (x, y) = find(&buffer, "16:02");
    assert!(!buffer[(x, y)].modifier.contains(Modifier::BOLD));
    // The pill: ` 4 ` on the accent, its text in `accent_on`.
    let preview = name + 1;
    let pill: String = (0..separator(&buffer))
        .filter(|&x| buffer[(x, preview)].bg == accent)
        .map(|x| buffer[(x, preview)].symbol())
        .collect();
    assert_eq!(pill, " 4 ", "{}", text(&buffer));
    let x = (0..separator(&buffer))
        .find(|&x| buffer[(x, preview)].bg == accent && buffer[(x, preview)].symbol() == "4")
        .expect("the count");
    assert_eq!(buffer[(x, preview)].fg, on_accent);
    assert!(!buffer[(x, preview)].modifier.contains(Modifier::BOLD));
    // The muted chat says so with a bell, not the old `⊘`.
    assert!(row(&buffer, grace).contains('🔕'), "{}", text(&buffer));
    let _ = grace;
    assert!(!text(&buffer).contains('⊘'), "{}", text(&buffer));
}

#[test]
fn more_than_ninety_nine_unread_messages_show_the_capped_pill() {
    let busy = chat(ADA, "Ada Lovelace", 120);
    let mut app = app_with(vec![busy, chat(GRACE, "Grace Hopper", 9)]);
    let buffer = draw(&mut app, 80, 24);
    let shown = text(&buffer);
    assert!(shown.contains("99+"), "the capped pill:\n{shown}");
    assert!(shown.contains(" 9 "), "a small count stays whole:\n{shown}");
}

#[test]
fn the_open_chat_is_filled_with_the_keys_or_without_them_and_the_message_only_with_them() {
    let palette = Palette::default();
    let active = colour(palette.surface_active());
    // The chat pane is 32 columns at 80: the line at 32, the conversation from 33.
    // The list's first row is the filter bar; the chats start under it.
    let list_filled = |buffer: &Buffer| (1..32).any(|x| buffer[(x, 3)].bg == active);
    let conversation_filled =
        |buffer: &Buffer| (2..22).any(|y| (33..80).any(|x| buffer[(x, y)].bg == active));

    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.pane(), Pane::Messages);
    let buffer = draw(&mut app, 80, 24);
    // The conversation has the keys: its selected message is filled, and the list still says
    // which chat is open.
    assert!(conversation_filled(&buffer), "{}", text(&buffer));
    assert!(list_filled(&buffer), "{}", text(&buffer));

    // `Tab` moves the keys to the list: the message's fill goes, the chat's stays.
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.pane(), Pane::Chats);
    let buffer = draw(&mut app, 80, 24);
    assert!(list_filled(&buffer), "{}", text(&buffer));
    assert!(!conversation_filled(&buffer), "{}", text(&buffer));
}

#[test]
fn the_transcript_opens_with_its_start_marker() {
    let muted = colour(Palette::default().text_muted());
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "Beginning of conversation");
    assert_eq!(buffer[(x, y)].fg, muted);
    assert!(
        y < row_of(&buffer, "hello there"),
        "above the first message:\n{}",
        text(&buffer)
    );
}

#[test]
fn the_scroll_gutter_says_what_is_hidden() {
    let mut app = two_chats();
    let mut many = conversation();
    for n in 0..30 {
        many.push(message(
            &format!("N{n}"),
            &format!("line {n}"),
            false,
            "2026-10-18T10:00:00Z",
        ));
    }
    open(&mut app, ADA, many);
    // It opens at the newest message, so there is more above.
    let buffer = draw(&mut app, 80, 24);
    let shown = text(&buffer);
    assert!(shown.contains("▲"), "more above:\n{shown}");
    // At the oldest, the gutter points the other way and the start marker is on screen.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('g')));
    let buffer = draw(&mut app, 80, 24);
    let shown = text(&buffer);
    assert!(shown.contains("▼"), "more below:\n{shown}");
    assert!(!shown.contains("▲"), "nothing above:\n{shown}");
    assert!(shown.contains("Beginning of conversation"), "{shown}");
}

#[test]
fn the_composer_has_a_field_with_a_placeholder_and_the_keys_are_said_once() {
    let palette = Palette::default();
    let muted = colour(palette.text_muted());
    let faint = colour(palette.text_faint());
    let accent = colour(palette.accent());
    let mut app = two_chats();
    // With nothing to send and no keys on it, the composer says how to write at all.
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "i or Enter to write");
    assert_eq!(buffer[(x, y)].fg, muted);
    assert!(!text(&buffer).contains("Write a message"));

    // Opened, the field is empty and says so with a placeholder, in the faint role, with the
    // caret where the first letter goes.
    open(&mut app, ADA, conversation());
    let buffer = draw(&mut app, 80, 24);
    let (x, y) = find(&buffer, "Write a message");
    assert_eq!(buffer[(x, y)].fg, faint);
    assert_eq!(buffer[(x - 2, y)].symbol(), "›");
    // The field is closed by a hairline under it, and the accent marks its rule while it holds
    // the keys.
    let floor = row_of(&buffer, "Write a message") + 1;
    assert_eq!(buffer[(40, floor)].symbol(), "─");
    assert_eq!(buffer[(33, y - 1)].symbol(), "▌");
    assert_eq!(buffer[(33, y - 1)].fg, accent);

    // The keys are said once: the footer, never again on the rule.
    let shown = text(&buffer);
    assert_eq!(shown.matches("Enter send").count(), 1, "once:\n{shown}");
    assert_eq!(shown.matches("Alt-Enter").count(), 1, "once:\n{shown}");
    assert!(!shown.contains("Enter sends"), "{shown}");

    // A draft replaces the placeholder, and the rule still reaches the last column.
    app.handle_key(key(KeyCode::Char('h')));
    app.handle_key(key(KeyCode::Char('i')));
    let buffer = draw(&mut app, 80, 24);
    let shown = text(&buffer);
    assert!(!shown.contains("Write a message"), "{shown}");
    let rule = row_of(&buffer, "▌");
    assert_eq!(buffer[(79, rule)].symbol(), "─", "{shown}");

    // With a reply's quote the rule carries it and still reaches the last column.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('r')));
    for width in [80, 240] {
        let buffer = draw(&mut app, width, 24);
        let rule = row_of(&buffer, "replying to:");
        assert!(
            row(&buffer, rule).contains("replying to:"),
            "{}",
            text(&buffer)
        );
        assert_eq!(
            buffer[(width - 1, rule)].symbol(),
            "─",
            "the rule reaches the last column:\n{}",
            text(&buffer)
        );
    }
}

/// U7: every element the redesign added takes its colour from the theme's roles — checked cell by
/// cell, since a literal and a role can be the same colour in one buffer.
#[test]
fn each_new_element_uses_its_exact_role() {
    let palette = Palette::default();
    let role = colour;
    let mut app = two_chats();
    let mut lost = message("M2", "did you get this", true, "2026-10-17T17:02:00Z");
    lost.state = Some(MessageState::NotSent);
    open(
        &mut app,
        ADA,
        vec![
            message("A1", "hello there", false, "2026-10-17T17:01:00Z"),
            lost,
        ],
    );
    let buffer = draw(&mut app, 80, 24);
    // An incoming bubble on the chrome, its time and the `!` where the ticks would go.
    let (x, y) = find(&buffer, "hello there");
    assert_eq!(buffer[(x, y)].bg, role(palette.surface_chrome()));
    assert_eq!(buffer[(x, y)].fg, role(palette.text()));
    let (x, y) = find(&buffer, "17:01");
    assert_eq!(buffer[(x, y)].fg, role(palette.text_faint()));
    assert_eq!(buffer[(x, y)].bg, role(palette.surface_chrome()));
    let (x, y) = find(&buffer, "did you get this");
    assert_eq!(buffer[(x, y)].bg, role(palette.bubble_outgoing()));
    let (x, y) = find(&buffer, "!");
    assert_eq!(buffer[(x, y)].fg, role(palette.danger()));
    // The unread pill: the accent as its background, `accent_on` as its text.
    let sep = separator(&buffer);
    let (_, grace) = find(&buffer, "Grace Hopper");
    let preview = grace - 2;
    let pill = (0..sep)
        .find(|&x| buffer[(x, preview)].bg == role(palette.accent()))
        .expect("the pill");
    assert_eq!(buffer[(pill + 1, preview)].fg, role(palette.accent_on()));
    // The filter chip in force: the same pair.
    let chip = (0..sep)
        .find(|&x| buffer[(x, 2)].bg == role(palette.accent()))
        .expect("the chip");
    assert_eq!(buffer[(chip + 1, 2)].fg, role(palette.accent_on()));
    // The selected chat's rows on the active surface, the rule between chats in the border role.
    assert_eq!(buffer[(1, grace - 3)].bg, role(palette.surface_active()));
    assert_eq!(buffer[(2, grace - 1)].fg, role(palette.border()));
    // The placeholder in the faint role, the prompt before it muted.
    let (x, y) = find(&buffer, "Write a message");
    assert_eq!(buffer[(x, y)].fg, role(palette.text_faint()));
    assert_eq!(buffer[(x - 2, y)].fg, role(palette.text_muted()));
    // The day separator muted.
    let (x, y) = find(&buffer, "SAT 17 OCT 2026");
    assert_eq!(buffer[(x, y)].fg, role(palette.text_muted()));
}

/// U7: the crate never names a colour itself. The one legitimate `Color::Rgb(` is the palette
/// helper's body; a literal (hex or digits) anywhere else fails, and so does losing the helper.
#[test]
fn the_only_colour_the_crate_names_is_the_palette_helper() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut pending = vec![root];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the crate's sources") {
            let path = entry.expect("a source file").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty(), "the crate has sources");
    let helper = "Color::Rgb(colour.r, colour.g, colour.b)";
    let mut helpers = 0;
    let mut offenders = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read a source file");
        for (number, line) in text.lines().enumerate() {
            let Some(at) = line.find("Color::Rgb(") else {
                continue;
            };
            let before = &line[..at];
            // A comment may name it; code may not.
            if before.trim_start().starts_with("//") {
                continue;
            }
            if line.trim() == helper {
                helpers += 1;
                continue;
            }
            offenders.push(format!(
                "{}:{}: {}",
                file.display(),
                number + 1,
                line.trim()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "a colour is named outside the palette helper: {offenders:?}"
    );
    assert_eq!(helpers, 1, "the palette helper is the one site");
}

#[test]
fn the_overlays_share_one_width_rule() {
    let chrome = colour(Palette::default().surface_chrome());
    let mut app = two_chats();
    app.handle_key(key(KeyCode::Char('?')));
    let buffer = draw(&mut app, 240, 60);
    let y = row_of(&buffer, "Keys");
    let width = (0..240).filter(|&x| buffer[(x, y)].bg == chrome).count();
    assert_eq!(width, 76, "the widest overlay:\n{}", text(&buffer));
    // On a small screen the same panel is the screen minus four columns.
    let buffer = draw(&mut app, 40, 20);
    let y = row_of(&buffer, "Keys");
    let width = (0..40).filter(|&x| buffer[(x, y)].bg == chrome).count();
    assert_eq!(width, 36, "{}", text(&buffer));
}

#[test]
fn the_reaction_picker_stacks_its_keys_when_it_is_narrow() {
    let mut app = two_chats();
    open(&mut app, ADA, conversation());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('e')));
    // Wide: one row of pairs.
    let buffer = draw(&mut app, 80, 24);
    assert_eq!(
        row_of(&buffer, "👍"),
        row_of(&buffer, "❤"),
        "one row of pairs:\n{}",
        text(&buffer)
    );
    // Narrow: a row per reaction, so the panel still fits.
    let buffer = draw(&mut app, 40, 20);
    assert_ne!(
        row_of(&buffer, "👍"),
        row_of(&buffer, "❤"),
        "one reaction per row:\n{}",
        text(&buffer)
    );
}

/// U1: the chat list takes 30 % of the body, never under `LIST` and never over `LIST_MAX`; the
/// conversation gets the rest. Below `NARROW` only the focused pane is drawn, as before.
#[test]
fn the_chat_list_takes_thirty_percent_within_its_bounds() {
    for (width, height, want) in [(240u16, 60u16, 48u16), (80, 24, 32)] {
        let mut app = app_with(vec![chat("15551230001@s.whatsapp.net", "Ada", 0)]);
        let buffer = draw(&mut app, width, height);
        let got = separator(&buffer);
        assert_eq!(
            got, want,
            "at {width}x{height} the list is {got} columns, want {want}"
        );
    }
    // 40 columns is under NARROW: one pane, so no separator at all.
    let mut app = app_with(vec![chat("15551230001@s.whatsapp.net", "Ada", 0)]);
    let buffer = draw(&mut app, 40, 20);
    assert!(
        !text(&buffer).contains('│'),
        "narrow mode draws a single pane:\n{}",
        text(&buffer)
    );
}

/// A very wide body does not overflow the list's share: 30 % is computed in u32 and clamped.
#[test]
fn a_very_wide_body_does_not_overflow_the_list_width() {
    let mut app = app_with(vec![chat("15551230001@s.whatsapp.net", "Ada", 0)]);
    let buffer = draw(&mut app, 3000, 20);
    assert_eq!(separator(&buffer), 48, "the list stays at its most width");
}
