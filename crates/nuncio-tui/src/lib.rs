//! `nuncio-tui`'s model and rendering, as a library so it can be tested against a
//! `TestBackend` buffer without a terminal or a daemon.
//!
//! [`App`] is the pure state: the shared view [`Model`] from `nuncio-client` (the chat list with
//! archived chats sectioned last, the loaded conversations, the composer, and the rules the
//! desktop app follows too), plus the focused pane, the overlays, the scroll, the pictures and
//! the Omarchy [`Palette`]. It changes
//! only through [`App::handle_key`], [`App::handle_mouse`] (mapped through the last drawn
//! [`ScreenMap`] by [`hit_test`]) and [`App::apply`] (a reply or an event frame), which return
//! an [`Action`] for the I/O loop. [`Daemon`] is that I/O: a request connection and a subscribed
//! event connection to `nunciod`. The keys are listed in [`KEYS`] and in `README.md`.

use std::path::PathBuf;

use nuncio_client::model::{Effect, Model, chat_title, is_picture, quoted};
use nuncio_client::wire::MessageState;
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Rgb, Snapshot};
use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui_image::StatefulImage;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_segmentation::UnicodeSegmentation;

mod daemon;
pub mod images;

pub use daemon::{Daemon, open_externally};
pub use nuncio_client::{Backoff, Reconnect};

/// Where the chat list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Read from the daemon over the socket.
    Daemon,
    /// The compiled-in fixture, because the daemon was unreachable. The screen says so.
    Fixture,
}

/// Which pane has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The chat list.
    Chats,
    /// The selected chat's conversation (scrolling).
    Messages,
    /// The composer under the conversation.
    Compose,
}

/// What the I/O loop must do after a key or a frame. The [`App`] itself never does I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Leave the program.
    Quit,
    /// Send this frame on the request connection and [`App::apply`] the reply.
    Request(ClientFrame),
    /// Open this downloaded media file with the desktop's opener (it was asked for with `o`).
    Open(PathBuf),
}

/// Where things were on the last drawn screen, so a mouse event can be mapped to what is under
/// it ([`hit_test`]). Built by [`App::draw`]; empty before the first draw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenMap {
    /// The chat pane, borders included.
    pub chats: Rect,
    /// The rows of the chat list inside its border; each chat takes [`CHAT_ROWS`] rows.
    pub chat_rows: Rect,
    /// The list's scroll: the index of the chat on its first row.
    pub chat_offset: usize,
    /// How many chats are listed.
    pub chat_count: usize,
    /// The conversation pane, borders included (the composer is inside it).
    pub conversation: Rect,
    /// The rows the conversation's messages are drawn in.
    pub message_area: Rect,
    /// For each row of `message_area`, the index of the loaded message drawn there.
    pub message_rows: Vec<Option<usize>>,
    /// The composer's rows (the rule above them included).
    pub composer: Rect,
}

/// What is under a screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The chat pane: the chat at this list index, or none (a border, below the last chat).
    Chats(Option<usize>),
    /// The conversation pane: the loaded message at this index, or none.
    Conversation(Option<usize>),
    /// The composer.
    Composer,
}

/// Rows one chat takes in the list: its name, then its last message.
pub const CHAT_ROWS: u16 = 2;

/// What is drawn at `(column, row)` on the screen `map` describes, if anything the mouse acts on.
pub fn hit_test(map: &ScreenMap, column: u16, row: u16) -> Option<Target> {
    let inside = |area: Rect| area.contains(ratatui::layout::Position::new(column, row));
    if inside(map.chats) {
        let index = inside(map.chat_rows)
            .then(|| map.chat_offset + usize::from((row - map.chat_rows.y) / CHAT_ROWS))
            .filter(|&index| index < map.chat_count);
        return Some(Target::Chats(index));
    }
    if inside(map.composer) {
        return Some(Target::Composer);
    }
    if inside(map.conversation) {
        let message = inside(map.message_area)
            .then(|| map.message_rows.get(usize::from(row - map.message_area.y)))
            .flatten()
            .copied()
            .flatten();
        return Some(Target::Conversation(message));
    }
    None
}

/// How many lines one wheel step scrolls the conversation.
const WHEEL: usize = 3;

/// Marks a pinned chat.
const PINNED: char = '★';
/// Marks a muted chat.
const MUTED: char = '⊘';
/// Marks an archived chat.
const ARCHIVED: char = '▤';

/// How many lines PageUp/PageDown scroll.
const PAGE: usize = 10;
/// The reactions the picker offers (the shared set), on keys `1`..`6`; `x` removes mine.
pub use nuncio_client::model::REACTIONS;
/// How many characters of a quoted message are shown.
const SNIPPET: usize = 40;
/// How many rows the composer grows to; a longer draft scrolls with the cursor.
const COMPOSER_ROWS: usize = 5;

/// Every key, grouped by where it works; drawn by the `?` help screen and documented in the
/// crate README.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Chats",
        &[
            ("j/k ↓/↑", "move down / up"),
            (
                "Enter",
                "open the chat and write (marks it read on WhatsApp)",
            ),
            ("l →", "open it to read"),
            ("Tab", "focus the conversation"),
            ("/", "search the selected chat"),
            ("q Esc", "quit"),
        ],
    ),
    (
        "Conversation",
        &[
            ("j/k ↓/↑", "select a newer / older message"),
            ("r", "reply to it"),
            ("e", "react to it"),
            ("R", "retry a not sent or not confirmed send"),
            ("o", "show the picture in full, or open other media"),
            ("O", "open its media externally"),
            ("Enter i", "write"),
            ("PgUp/PgDn", "scroll a page"),
            ("g/G Home/End", "oldest / newest"),
            ("Esc h ← Tab", "back to the chats"),
        ],
    ),
    (
        "Composer",
        &[
            ("Enter", "send"),
            ("Alt-Enter C-j", "new line"),
            ("←/→ Home/End", "move the cursor"),
            ("Bksp/Delete", "erase"),
            ("Esc", "back to the conversation"),
        ],
    ),
    (
        "Search",
        &[
            ("type, Enter", "search"),
            ("j/k", "move"),
            ("Enter", "select"),
            ("/", "edit"),
            ("Esc", "close"),
        ],
    ),
    (
        "Reactions",
        &[
            ("1-6", "👍 ❤️ 😂 😮 😢 🙏"),
            ("x", "remove mine"),
            ("Esc", "cancel"),
        ],
    ),
    (
        "Everywhere",
        &[
            ("?", "this help"),
            ("Ctrl-C", "quit"),
            ("click", "a chat opens it, a message selects it"),
            ("wheel", "scrolls the pane under the pointer"),
            ("Shift+drag", "selects text"),
        ],
    ),
];

/// The chat list from `proto/fixtures/chats_response.json`.
///
/// Compiled in, so the client always renders when no daemon is reachable — which is what
/// makes S9 checkable with no daemon and no linked device. The fixture is shared with the
/// Go daemon and checked by `nuncio-client`'s contract tests.
pub fn fixture_snapshot() -> Snapshot {
    Snapshot::from_chats_json(include_str!("../../../proto/fixtures/chats_response.json"))
        .expect("the embedded chats fixture matches the wire types")
}

/// The terminal client: the shared view [`Model`] (chat list, transcript, composer and their
/// rules) plus what only the terminal has: the focused pane, the overlays, the scroll, the
/// pictures and the [`Palette`]. It changes only through [`App::handle_key`],
/// [`App::handle_mouse`] and [`App::apply`], so it is testable without a terminal or a daemon.
pub struct App {
    model: Model,
    source: Source,
    palette: Palette,
    pane: Pane,
    list: ListState,
    image: Option<StatefulProtocol>,
    /// Lines scrolled up from the newest message; clamped when drawn.
    scroll: usize,
    help: bool,
    /// Scroll the selected message into view on the next draw.
    reveal: bool,
    /// The reaction picker is open on the selected message.
    picking: bool,
    /// The search results' list scroll (the results and the chosen one are the model's).
    search_list: ListState,
    /// The inline pictures, when the terminal draws them.
    images: Option<images::Images>,
    utc_offset: time::UtcOffset,
    /// Where things were on the last drawn screen, for the mouse.
    map: ScreenMap,
}

impl From<Effect> for Action {
    fn from(effect: Effect) -> Self {
        match effect {
            Effect::Request(frame) => Action::Request(frame),
            Effect::Open(path) => Action::Open(path),
        }
    }
}

impl App {
    /// The chat list is shown with archived chats last (kept in the daemon's order otherwise).
    pub fn new(snapshot: Snapshot, source: Source, palette: Palette) -> Self {
        let mut app = Self {
            model: Model::new(snapshot, "tui"),
            source,
            palette,
            pane: Pane::Chats,
            list: ListState::default(),
            image: None,
            scroll: 0,
            help: false,
            reveal: false,
            picking: false,
            search_list: ListState::default(),
            images: None,
            utc_offset: time::UtcOffset::UTC,
            map: ScreenMap::default(),
        };
        app.sync();
        app
    }

    /// Attach an inline image (the `--image` debug flag) to show beside the chat list.
    pub fn with_image(mut self, image: StatefulProtocol) -> Self {
        self.image = Some(image);
        self
    }

    /// Draw pictures in the transcript with this protocol: an image or sticker message that states
    /// its size gets a box, its thumbnail is drawn in it, and `o` fetches the full picture into the
    /// same box. Without it (the `off` choice, or tests of everything else) messages show their
    /// `[image]` label only and `o` opens media externally.
    pub fn with_graphics(mut self, graphics: images::Graphics) -> Self {
        self.images = Some(images::Images::new(&graphics));
        self
    }

    /// Wait until every picture asked for by the last draw is encoded (the workers run off the UI
    /// thread), or `timeout`; `true` when none is left. The loop never needs it: it redraws every
    /// tick and draws what is ready.
    pub fn wait_for_images(&mut self, timeout: std::time::Duration) -> bool {
        let Some(images) = self.images.as_mut() else {
            return true;
        };
        let (settled, failures) = images.wait(timeout);
        self.picture_failures(failures);
        settled
    }

    /// How many pictures were encoded so far (a cache hit encodes none).
    pub fn image_encodes(&self) -> u64 {
        self.images.as_ref().map_or(0, images::Images::encodes)
    }

    /// How many encodes the workers ran, kept or dropped (a job for an old width is skipped).
    pub fn image_jobs_run(&self) -> u64 {
        self.images.as_ref().map_or(0, images::Images::jobs_run)
    }

    /// How many encoded pictures the cache holds.
    pub fn image_cache_len(&self) -> usize {
        self.images.as_ref().map_or(0, images::Images::len)
    }

    /// A full picture that cannot be shown says why in the footer; its thumbnail stays.
    fn picture_failures(&mut self, failures: Vec<String>) {
        if let Some(why) = failures.into_iter().last() {
            self.model
                .set_notice(format!("cannot show the picture: {why}"));
        }
    }

    /// The prefix of the `client_id`s this app chooses: `<prefix>-1`, `<prefix>-2`, ...
    /// It must differ between runs, so a new send never repeats an old `client_id`.
    pub fn with_client_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.model.set_client_prefix(prefix);
        self
    }

    /// Show message times at this offset from UTC: `main` passes the local one, read once at
    /// startup (so a message from the other side of a daylight-saving change is an hour off
    /// until the next start). Without it, times are UTC.
    pub fn with_utc_offset(mut self, offset: time::UtcOffset) -> Self {
        self.utc_offset = offset;
        self
    }

    /// Follow a theme switch (the shared `ThemeWatcher`).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// A one-line notice in the footer (an error from the daemon, a lost connection).
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.model.set_notice(notice);
    }

    /// The shared view model this terminal client draws.
    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn chats(&self) -> &[Chat] {
        self.model.chat_list().chats()
    }

    pub fn selected(&self) -> usize {
        self.model.chat_list().selected()
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// Whether the driver is connected to the daemon (`false` while it reconnects).
    pub fn connected(&self) -> bool {
        self.model.connected()
    }

    /// The driver lost (`false`) or regained (`true`) its daemon connection. While it is lost
    /// the header says `reconnecting…` instead of the session.
    pub fn set_connected(&mut self, connected: bool) {
        self.model.set_connected(connected);
    }

    /// What to ask the daemon again after a reconnect, since events were missed: the status,
    /// the chat list and the selected chat's messages. The draft, the selected chat, the open
    /// pane and the selected message are kept.
    pub fn resync(&mut self) -> Vec<ClientFrame> {
        self.model.resync()
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The text being written.
    pub fn composer(&self) -> &str {
        self.model.composer().text()
    }

    /// The JID of the selected chat, if any.
    pub fn selected_jid(&self) -> Option<&str> {
        self.model.selected_jid()
    }

    /// Select the next chat, stopping at the last one.
    pub fn select_next(&mut self) {
        self.model.select_next_chat();
        self.sync();
    }

    /// Select the previous chat, stopping at the first one.
    pub fn select_previous(&mut self) {
        self.model.select_previous_chat();
        self.sync();
    }

    pub fn toggle_pane(&mut self) {
        self.set_pane(match self.pane {
            Pane::Chats => Pane::Messages,
            Pane::Messages | Pane::Compose => Pane::Chats,
        });
    }

    /// Focus `pane`; outside the chat list the selected chat's conversation is open.
    fn set_pane(&mut self, pane: Pane) {
        self.pane = pane;
        self.model.set_open(pane != Pane::Chats);
    }

    /// The model moved the view to another chat or made a send: the conversation scrolls back to
    /// the newest message.
    fn sync(&mut self) {
        if self.model.take_view_reset() {
            self.scroll = 0;
        }
    }

    /// The daemon answered a request about the phone-number chat `alias` with its own reply naming
    /// the chat `canonical` (ADR 0001); see [`Model::adopt`]. The driver calls it through
    /// [`App::apply_reply`], which pairs a request with its own reply.
    pub fn adopt(&mut self, alias: &str, canonical: &str) {
        self.model.adopt(alias, canonical);
        self.sync();
    }

    /// Fold the daemon's `reply` to `request` into the model, as [`App::apply`] does; see
    /// [`Model::apply_reply`]: a reply is paired with the request it answers, never guessed.
    pub fn apply_reply(&mut self, request: &ClientFrame, reply: &DaemonFrame) -> Option<Action> {
        let effect = self.model.apply_reply(request, reply);
        self.applied(reply, effect)
    }

    /// Fold one daemon frame (a reply or an event) into the model. An `event_focus` opens the
    /// focused chat (closing the help) and asks for its messages; the messages of a chat just
    /// opened with unread messages, or a new message in the open chat, ask for a `mark_read` (one
    /// in flight per chat).
    pub fn apply(&mut self, frame: &DaemonFrame) -> Option<Action> {
        let effect = self.model.apply(frame);
        self.applied(frame, effect)
    }

    fn applied(&mut self, frame: &DaemonFrame, effect: Option<Effect>) -> Option<Action> {
        self.sync();
        if matches!(frame, DaemonFrame::EventFocus { .. }) && effect.is_some() {
            self.help = false;
            self.set_pane(Pane::Messages);
        }
        effect.map(Action::from)
    }

    /// The daemon refused `frame` with an `error`: show it in the footer and end what the
    /// request left waiting (a download's pending open, a search's "searching…").
    pub fn request_failed(&mut self, frame: &ClientFrame, message: impl Into<String>) {
        self.model.request_failed(frame, message);
    }

    /// The daemon answered my send (or retry) `client_id` with `not_confirmed` (WhatsApp's
    /// acknowledgement timed out), or its reply did not come within the read bound after it was
    /// written in full: it may have been delivered. It is shown as not confirmed, not
    /// as not sent, and `R` still retries it: a retry reuses its WhatsApp id, so it can never
    /// become a second message.
    pub fn send_not_confirmed(&mut self, client_id: &str) {
        self.model.send_not_confirmed(client_id);
    }

    /// Enter on a chat (or a click on it): open it with the composer focused, ready to type.
    fn open_to_write(&mut self) -> Option<Action> {
        let action = self.open_selected()?;
        self.model.composer_mut().cancel_reply();
        self.set_pane(Pane::Compose);
        Some(action)
    }

    /// Open the selected chat: show its conversation and ask the daemon for its messages.
    fn open_selected(&mut self) -> Option<Action> {
        let frame = self.model.open_selected()?;
        self.set_pane(Pane::Messages);
        Some(Action::Request(frame))
    }

    /// `o` (`external` false) and `O` (true): ask the daemon for the selected media message's
    /// file. With pictures drawn, `o` on an image or sticker shows the full picture in its box;
    /// everything else (and `O`) opens the file externally once it is downloaded.
    fn download_selected(&mut self, external: bool) -> Option<Action> {
        let download = self
            .model
            .download_selected(self.images.is_some(), external)?;
        if download.inline
            && let Some(images) = self.images.as_mut()
        {
            images.retry_full(&download.jid, &download.id);
        }
        Some(Action::Request(download.frame()))
    }

    fn picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let emoji = match key.code {
            KeyCode::Esc => {
                self.picking = false;
                return None;
            }
            KeyCode::Char('x') => "",
            KeyCode::Char(c @ '1'..='6') => REACTIONS[usize::from(c as u8 - b'1')],
            _ => return None,
        };
        self.picking = false;
        self.model.react_selected(emoji).map(Action::Request)
    }

    fn search_key(&mut self, key: KeyEvent) -> Option<Action> {
        self.model.selected_jid()?;
        let editing = self.model.transcript().search()?.is_editing();
        match key.code {
            KeyCode::Esc => self.model.close_search(),
            KeyCode::Backspace if editing => self.model.search_backspace(),
            KeyCode::Enter if editing => return self.model.submit_search().map(Action::Request),
            KeyCode::Char(c) if editing => self.model.search_type(c),
            KeyCode::Char('/') => self.model.search_edit(),
            KeyCode::Char('j') | KeyCode::Down => self.model.select_next_result(),
            KeyCode::Char('k') | KeyCode::Up => self.model.select_previous_result(),
            // Close the search and select the chosen result, if it is loaded.
            KeyCode::Enter if self.model.jump_to_result() => {
                self.reveal = true;
                self.set_pane(Pane::Messages);
            }
            _ => {}
        }
        None
    }

    /// One key press. Returns what the I/O loop must do, if anything.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let action = self.key(key);
        self.sync();
        action
    }

    fn key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if self.pane == Pane::Compose {
            return self.compose_key(key);
        }
        if self.model.transcript().search().is_some() {
            return self.search_key(key);
        }
        if self.picking {
            return self.picker_key(key);
        }
        if self.help {
            if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
                self.help = false;
            }
            return None;
        }
        let chosen = self.model.selected_chat().is_some();
        match (self.pane, key.code) {
            (_, KeyCode::Char('?')) => self.help = true,
            (_, KeyCode::Char('q')) | (Pane::Chats, KeyCode::Esc) => return Some(Action::Quit),
            (_, KeyCode::Tab) => self.toggle_pane(),
            (_, KeyCode::Char('i')) if chosen => {
                self.model.composer_mut().cancel_reply();
                self.set_pane(Pane::Compose);
            }
            (_, KeyCode::Char('/')) if chosen => {
                self.model.open_search();
                self.search_list = ListState::default();
            }
            (Pane::Chats, KeyCode::Char('j') | KeyCode::Down) => self.select_next(),
            (Pane::Chats, KeyCode::Char('k') | KeyCode::Up) => self.select_previous(),
            (Pane::Chats, KeyCode::Enter) => return self.open_to_write(),
            (Pane::Chats, KeyCode::Char('l') | KeyCode::Right) => return self.open_selected(),
            (Pane::Messages, KeyCode::Char('k') | KeyCode::Up) => {
                self.reveal |= self.model.select_older();
            }
            (Pane::Messages, KeyCode::Char('j') | KeyCode::Down) => {
                self.reveal |= self.model.select_newer();
            }
            (Pane::Messages, KeyCode::Char('r')) => {
                if self.model.reply_to_selected() {
                    self.set_pane(Pane::Compose);
                }
            }
            (Pane::Messages, KeyCode::Char('e')) => {
                self.picking = self.model.target_id("reacted to").is_some();
            }
            (Pane::Messages, KeyCode::Char('R')) => {
                return self.model.retry_selected().map(Action::Request);
            }
            (Pane::Messages, KeyCode::Char('o')) => return self.download_selected(false),
            (Pane::Messages, KeyCode::Char('O')) => return self.download_selected(true),
            (Pane::Messages, KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left) => {
                self.set_pane(Pane::Chats);
            }
            (Pane::Messages, KeyCode::Enter) => self.set_pane(Pane::Compose),
            (_, KeyCode::PageUp) => self.scroll_up(PAGE),
            (_, KeyCode::PageDown) => self.scroll_down(PAGE),
            (Pane::Messages, KeyCode::Char('g') | KeyCode::Home) => self.scroll = usize::MAX,
            (Pane::Messages, KeyCode::Char('G') | KeyCode::End) => self.scroll = 0,
            _ => {}
        }
        None
    }

    /// One mouse event, mapped through the last drawn screen ([`hit_test`]). A left click on a
    /// chat opens it with the composer focused (as Enter does); in the conversation it focuses
    /// the conversation and selects the clicked message; in the composer it focuses the composer.
    /// The wheel scrolls the pane under the pointer: the conversation by lines, the chat list by
    /// chats. Nothing happens while the help, the reaction picker or the search is open.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<Action> {
        let action = self.mouse(mouse);
        self.sync();
        action
    }

    fn mouse(&mut self, mouse: MouseEvent) -> Option<Action> {
        if self.help || self.picking || self.model.transcript().search().is_some() {
            return None;
        }
        let target = hit_test(&self.map, mouse.column, mouse.row)?;
        let chosen = self.model.selected_chat().is_some();
        match (mouse.kind, target) {
            (MouseEventKind::Down(MouseButton::Left), Target::Chats(Some(index))) => {
                self.model.select_chat(index);
                return self.open_to_write();
            }
            (MouseEventKind::Down(MouseButton::Left), Target::Conversation(message)) => {
                if chosen {
                    if self.pane == Pane::Compose {
                        self.model.composer_mut().cancel_reply();
                    }
                    self.set_pane(Pane::Messages);
                    if let Some(index) = message {
                        self.model.select_message(index);
                    }
                }
            }
            (MouseEventKind::Down(MouseButton::Left), Target::Composer)
                if chosen && self.pane != Pane::Compose =>
            {
                self.model.composer_mut().cancel_reply();
                self.set_pane(Pane::Compose);
            }
            (MouseEventKind::ScrollUp, Target::Chats(_)) => self.select_previous(),
            (MouseEventKind::ScrollDown, Target::Chats(_)) => self.select_next(),
            (MouseEventKind::ScrollUp, Target::Conversation(_) | Target::Composer) => {
                self.scroll_up(WHEEL);
            }
            (MouseEventKind::ScrollDown, Target::Conversation(_) | Target::Composer) => {
                self.scroll_down(WHEEL);
            }
            _ => {}
        }
        None
    }

    fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    /// A key in the composer. Enter sends; Alt-Enter and Ctrl-J (for a terminal that does not
    /// pass Alt-Enter on) start a new line; the cursor moves and deletes by grapheme, so an emoji
    /// sequence or an accented letter is one step.
    fn compose_key(&mut self, key: KeyEvent) -> Option<Action> {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let composer = self.model.composer_mut();
        match key.code {
            KeyCode::Esc => {
                composer.cancel_reply();
                self.set_pane(Pane::Messages);
            }
            KeyCode::Enter if alt => composer.newline(),
            KeyCode::Char('j') if ctrl => composer.newline(),
            KeyCode::Enter => return self.send(),
            KeyCode::Backspace => composer.backspace(),
            KeyCode::Delete => composer.delete(),
            KeyCode::Left => composer.left(),
            KeyCode::Right => composer.right(),
            KeyCode::Home => composer.home(),
            KeyCode::End => composer.end(),
            KeyCode::Char(c) => composer.insert(c),
            _ => {}
        }
        None
    }

    /// Send the composer's text to the selected chat as one message. It is shown at once as
    /// "sending" until the daemon answers.
    fn send(&mut self) -> Option<Action> {
        let frame = self.model.send()?;
        self.set_pane(Pane::Messages);
        Some(Action::Request(frame))
    }

    /// Draw the whole screen into `frame`.
    pub fn draw(&mut self, frame: &mut Frame) {
        if let Some(images) = self.images.as_mut() {
            let failures = images.collect();
            self.picture_failures(failures);
        }
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_header(frame, header);
        let [chats, messages] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(body);
        self.draw_chats(frame, chats);
        self.draw_messages(frame, messages);
        self.draw_footer(frame, footer);
        if self.help {
            self.draw_help(frame, body);
        }
        if self.picking {
            self.draw_picker(frame, body);
        }
        if self.model.transcript().search().is_some() {
            self.draw_search(frame, body);
        }
    }

    fn draw_picker(&self, frame: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        for (n, emoji) in REACTIONS.iter().enumerate() {
            spans.push(Span::styled(
                format!(" {} ", n + 1),
                Style::default().fg(rgb(self.palette.accent)),
            ));
            spans.push(Span::raw(plain(emoji)));
        }
        spans.push(Span::styled("  x remove mine", self.muted()));
        let area = popup(area, 48, 4);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(rgb(self.palette.background)))
                .block(bordered(&self.palette).title(" React ")),
            area,
        );
    }

    fn draw_search(&mut self, frame: &mut Frame, area: Rect) {
        let chat = self.model.selected_chat().cloned();
        let palette = self.palette.clone();
        let muted = self.muted();
        let Some(search) = self.model.transcript().search() else {
            return;
        };
        let area = popup(area, 72, area.height);
        frame.render_widget(Clear, area);
        let block = bordered(&palette)
            .title(" Search ")
            .style(Style::default().bg(rgb(palette.background)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [prompt, results] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let cursor = if search.is_editing() { "▏" } else { "" };
        frame.render_widget(
            Paragraph::new(format!("Search: {}{cursor}", plain(search.query())))
                .style(Style::default().fg(rgb(palette.foreground))),
            prompt,
        );
        let items: Vec<ListItem> = match (search.results(), search.is_awaiting()) {
            (_, true) => vec![ListItem::new(Span::styled("searching…", muted))],
            (None, false) => vec![ListItem::new(Span::styled("Enter to search", muted))],
            (Some([]), false) => {
                vec![ListItem::new(Span::styled("no matches", muted))]
            }
            (Some(found), false) => found
                .iter()
                .map(|m| {
                    let by = sent_by(chat.as_ref(), m, self.utc_offset, false);
                    ListItem::new(format!("{by} {}", snippet(m)))
                })
                .collect(),
        };
        let list = List::new(items)
            .style(Style::default().fg(rgb(palette.foreground)))
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        // The list clamps the chosen result to what it shows; the model keeps that choice.
        self.search_list.select(search.selected());
        frame.render_stateful_widget(list, results, &mut self.search_list);
        self.model.set_result_selected(self.search_list.selected());
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let status = match self.source {
            Source::Daemon if !self.model.connected() => Span::styled(
                format!(
                    " · {} · reconnecting… · {} unread",
                    self.model.status().daemon,
                    self.model.status().unread
                ),
                Style::default()
                    .fg(rgb(self.palette.yellow))
                    .add_modifier(Modifier::BOLD),
            ),
            Source::Daemon => Span::styled(
                format!(
                    " · {} · {}{} · {} unread",
                    self.model.status().daemon,
                    self.model.status().session.as_str(),
                    // Why WhatsApp stopped the session, as fixed text (never WhatsApp's own).
                    self.model
                        .status()
                        .stop_reason
                        .map_or_else(String::new, |r| format!(" · {}", r.describe())),
                    self.model.status().unread
                ),
                self.muted(),
            ),
            Source::Fixture => Span::styled(
                format!(
                    " · FIXTURE DATA · no daemon · {} unread",
                    self.model.status().unread
                ),
                Style::default()
                    .fg(rgb(self.palette.yellow))
                    .add_modifier(Modifier::BOLD),
            ),
        };
        let line = Line::from(vec![
            Span::styled(
                "nuncio",
                Style::default()
                    .fg(rgb(self.palette.accent))
                    .add_modifier(Modifier::BOLD),
            ),
            status,
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        // The same hint first wherever q and / are keys, then the keys of where the focus is.
        let base = |enter: &str| {
            format!(" ? help · Tab switch pane · Enter {enter} · / search · q quit · ")
        };
        let keys = match self.pane {
            _ if self.model.transcript().search().is_some() => {
                " Enter search/select · j/k move · / edit · Esc close ".to_owned()
            }
            _ if self.picking => " 1-6 react · x remove mine · Esc cancel ".to_owned(),
            Pane::Chats => base("open") + "j/k move · l open to read · Ctrl-C quit ",
            Pane::Messages if self.model.transcript().cursor().is_some() => {
                base("write")
                    + "j/k select · r reply · e react · R retry · o open media · Esc chats "
            }
            Pane::Messages => base("write") + "j/k select · PgUp/PgDn scroll · Esc chats ",
            Pane::Compose if self.model.composer().reply_to().is_some() => {
                " Enter send reply · Alt-Enter newline · ←/→ move · Esc conversation · Ctrl-C quit "
                    .to_owned()
            }
            Pane::Compose => {
                " Enter send · Alt-Enter newline · ←/→ move · Esc conversation · Ctrl-C quit "
                    .to_owned()
            }
        };
        // The notice comes first, so a narrow terminal still shows it.
        let mut spans = Vec::new();
        if let Some(notice) = self.model.notice() {
            spans.push(Span::styled(
                format!(" {notice} ·"),
                Style::default().fg(rgb(self.palette.red)),
            ));
        }
        spans.push(Span::styled(keys, self.muted()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let key = Style::default().fg(rgb(self.palette.accent));
        let what = Style::default().fg(rgb(self.palette.foreground));
        let heading = key.add_modifier(Modifier::BOLD);
        // A heading per context, then its keys on one wrapped line, so it fits in 80x24.
        let mut lines: Vec<Line> = Vec::new();
        let pictures = self.images.is_some();
        for (context, keys) in KEYS {
            lines.push(Line::from(Span::styled(*context, heading)));
            let mut spans = vec![Span::raw("  ")];
            // With pictures off, `o` opens every media externally and `O` is the same key.
            let keys: Vec<(&str, &str)> = keys
                .iter()
                .filter(|(keys, _)| pictures || *keys != "O")
                .map(|&(keys, does)| match keys {
                    "o" if !pictures => (keys, "open its media externally"),
                    _ => (keys, does),
                })
                .collect();
            for (n, (keys, does)) in keys.iter().enumerate() {
                if n > 0 {
                    spans.push(Span::styled(" · ", self.muted()));
                }
                spans.push(Span::styled(plain(keys), key));
                spans.push(Span::styled(format!(" {}", plain(does)), what));
            }
            lines.push(Line::from(spans));
        }
        // As tall as the wrapped lines need, so the last keys are not cut off.
        let help = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(rgb(self.palette.background)))
            .block(bordered(&self.palette).title(" Keys "));
        // `line_count` counts the block's rows but wraps at the width it is given: the inner one.
        let height = help.line_count(area.width.min(76).saturating_sub(2));
        let popup = popup(area, 76, u16::try_from(height).unwrap_or(u16::MAX));
        frame.render_widget(Clear, popup);
        frame.render_widget(help, popup);
    }

    fn draw_chats(&mut self, frame: &mut Frame, area: Rect) {
        let block = pane_block(&self.palette, self.pane == Pane::Chats)
            .title(format!(" Chats ({}) ", self.chats().len()));
        let rows = block.inner(area);
        // Inside the border, less the highlight symbol's column (reserved on every row).
        let width = usize::from(area.width.saturating_sub(4));
        let items: Vec<ListItem> = self
            .chats()
            .iter()
            .map(|chat| chat_item(chat, &self.palette, width, self.utc_offset))
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_symbol("› ")
            // Even with no chat highlighted (a vanished open chat), so the columns never move.
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(
                Style::default()
                    .bg(rgb(self.palette.accent))
                    .fg(rgb(self.palette.background))
                    .add_modifier(Modifier::BOLD),
            );
        self.list.select(self.model.chat_list().highlighted());
        frame.render_stateful_widget(list, area, &mut self.list);
        self.map.chats = area;
        self.map.chat_rows = rows;
        self.map.chat_offset = self.list.offset();
        self.map.chat_count = self.chats().len();
    }

    fn draw_messages(&mut self, frame: &mut Frame, area: Rect) {
        let title = self
            .model
            .selected_chat()
            .map(|chat| plain(&chat_title(chat)))
            .unwrap_or_else(|| "no chat selected".to_owned());
        let block = pane_block(&self.palette, self.pane != Pane::Chats).title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(rgb(self.palette.foreground))
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.map.conversation = area;
        self.map.message_area = Rect::default();
        self.map.message_rows.clear();
        self.map.composer = Rect::default();
        if inner.is_empty() {
            return;
        }

        // The image needs `&mut self.image`, so it is drawn before anything borrows the
        // chat list again.
        let (image_area, rest) = if self.image.is_some() {
            let [image, text] =
                Layout::vertical([Constraint::Percentage(60), Constraint::Min(1)]).areas(inner);
            (Some(image), text)
        } else {
            (None, inner)
        };
        if let (Some(image_area), Some(image)) = (image_area, self.image.as_mut())
            && !image_area.is_empty()
        {
            frame.render_stateful_widget(StatefulImage::new(), image_area, image);
        }

        // The composer is the last rows (up to five, growing upwards), under a rule, inside the
        // same border.
        let draft = self.draft_rows(rest.width);
        let height = draft
            .as_ref()
            .map_or(1, |(rows, _)| rows.len().min(COMPOSER_ROWS));
        let [text_area, rule, composer] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(height as u16),
        ])
        .areas(rest);
        self.draw_conversation(frame, text_area);
        let quoted = self
            .model
            .composer()
            .reply_to()
            .filter(|_| self.pane == Pane::Compose)
            .map(|id| match quoted(self.model.loaded(), id) {
                Some(message) => format!("── replying to: {} ", snippet(message)),
                None => "── replying ".to_owned(),
            });
        let rule_text = quoted.unwrap_or_default() + &"─".repeat(usize::from(rule.width));
        frame.render_widget(Paragraph::new(rule_text).style(self.muted()), rule);
        self.map.composer = rule.union(composer);
        self.draw_composer(frame, composer, draft);
    }

    /// The draft laid out for a composer `width` columns wide, after the 2-column prompt, plus
    /// the cursor's row and column; `None` when there is no draft to show (the hint instead).
    fn draft_rows(&self, width: u16) -> Option<DraftRows> {
        let composer = self.model.composer();
        (self.pane == Pane::Compose || !composer.is_empty()).then(|| {
            layout_draft(
                composer.text(),
                composer.caret(),
                usize::from(width.saturating_sub(2)),
            )
        })
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect, draft: Option<DraftRows>) {
        if area.is_empty() {
            return;
        }
        let Some((rows, (row, column))) = draft else {
            let hint = Line::from(Span::styled("  Enter to write a message", self.muted()));
            frame.render_widget(Paragraph::new(hint), area);
            return;
        };
        // The rows shown: from the first, or scrolled so the cursor's row is the last one.
        let visible = usize::from(area.height);
        let top = (row + 1).saturating_sub(visible);
        let text = Style::default().fg(rgb(self.palette.foreground));
        let lines: Vec<Line> = rows
            .into_iter()
            .enumerate()
            .skip(top)
            .take(visible)
            .map(|(index, shown)| {
                let prompt = if index == 0 { "› " } else { "  " };
                Line::from(vec![
                    Span::styled(prompt, Style::default().fg(rgb(self.palette.accent))),
                    Span::styled(shown, text),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
        if self.pane == Pane::Compose {
            let x = (2 + column).min(usize::from(area.width.saturating_sub(1)));
            frame.set_cursor_position((area.x + x as u16, area.y + (row - top) as u16));
        }
    }

    fn draw_conversation(&mut self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        if let Some(images) = self.images.as_mut() {
            images.resized(area.width);
            let background = self.palette.background;
            images.backdrop([background.r, background.g, background.b]);
        }
        let (lines, selected, owners, boxes) = self.conversation_lines(area.width);
        let visible = usize::from(area.height);
        if let Some(start) = selected.filter(|_| self.reveal) {
            // Scroll so the selected message's header is on screen.
            let below = Paragraph::new(lines[start..].to_vec())
                .wrap(Wrap { trim: false })
                .line_count(area.width);
            self.scroll = self
                .scroll
                .clamp(below.saturating_sub(visible), below.saturating_sub(1));
        }
        self.reveal = false;
        // How many rows each line wraps to, and which message each row belongs to (the mouse).
        let heights: Vec<usize> = lines
            .iter()
            .map(|line| {
                Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(area.width)
            })
            .collect();
        let wrapped: Vec<Option<usize>> = heights
            .iter()
            .zip(&owners)
            .flat_map(|(&rows, owner)| std::iter::repeat_n(*owner, rows))
            .collect();
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total = paragraph.line_count(area.width);
        let top = total.saturating_sub(visible);
        self.scroll = self.scroll.min(top);
        self.map.message_area = area;
        self.map.message_rows = if total < visible {
            let mut rows = vec![None; visible - total];
            rows.extend(wrapped);
            rows
        } else {
            wrapped
                .into_iter()
                .skip(top - self.scroll)
                .take(visible)
                .collect()
        };
        // The transcript row drawn on the area's first row (negative: a short one sits low).
        let first_row = if total < visible {
            -((visible - total) as i64)
        } else {
            (top - self.scroll) as i64
        };
        if total < visible {
            // Newest at the bottom: a short conversation sits on the composer.
            let area = Rect {
                y: area.y + (visible - total) as u16,
                height: total as u16,
                ..area
            };
            frame.render_widget(paragraph, area);
        } else {
            let offset = u16::try_from(top - self.scroll).unwrap_or(u16::MAX);
            frame.render_widget(paragraph.scroll((offset, 0)), area);
        }
        self.draw_pictures(frame, area, &boxes, &heights, first_row);
    }

    /// Draw each picture box that is at least partly on screen, over the blank rows reserved for
    /// it: the full picture when it is encoded, else the thumbnail (encoded on a worker the first
    /// time it shows; the box stays blank meanwhile), else, for a thumbnail that does not decode,
    /// the `[image]` or `[sticker]` label; with no thumbnail the box stays empty, since the caption
    /// line already names the kind. A box cut by the top or the bottom of the area is
    /// drawn cut. A full picture asked for with `o` is encoded whether it is on screen or not.
    fn draw_pictures(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        boxes: &[PictureBox],
        heights: &[usize],
        first_row: i64,
    ) {
        let label_style = self.muted();
        let Some(jid) = self.model.selected_jid().map(str::to_owned) else {
            return;
        };
        let Some(images) = self.images.as_mut() else {
            return;
        };
        let transcript = self.model.transcript();
        let messages = transcript.conversation(&jid).unwrap_or(&[]);
        // A full picture with no box yet (its message states no size) is decoded to learn its
        // size; one with a box is encoded only while the box is on screen (below).
        for (chat, id, path) in transcript.full_pictures() {
            let boxed = boxes
                .iter()
                .any(|b| messages.get(b.index).is_some_and(|m| m.id == id));
            if chat != jid || boxed {
                continue;
            }
            let key = images::Key {
                chat: jid.clone(),
                id: id.to_owned(),
                variant: images::Variant::Full,
                columns: area.width,
            };
            if images.wants(&key) {
                images.request(key, images::Origin::File(path.to_owned()), None);
            }
        }
        let mut starts = Vec::with_capacity(heights.len());
        let mut row = 0;
        for height in heights {
            starts.push(row);
            row += height;
        }
        let visible = i64::from(area.height);
        for picture in boxes {
            let Some(message) = messages.get(picture.index) else {
                continue;
            };
            let y = starts.get(picture.line).map_or(0, |&s| s as i64) - first_row;
            if y >= visible || y + i64::from(picture.size.height) <= 0 {
                continue;
            }
            let Ok(position_y) = i16::try_from(y) else {
                continue;
            };
            let position = SignedPosition {
                x: 0,
                y: position_y,
            };
            let key = |variant| images::Key {
                chat: jid.clone(),
                id: message.id.clone(),
                variant,
                columns: area.width,
            };
            if let Some(protocol) = images.get(&key(images::Variant::Full)) {
                frame.render_widget(SlicedImage::new(protocol, position), area);
                continue;
            }
            if let Some(path) = transcript.full_picture(&jid, &message.id) {
                let full = key(images::Variant::Full);
                if images.wants(&full) {
                    images.request(
                        full,
                        images::Origin::File(path.to_owned()),
                        Some(picture.size),
                    );
                }
            }
            let thumb = message
                .thumb
                .as_ref()
                .filter(|_| !images.has_failed(&jid, &message.id, images::Variant::Thumb));
            if let Some(thumb) = thumb {
                let thumb_key = key(images::Variant::Thumb);
                if let Some(protocol) = images.get(&thumb_key) {
                    frame.render_widget(SlicedImage::new(protocol, position), area);
                } else {
                    images.request(
                        thumb_key,
                        images::Origin::Base64(thumb.clone()),
                        Some(picture.size),
                    );
                }
                continue;
            }
            // Without a thumbnail the box stays empty until `o` fetches the picture: the caption
            // above already says `[sticker]` or `[image]`. One that does not decode keeps a label.
            if message.thumb.is_some() && (0..visible).contains(&y) {
                let label = Rect {
                    y: area.y + y as u16,
                    height: 1,
                    ..area
                };
                frame.render_widget(
                    Paragraph::new(format!("[{}]", plain(&message.kind))).style(label_style),
                    label,
                );
            }
        }
    }

    /// The box a picture message gets in a transcript `columns` wide, when pictures are drawn: from
    /// the size the message states, else from its full picture once that is decoded; `None` for
    /// any other message.
    fn picture_box(
        &self,
        jid: &str,
        message: &Message,
        columns: u16,
    ) -> Option<ratatui::layout::Size> {
        let images = self.images.as_ref()?;
        if !is_picture(&message.kind) || message.id.is_empty() {
            return None;
        }
        let (width, height) = match (message.width, message.height) {
            (Some(width), Some(height)) => (width, height),
            _ => images.full_pixels(jid, &message.id)?,
        };
        Some(images::box_size(width, height, columns, images.font()))
    }

    /// Whether the full picture of `message` is being fetched or decoded (`Downloading…`).
    fn fetching(&self, jid: &str, message: &Message, columns: u16) -> bool {
        let transcript = self.model.transcript();
        if transcript.is_showing(&message.id) {
            return true;
        }
        transcript.full_picture(jid, &message.id).is_some()
            && self.images.as_ref().is_some_and(|images| {
                images.is_pending(&images::Key {
                    chat: jid.to_owned(),
                    id: message.id.clone(),
                    variant: images::Variant::Full,
                    columns,
                })
            })
    }

    /// The selected chat's conversation, oldest first, laid out `columns` wide; before it is
    /// loaded, its last message. Also the index of the selected message's header line, for each
    /// line the index of the loaded message it belongs to, and the picture boxes (blank lines the
    /// pictures are drawn over).
    fn conversation_lines(&self, columns: u16) -> ConversationLines {
        let Some(chat) = self.model.selected_chat() else {
            return (Vec::new(), None, Vec::new(), Vec::new());
        };
        let loaded = self.model.transcript().conversation(&chat.jid);
        let messages: Vec<&Message> = match loaded {
            Some(messages) => messages.iter().collect(),
            None => chat.last_message.iter().collect(),
        };
        if messages.is_empty() {
            return (
                vec![Line::from(Span::styled("(no messages)", self.muted()))],
                None,
                vec![None],
                Vec::new(),
            );
        }
        let body = Style::default().fg(rgb(self.palette.foreground));
        let mut lines = Vec::with_capacity(messages.len() * 2);
        let mut owners = Vec::with_capacity(messages.len() * 2);
        let mut selected = None;
        let mut boxes = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            let owner = loaded.is_some().then_some(index);
            let chosen = loaded.is_some() && self.model.transcript().cursor() == Some(index);
            let mut header = if chosen {
                selected = Some(lines.len());
                format!("› {}", self.header_of(chat, message))
            } else {
                self.header_of(chat, message)
            };
            if self.model.transcript().is_sending(message) {
                header.push_str(" · sending");
            }
            let header_style = match message.state {
                Some(MessageState::NotSent) => Style::default().fg(rgb(self.palette.red)),
                _ if message.from_me => Style::default().fg(rgb(self.palette.accent)),
                _ => self.muted(),
            };
            let header_style = if chosen {
                header_style.add_modifier(Modifier::REVERSED)
            } else {
                header_style
            };
            lines.push(Line::from(Span::styled(header, header_style)));
            if let Some(quoted_id) = &message.reply_to {
                let quote = match quoted(messages.iter().copied(), quoted_id) {
                    Some(original) => snippet(original),
                    None => "(a message not loaded)".to_owned(),
                };
                lines.push(Line::from(Span::styled(format!("> {quote}"), self.muted())));
            }
            let text = if message.kind == "text" {
                message.text.clone()
            } else {
                // An animated sticker shows its first frame; the marker says it moves.
                let marker = if message.kind == "sticker" && message.animated {
                    " ▶"
                } else {
                    ""
                };
                let text = format!("[{}]{marker} {}", message.kind, message.text);
                if self.fetching(&chat.jid, message, columns) {
                    downloading(&text, columns)
                } else {
                    text
                }
            };
            lines.extend(
                text.lines()
                    .map(|line| Line::from(Span::styled(plain(line), body))),
            );
            if let Some(size) = loaded
                .is_some()
                .then(|| self.picture_box(&chat.jid, message, columns))
                .flatten()
            {
                boxes.push(PictureBox {
                    line: lines.len(),
                    size,
                    index,
                });
                lines.extend(std::iter::repeat_n(
                    Line::default(),
                    usize::from(size.height),
                ));
            }
            if !message.reactions.is_empty() {
                let mut spans = vec![Span::raw("  ")];
                for reaction in &message.reactions {
                    let style = if reaction.from_me {
                        Style::default().fg(rgb(self.palette.accent))
                    } else {
                        body
                    };
                    spans.push(Span::styled(format!("{} ", plain(&reaction.emoji)), style));
                }
                lines.push(Line::from(spans));
            }
            owners.resize(lines.len(), owner);
        }
        (lines, selected, owners, boxes)
    }

    fn header_of(&self, chat: &Chat, message: &Message) -> String {
        sent_by(
            Some(chat),
            message,
            self.utc_offset,
            self.model.transcript().is_unconfirmed(message),
        )
    }

    fn muted(&self) -> Style {
        Style::default()
            .fg(rgb(self.palette.foreground))
            .add_modifier(Modifier::DIM)
    }
}

/// The transcript as laid out: its lines, the selected message's header line, each line's
/// message, and the picture boxes.
type ConversationLines = (
    Vec<Line<'static>>,
    Option<usize>,
    Vec<Option<usize>>,
    Vec<PictureBox>,
);

/// A picture's box in the transcript: its first (blank) line, its size in cells, and the index of
/// its loaded message.
struct PictureBox {
    line: usize,
    size: ratatui::layout::Size,
    index: usize,
}

/// A picture's caption with ` · Downloading…` on its last line, cut with `…` where needed so it
/// wraps to no more rows than the caption alone: the box under it never moves.
fn downloading(caption: &str, columns: u16) -> String {
    const SUFFIX: &str = " · Downloading…";
    let rows = |text: &str| {
        Paragraph::new(plain(text))
            .wrap(Wrap { trim: false })
            .line_count(columns)
    };
    let (head, last) = match caption.rsplit_once('\n') {
        Some((head, last)) => (Some(head), last),
        None => (None, caption),
    };
    let allowed = rows(last);
    let mut kept: Vec<char> = last.chars().collect();
    let mut line = format!("{last}{SUFFIX}");
    while rows(&line) > allowed && !kept.is_empty() {
        kept.pop();
        line = format!("{}…{SUFFIX}", kept.iter().collect::<String>());
    }
    match head {
        Some(head) => format!("{head}\n{line}"),
        None => line,
    }
}

/// A draft as drawn: its rows, and the cursor's row and column in them.
type DraftRows = (Vec<String>, (usize, usize));

/// Lay `draft` out in rows of at most `width` columns: each line of it starts a row, and a line
/// wider than that wraps after its last space that fits (between graphemes when a word alone is
/// wider than the row). The last column is kept free so the cursor always has a cell after the
/// text. Rows hold the text as drawn ([`plain`]), so the columns are the ones the terminal draws;
/// the cursor (`caret`, a byte offset) is placed by the same widths.
fn layout_draft(draft: &str, caret: usize, width: usize) -> DraftRows {
    let room = width.saturating_sub(1).max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    // Where the current row may break: the byte and column just after its last space.
    let mut space: Option<(usize, usize)> = None;
    let mut cursor: Option<(usize, usize)> = None;
    for (start, grapheme) in draft.grapheme_indices(true) {
        if grapheme == "\n" || grapheme == "\r\n" {
            if cursor.is_none() && start >= caret {
                cursor = Some((rows.len() - 1, used));
            }
            rows.push(String::new());
            used = 0;
            space = None;
            continue;
        }
        let shown = plain(grapheme);
        let w = display_width(&shown);
        if used > 0 && used + w > room {
            let row = rows.len() - 1;
            match space.take().filter(|_| !grapheme.trim().is_empty()) {
                // Carry the word being written over to the next row, cursor included.
                Some((byte, column)) => {
                    let word = rows[row].split_off(byte);
                    rows.push(word);
                    used -= column;
                    if let Some((at_row, at_column)) = cursor.as_mut()
                        && *at_row == row
                        && *at_column >= column
                    {
                        *at_row += 1;
                        *at_column -= column;
                    }
                }
                None => {
                    rows.push(String::new());
                    used = 0;
                }
            }
        }
        if cursor.is_none() && start >= caret {
            cursor = Some((rows.len() - 1, used));
        }
        if let Some(row) = rows.last_mut() {
            row.push_str(&shown);
            used += w;
            if grapheme.trim().is_empty() {
                space = Some((row.len(), used));
            }
        }
    }
    let cursor = cursor.unwrap_or((rows.len() - 1, used));
    (rows, cursor)
}

/// A centred overlay at most `width` by `height`, inside `area`.
fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = area.width.min(width);
    let height = area.height.min(height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A message as one short line: its first line (or its kind, for media with no caption).
fn snippet(message: &Message) -> String {
    let text = plain(message.text.lines().next().unwrap_or_default());
    let text = if text.is_empty() {
        &message.kind
    } else {
        &text
    };
    if text.chars().count() > SNIPPET {
        text.chars().take(SNIPPET).collect::<String>() + "…"
    } else {
        text.to_owned()
    }
}

/// `text` cut to at most `width` terminal columns, ending in `…` when it was cut.
fn fit(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = char_width(c);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(c);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Columns `text` takes, as ratatui lays it out (unicode-width).
fn display_width(text: &str) -> usize {
    Span::raw(text).width()
}

fn char_width(c: char) -> usize {
    display_width(c.encode_utf8(&mut [0; 4]))
}

/// Text as it is drawn: emoji sequences are taken apart into their plain code points.
///
/// ratatui gives a whole grapheme cluster one width (`👨‍👩‍👧‍👦` and `🏳️‍🌈` are 2 columns,
/// `❤️` with its U+FE0F is 2, `👋🏽` is 2), but terminals disagree: a terminal without grapheme
/// clustering draws each code point at its own width (`👨‍👩‍👧‍👦` as 8 columns, `❤️` as 1), and
/// even Ghostty, which clusters, drew `🏳️‍🌈` and the family at a width ratatui did not expect.
/// The cursor then drifts, cells ratatui believes it cleared keep old characters (a stray `!`,
/// a stray block). Dropping the joiners (U+200D), the emoji presentation selector (U+FE0F) and
/// the skin-tone modifiers (U+1F3FB..U+1F3FF) leaves only code points whose width every
/// terminal agrees on, so the frame stays intact everywhere. The trade-off: a ZWJ sequence
/// shows as its parts (the family as four people, the rainbow flag as `🏳🌈`), `❤️` may be
/// drawn in its text style, skin tones are lost, and a ZWJ inside a non-emoji script (some
/// Indic conjuncts) is dropped too. Only the display changes: sends and reactions keep the
/// original text.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{200d}' | '\u{fe0f}' | '\u{1f3fb}'..='\u{1f3ff}'))
        .collect()
}

/// A bordered pane in the theme's accent colour.
fn bordered(palette: &Palette) -> Block<'static> {
    Block::bordered().border_style(Style::default().fg(rgb(palette.accent)))
}

/// One of the two panes: the focused one has a thick border in the accent colour, the other a
/// thin dim one, so where the keys go is visible at a glance (and without colour).
fn pane_block(palette: &Palette, focused: bool) -> Block<'static> {
    if focused {
        bordered(palette)
            .border_type(BorderType::Thick)
            .border_style(
                Style::default()
                    .fg(rgb(palette.accent))
                    .add_modifier(Modifier::BOLD),
            )
    } else {
        Block::bordered().border_style(
            Style::default()
                .fg(rgb(palette.foreground))
                .add_modifier(Modifier::DIM),
        )
    }
}

fn rgb(colour: Rgb) -> Color {
    Color::Rgb(colour.r, colour.g, colour.b)
}

/// Who sent the message (us, the group member the daemon names in `sender`, or the chat) and
/// when; a send of ours that is not sent yet says so, so a failure stays visible, and one that
/// was not confirmed (`unconfirmed`) says that instead, since it may have been delivered.
fn sent_by(
    chat: Option<&Chat>,
    message: &Message,
    offset: time::UtcOffset,
    unconfirmed: bool,
) -> String {
    let sender = message.sender.as_deref().filter(|s| !s.is_empty());
    let who = match (message.from_me, sender, chat) {
        (true, _, _) => "you".to_owned(),
        (false, Some(sender), _) => plain(sender),
        (false, None, Some(chat)) => plain(&chat_title(chat)),
        (false, None, None) => "them".to_owned(),
    };
    let state = match message.state {
        Some(MessageState::Queued) => " · queued",
        Some(MessageState::NotSent) if unconfirmed => " · not confirmed",
        Some(MessageState::NotSent) => " · not sent",
        Some(MessageState::Unknown) | None => "",
    };
    format!("{who} · {}{state}", local_hhmm(message, offset))
}

/// The message's time as `HH:MM` at `offset` from UTC (the wire's timestamps are UTC), or
/// `--:--` when it has none.
fn local_hhmm(message: &Message, offset: time::UtcOffset) -> String {
    let Some(utc) = message.time_hhmm() else {
        return "--:--".to_owned();
    };
    let (Ok(hours), Ok(minutes)) = (utc[..2].parse::<i32>(), utc[3..].parse::<i32>()) else {
        return "--:--".to_owned();
    };
    let local = (hours * 60 + minutes + i32::from(offset.whole_minutes())).rem_euclid(24 * 60);
    format!("{:02}:{:02}", local / 60, local % 60)
}

/// The marker column at the end of a chat's name row: a space, then `★ ⊘ ▤` with a space between
/// them, a missing marker left blank, so every marker sits in the same column on every row.
const MARKERS: usize = 6;
/// Below this many columns for the name, the markers give up their column: only the ones a chat
/// has are drawn, right after the name, so a very narrow list still shows who the chat is.
const NARROW_NAME: usize = 6;

/// A chat in the list: its name and unread badge, padded to the marker column, then its markers;
/// under it, its last message cut to `width` columns with `…` so the ` · HH:MM` after it is
/// always whole. The name is cut first, so the badge and the markers always show.
fn chat_item(
    chat: &Chat,
    palette: &Palette,
    width: usize,
    offset: time::UtcOffset,
) -> ListItem<'static> {
    let badge = (chat.unread > 0).then(|| format!(" ({})", chat.unread));
    let badge_width = badge.as_deref().map_or(0, display_width);
    let column = width.saturating_sub(MARKERS).saturating_sub(badge_width) >= NARROW_NAME;
    let markers = if column {
        format!(
            " {} {} {}",
            if chat.pinned { PINNED } else { ' ' },
            if chat.muted { MUTED } else { ' ' },
            if chat.archived { ARCHIVED } else { ' ' }
        )
    } else {
        let shown: String = [
            (chat.pinned, PINNED),
            (chat.muted, MUTED),
            (chat.archived, ARCHIVED),
        ]
        .iter()
        .filter_map(|&(on, marker)| on.then_some(marker))
        .collect();
        if shown.is_empty() {
            shown
        } else {
            format!(" {shown}")
        }
    };
    let markers_width = display_width(&markers);
    let room = width
        .saturating_sub(markers_width)
        .saturating_sub(badge_width);
    let name = fit(&plain(&chat_title(chat)), room);
    let mut title = vec![Span::styled(
        name.clone(),
        Style::default()
            .fg(rgb(palette.foreground))
            .add_modifier(Modifier::BOLD),
    )];
    let mut used = display_width(&name);
    if let Some(badge) = badge {
        used += display_width(&badge);
        title.push(Span::raw(" "));
        title.push(Span::styled(
            badge.trim_start().to_owned(),
            Style::default()
                .bg(rgb(palette.red))
                .fg(rgb(palette.background))
                .add_modifier(Modifier::BOLD),
        ));
    }
    let pad = width.saturating_sub(markers_width).saturating_sub(used);
    title.push(Span::raw(" ".repeat(pad)));
    title.push(Span::styled(
        markers,
        Style::default().fg(rgb(palette.accent)),
    ));

    // A message with no text (a non-text kind) shows its kind instead of an empty row.
    let detail = match chat.last_message.as_ref() {
        Some(message) => {
            let text = message.text.lines().next().unwrap_or_default();
            let text = plain(if text.is_empty() { &message.kind } else { text });
            let time = format!(" · {}", local_hhmm(message, offset));
            fit(&text, width.saturating_sub(display_width(&time))) + &time
        }
        None => "(no messages)".to_owned(),
    };

    ListItem::new(vec![
        Line::from(title),
        Line::from(Span::styled(
            detail,
            Style::default()
                .fg(rgb(palette.foreground))
                .add_modifier(Modifier::DIM),
        )),
    ])
}

#[cfg(test)]
mod tests {
    use nuncio_client::{Chat, Message};
    use time::UtcOffset;

    const UTC: UtcOffset = UtcOffset::UTC;

    fn chat(name: &str, jid: &str) -> Chat {
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

    /// A 80x24 screen: the list in columns 0..32 (two chats from row 2, scrolled by one), the
    /// conversation in 32..80 with its messages in rows 2..20 and the composer in rows 20..23.
    fn map() -> super::ScreenMap {
        use ratatui::layout::Rect;
        let mut message_rows = vec![None; 18];
        message_rows[14] = Some(0);
        message_rows[15] = Some(0);
        message_rows[16] = Some(1);
        message_rows[17] = Some(1);
        super::ScreenMap {
            chats: Rect::new(0, 1, 32, 22),
            chat_rows: Rect::new(1, 2, 30, 20),
            chat_offset: 1,
            chat_count: 3,
            conversation: Rect::new(32, 1, 48, 22),
            message_area: Rect::new(33, 2, 46, 18),
            message_rows,
            composer: Rect::new(33, 20, 46, 2),
        }
    }

    #[test]
    fn hit_test_maps_a_cell_to_the_chat_message_or_composer_under_it() {
        use super::{Target, hit_test};
        let map = map();
        // Each chat takes two rows, counted from the list's scroll.
        assert_eq!(hit_test(&map, 5, 2), Some(Target::Chats(Some(1))));
        assert_eq!(hit_test(&map, 30, 3), Some(Target::Chats(Some(1))));
        assert_eq!(hit_test(&map, 5, 4), Some(Target::Chats(Some(2))));
        // Below the last chat, and on the border: the pane, no chat.
        assert_eq!(hit_test(&map, 5, 6), Some(Target::Chats(None)));
        assert_eq!(hit_test(&map, 0, 2), Some(Target::Chats(None)));
        assert_eq!(hit_test(&map, 31, 2), Some(Target::Chats(None)));
        // The conversation's rows name their message; an empty row, the border: none.
        assert_eq!(hit_test(&map, 40, 16), Some(Target::Conversation(Some(0))));
        assert_eq!(hit_test(&map, 78, 19), Some(Target::Conversation(Some(1))));
        assert_eq!(hit_test(&map, 40, 5), Some(Target::Conversation(None)));
        assert_eq!(hit_test(&map, 79, 10), Some(Target::Conversation(None)));
        assert_eq!(hit_test(&map, 33, 20), Some(Target::Composer));
        assert_eq!(hit_test(&map, 78, 21), Some(Target::Composer));
        // The header and the footer are neither pane.
        assert_eq!(hit_test(&map, 5, 0), None);
        assert_eq!(hit_test(&map, 40, 23), None);
        // Nothing drawn yet: nothing to hit.
        assert_eq!(hit_test(&super::ScreenMap::default(), 5, 5), None);
    }

    #[test]
    fn message_times_are_shown_as_utc_hh_mm_with_no_offset() {
        let message = Message {
            id: "3EB0AAA1".to_owned(),
            kind: "text".to_owned(),
            text: "hello".to_owned(),
            from_me: false,
            timestamp: "2026-10-18T17:55:04Z".to_owned(),
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
        };
        assert_eq!(
            super::sent_by(Some(&chat("Ada", "a@b")), &message, UTC, false),
            "Ada · 17:55"
        );
    }

    #[test]
    fn a_send_that_is_not_sent_yet_says_so() {
        let mut message = Message {
            id: "3EB0BBB3".to_owned(),
            kind: "text".to_owned(),
            text: "On my way".to_owned(),
            from_me: true,
            timestamp: "2026-10-18T17:56:12Z".to_owned(),
            media: None,
            state: Some(nuncio_client::wire::MessageState::NotSent),
            client_id: Some("c-1".to_owned()),
            reply_to: None,
            reactions: Vec::new(),
            thumb: None,
            thumb_mime: None,
            width: None,
            height: None,
            animated: false,
            sender: None,
        };
        assert_eq!(
            super::sent_by(None, &message, UTC, false),
            "you · 17:56 · not sent"
        );
        message.state = Some(nuncio_client::wire::MessageState::Queued);
        assert_eq!(
            super::sent_by(None, &message, UTC, false),
            "you · 17:56 · queued"
        );
        message.state = None;
        assert_eq!(super::sent_by(None, &message, UTC, false), "you · 17:56");
    }
}
