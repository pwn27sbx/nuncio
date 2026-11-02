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

use std::collections::HashSet;
use std::path::PathBuf;

use nuncio_client::model::{Effect, Filter, Model, chat_title, is_picture, quoted};
use nuncio_client::wire::{MessageState, Session};
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Rgb, Snapshot};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
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
    /// The daemon was unreachable: an empty, unlinked screen that says so — never demo data that
    /// a user could mistake for their own chats.
    Unavailable,
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
    /// The chat pane, its header rule and the line after it included.
    pub chats: Rect,
    /// The rows of the chat list under its header rule; each chat takes [`CHAT_ROWS`] rows.
    pub chat_rows: Rect,
    /// The list's scroll: the index of the chat on its first row.
    pub chat_offset: usize,
    /// The model index of every chat row drawn, in order: a filter shows a subset, and a click
    /// has to land on the chat it is drawn for, not on its position on screen.
    pub chat_indices: Vec<usize>,
    /// The conversation pane, its header rule included (the composer is inside it).
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
    /// The chat pane: the chat at this list index, or none (a rule or line, below the last
    /// chat).
    Chats(Option<usize>),
    /// The conversation pane: the loaded message at this index, or none.
    Conversation(Option<usize>),
    /// The composer.
    Composer,
}

/// Rows one chat takes in the list: its name, its last message, then the thin rule that
/// separates it from the next one.
pub const CHAT_ROWS: u16 = 3;

/// What is drawn at `(column, row)` on the screen `map` describes, if anything the mouse acts on.
pub fn hit_test(map: &ScreenMap, column: u16, row: u16) -> Option<Target> {
    let inside = |area: Rect| area.contains(ratatui::layout::Position::new(column, row));
    if inside(map.chats) {
        let index = inside(map.chat_rows)
            .then(|| map.chat_offset + usize::from((row - map.chat_rows.y) / CHAT_ROWS))
            .and_then(|row_index| map.chat_indices.get(row_index).copied());
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
/// Marks a muted chat: a quiet dotted circle, not the old `⊘` that read as "forbidden".
const MUTED: char = '◌';
/// Marks an archived chat.
const ARCHIVED: char = '▤';

/// The chat list's least width: 30 % of a narrow body would be unreadable.
const LIST: u16 = 32;
/// The chat list's most width: past this the conversation is the one that suffers.
const LIST_MAX: u16 = 48;
/// Below this width only the focused pane is drawn, and `Tab` swaps it (R6).
const NARROW: u16 = 60;
/// The chat list's share of the body: 30 % of it, never under [`LIST`] and never over
/// [`LIST_MAX`]. The conversation takes everything else.
fn list_width(width: u16) -> u16 {
    // u32 arithmetic: `width * 30` overflows a u16 past 2184 columns and panics in debug.
    (u32::from(width) * 30 / 100).clamp(u32::from(LIST), u32::from(LIST_MAX)) as u16
}

/// How many lines PageUp/PageDown scroll.
const PAGE: usize = 10;
/// A message's bubble takes at most this share of the conversation's columns; the rest is the
/// gap that shows which side a message came from.
const BUBBLE_SHARE: u32 = 66;

/// What a narrow window's only visible pane says about returning to the other one. It is the
/// first thing the header keeps: without it a narrow window has no way back but the keys.
const NARROW_HINT: &str = " · Tab";
/// The narrowest a bubble gets, so a one-word message still reads as a bubble around its time.
const BUBBLE_MIN: u16 = 11;
/// The clock on a queued outgoing bubble, where the delivery ticks would go.
const CLOCK: char = '◷';
/// The `!` on an outgoing bubble that was not sent.
const NOT_SENT: char = '!';
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
            ("[ ]", "filter: All, Unread, Archived"),
            ("Tab", "focus the conversation"),
            ("/", "search the list"),
            ("q Esc", "quit"),
        ],
    ),
    (
        "Conversation",
        &[
            ("/", "search the chat"),
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
    /// Which chats the list shows (R6/Q3): All, Unread or Archived, cycled with `[`/`]`.
    filter: Filter,
    /// What the list is searched for, typed there with `/` (never sent: it only filters).
    query: String,
    /// The list's search field has the keys.
    typing: bool,
    /// True on the last draw when only one pane fits (under `NARROW` columns): the rule says
    /// so and `Tab` is the only way to the other one.
    narrow: bool,
    /// The pictures' protocol name, shown in `?`.
    protocol: Option<&'static str>,
    utc_offset: time::UtcOffset,
    /// The local date the transcript calls today (`Today`/`Yesterday` on the day separators).
    today: time::Date,
    /// Where things were on the last drawn screen, for the mouse.
    map: ScreenMap,
    /// What the last draw asked the daemon for (a picture that came into view), drained by the
    /// loop with [`App::take_actions`].
    pending: Vec<Action>,
    /// The pictures already asked for, so a fetch that failed is never retried by itself.
    pictures_wanted: HashSet<(String, String)>,
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
            filter: Filter::All,
            query: String::new(),
            typing: false,
            narrow: false,
            protocol: None,
            utc_offset: time::UtcOffset::UTC,
            today: time::OffsetDateTime::now_utc().date(),
            map: ScreenMap::default(),
            pending: Vec::new(),
            pictures_wanted: HashSet::new(),
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
        self.protocol = Some(graphics.protocol_name());
        self
    }

    /// What the last draw asked the daemon for (the full picture of a picture that came into
    /// view), for the loop to send before the next frame.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.pending)
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
        // The day labels are local: a message at 01:00 UTC is yesterday three hours west.
        self.today = time::OffsetDateTime::now_utc().to_offset(offset).date();
        self
    }

    /// Treat `today` as the day the transcript calls `Today`: the day separators say `Yesterday`
    /// for the day before it and the date for any other. Tests set it so the labels are fixed;
    /// `main` leaves it to [`App::with_utc_offset`] and the clock.
    pub fn with_today(mut self, today: time::Date) -> Self {
        self.today = today;
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
        // Walk the list the filter shows, not the model's whole one: otherwise a step lands on a
        // hidden chat and the selection appears not to move.
        let listed = self.listed();
        if listed.is_empty() {
            return;
        }
        let at = listed
            .iter()
            .position(|&index| index == self.selected())
            .unwrap_or(0);
        let next = listed[(at + 1).min(listed.len() - 1)];
        self.model.select_chat(next);
        self.sync();
    }

    /// Select the previous chat, stopping at the first one.
    pub fn select_previous(&mut self) {
        let listed = self.listed();
        if listed.is_empty() {
            return;
        }
        let at = listed
            .iter()
            .position(|&index| index == self.selected())
            .unwrap_or(0);
        let previous = listed[at.saturating_sub(1)];
        self.model.select_chat(previous);
        self.sync();
    }

    pub fn toggle_pane(&mut self) {
        self.set_pane(match self.pane {
            Pane::Chats => Pane::Messages,
            Pane::Messages | Pane::Compose => Pane::Chats,
        });
    }

    /// The model indices of the chats the list shows, in its order, under the current filter.
    pub fn listed(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        self.model
            .chat_list()
            .chats()
            .iter()
            .enumerate()
            .filter(|(_, chat)| self.filter.keeps(chat))
            .filter(|(_, chat)| {
                query.is_empty()
                    || chat_title(chat).to_lowercase().contains(&query)
                    || chat
                        .aliases
                        .iter()
                        .any(|alias| alias.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// What the list is searched for, and whether its field has the keys (for the tests and the
    /// list's own bar).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the list's search field has the keys.
    pub fn typing(&self) -> bool {
        self.typing
    }

    /// Cycle the chat list's filter with `[`/`]` and land the selection on a chat it shows.
    pub fn cycle_filter(&mut self, next: bool) {
        let at = Filter::EACH
            .iter()
            .position(|filter| *filter == self.filter)
            .unwrap_or(0);
        let len = Filter::EACH.len();
        self.filter = Filter::EACH[if next {
            (at + 1) % len
        } else {
            (at + len - 1) % len
        }];
        self.ensure_selected_listed();
    }

    /// Keep the selection on a chat the filter shows: the nearest one at or after it, else the
    /// last one before it. A filter that hides everything leaves the selection where it is and
    /// the pane says so, so no hidden chat is ever selected.
    fn ensure_selected_listed(&mut self) {
        // Unfiltered and unsearched, the model already keeps the selection on a chat; only a
        // filter or a query can hide it.
        if (self.filter == Filter::All && self.query.trim().is_empty())
            || self.model.chat_list().chats().is_empty()
        {
            return;
        }
        let listed = self.listed();
        if listed.is_empty() || listed.contains(&self.selected()) {
            return;
        }
        let target = listed
            .iter()
            .find(|&&index| index >= self.selected())
            .or_else(|| listed.last())
            .copied();
        if let Some(target) = target {
            self.model.select_chat(target);
        }
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
        // A frame can move a chat out of the filter (it was read, archived): the selection has to
        // land somewhere it can still see.
        self.ensure_selected_listed();
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
        if self.typing {
            match key.code {
                // Esc gives the list back whole; Enter keeps the query as a filter.
                KeyCode::Esc => {
                    self.query.clear();
                    self.typing = false;
                }
                KeyCode::Enter => self.typing = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => return None,
            }
            self.ensure_selected_listed();
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
            // `/` searches the list when the keys are on it, the open chat otherwise.
            (Pane::Chats, KeyCode::Char('/')) => self.typing = true,
            (_, KeyCode::Char('/')) if chosen => {
                self.model.open_search();
                self.search_list = ListState::default();
            }
            (Pane::Chats, KeyCode::Char('j') | KeyCode::Down) => self.select_next(),
            (Pane::Chats, KeyCode::Char('k') | KeyCode::Up) => self.select_previous(),
            (Pane::Chats, KeyCode::Enter) => return self.open_to_write(),
            (Pane::Chats, KeyCode::Char('l') | KeyCode::Right) => return self.open_selected(),
            (_, KeyCode::Char('[')) => self.cycle_filter(false),
            (_, KeyCode::Char(']')) => self.cycle_filter(true),
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

    /// Draw the whole screen into `frame` (design plan §9): the header, the chat list on the
    /// chrome and the conversation on the surface with one line between them, and the footer.
    pub fn draw(&mut self, frame: &mut Frame) {
        if let Some(images) = self.images.as_mut() {
            let failures = images.collect();
            self.picture_failures(failures);
        }
        frame.render_widget(
            Block::new().style(
                Style::default()
                    .bg(rgb(self.palette.surface()))
                    .fg(rgb(self.palette.text())),
            ),
            frame.area(),
        );
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_header(frame, header);
        // What the mouse can hit is rebuilt every frame: a pane that is not drawn must not keep
        // last frame's rectangles (a click in a hidden pane would land on the visible one).
        self.map.chats = Rect::default();
        self.map.conversation = Rect::default();
        self.map.message_area = Rect::default();
        self.map.composer = Rect::default();
        self.narrow = body.width < NARROW;
        if self.narrow {
            if self.pane == Pane::Chats {
                self.draw_chats(frame, body);
            } else {
                self.draw_messages(frame, body);
            }
        } else {
            let [chats, line, messages] = Layout::horizontal([
                Constraint::Length(list_width(body.width)),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(body);
            self.draw_separator(frame, line);
            self.draw_chats(frame, chats);
            // The line between the panes is part of the chat pane for the mouse.
            self.map.chats = chats.union(line);
            self.draw_messages(frame, messages);
        }
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

    /// The one line between the panes: it meets the header rule with `┬`.
    fn draw_separator(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let lines: Vec<Line> = (0..area.height)
            .map(|row| Line::from(if row == 0 { "┬" } else { "│" }))
            .collect();
        frame.render_widget(Paragraph::new(lines).style(self.line_style()), area);
    }

    /// The conversation's header rule: the chat's avatar chip, its name and a subtitle the wire
    /// owns (`…0001 · contact` or `group`). The focused pane's name is bold, the other's muted.
    fn draw_conversation_header(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        let focused = self.pane != Pane::Chats;
        let name_style = if focused {
            Style::default()
                .fg(rgb(self.palette.text()))
                .add_modifier(Modifier::BOLD)
        } else {
            self.muted()
        };
        let mut spans = vec![Span::styled("─", self.line_style())];
        let mut used = 1;
        // A narrow window draws one pane, so its header always ends with how to switch panes:
        // the name and the subtitle leave that hint its columns.
        let reserved = if self.narrow {
            display_width(NARROW_HINT)
        } else {
            0
        };
        match self.model.selected_chat() {
            Some(chat) => {
                let title = plain(&chat_title(chat));
                let seed = avatar_seed(&title) % AVATAR_TINTS;
                let tint = rgb(self.palette.avatar_tint(seed));
                let ink = rgb(self.palette.avatar_ink(seed));
                let initial = avatar_initials(&title)[0];
                spans.push(Span::raw(" "));
                spans.push(Span::styled("▗", Style::default().fg(tint)));
                spans.push(Span::styled(
                    initial.to_string(),
                    Style::default().fg(ink).bg(tint),
                ));
                spans.push(Span::styled("▖", Style::default().fg(tint)));
                used += 4;
                let room = width.saturating_sub(used + 1 + reserved);
                let name = fit(&format!(" {title}"), room);
                used += display_width(&name);
                spans.push(Span::styled(name, name_style));
                if let Some(subtitle) = chat_subtitle(chat) {
                    let room = width.saturating_sub(used + 1 + reserved);
                    let subtitle = fit(&format!(" · {subtitle}"), room);
                    used += display_width(&subtitle);
                    spans.push(Span::styled(subtitle, self.muted()));
                }
            }
            // With no chat the pane still has a title, so its focus shows.
            None => {
                let mut title = " Conversation".to_owned();
                if self.narrow {
                    title.push_str(NARROW_HINT);
                }
                used += display_width(&title);
                spans.push(Span::styled(title, name_style));
            }
        }
        if self.narrow && self.model.selected_chat().is_some() {
            let room = width.saturating_sub(used + 1);
            let suffix = fit(NARROW_HINT, room);
            used += display_width(&suffix);
            spans.push(Span::styled(suffix, self.muted()));
        }
        spans.push(Span::styled(
            "─".repeat(width.saturating_sub(used)),
            self.line_style(),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// A pane's header: its title on the header rule. The focused pane's title is bold, the
    /// other's muted, so where the keys go shows without colour.
    fn draw_rule_title(&self, frame: &mut Frame, area: Rect, title: String, focused: bool) {
        if area.is_empty() {
            return;
        }
        let style = if focused {
            Style::default()
                .fg(rgb(self.palette.text()))
                .add_modifier(Modifier::BOLD)
        } else {
            self.muted()
        };
        let mut spans = vec![Span::styled("─", self.line_style())];
        let mut used = 1;
        if !title.is_empty() {
            let title = fit(
                &format!(" {title} "),
                usize::from(area.width).saturating_sub(1),
            );
            used += display_width(&title);
            spans.push(Span::styled(title, style));
        }
        spans.push(Span::styled(
            "─".repeat(usize::from(area.width).saturating_sub(used)),
            self.line_style(),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The width every overlay takes: four columns narrower than the screen, and never wider
    /// than 76 (R8).
    fn overlay_width(area: Rect) -> u16 {
        area.width.saturating_sub(4).min(76)
    }

    /// A panel over `area` hidden behind a solid `surface` scrim (a terminal has no alpha): the
    /// centred panel is the chrome, its content inset by one column on each side.
    fn panel(&self, frame: &mut Frame, area: Rect, scrim: bool, width: u16, height: u16) -> Rect {
        if scrim {
            frame.render_widget(Clear, area);
            frame.render_widget(
                Block::new().style(Style::default().bg(rgb(self.palette.surface()))),
                area,
            );
        }
        let panel = popup(area, width, height);
        frame.render_widget(Clear, panel);
        frame.render_widget(
            Block::new().style(
                Style::default()
                    .bg(rgb(self.palette.surface_chrome()))
                    .fg(rgb(self.palette.text())),
            ),
            panel,
        );
        Rect {
            x: panel.x.saturating_add(1).min(panel.right()),
            width: panel.width.saturating_sub(2),
            ..panel
        }
    }

    fn draw_picker(&self, frame: &mut Frame, area: Rect) {
        let width = Self::overlay_width(area);
        // Six `n emoji` pairs plus `x` need 52 columns; below that each takes its own row, keys
        // right-aligned like every other overlay (R8).
        let one_row = width >= 52;
        let mut lines = vec![self.panel_title("React"), Line::default()];
        if one_row {
            let mut spans = Vec::new();
            for (n, emoji) in REACTIONS.iter().enumerate() {
                spans.push(Span::styled(format!(" {} ", n + 1), self.muted()));
                spans.push(Span::raw(plain(emoji)));
            }
            spans.push(Span::styled("  x remove mine", self.muted()));
            lines.push(Line::from(spans));
        } else {
            let inner = usize::from(width.saturating_sub(2));
            for (n, emoji) in REACTIONS.iter().enumerate() {
                let emoji = plain(emoji);
                let pad = inner.saturating_sub(display_width(&emoji) + 2).max(1);
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(emoji, Style::default().fg(rgb(self.palette.text()))),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(format!("{} ", n + 1), self.muted()),
                ]));
            }
            lines.push(Line::from(Span::styled(" x  remove mine ", self.muted())));
        }
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let inner = self.panel(frame, area, false, width, height);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn panel_title(&self, title: &str) -> Line<'static> {
        Line::from(Span::styled(
            title.to_owned(),
            Style::default()
                .fg(rgb(self.palette.text()))
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn draw_search(&mut self, frame: &mut Frame, area: Rect) {
        let chat = self.model.selected_chat().cloned();
        let muted = self.muted();
        let text = Style::default().fg(rgb(self.palette.text()));
        let bar = Line::from(vec![
            Span::styled("▌", Style::default().fg(rgb(self.palette.accent()))),
            Span::raw(" "),
        ]);
        let active = Style::default().bg(rgb(self.palette.surface_active()));
        let inner = self.panel(frame, area, true, Self::overlay_width(area), area.height);
        let Some(search) = self.model.transcript().search() else {
            return;
        };
        let [prompt, results] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let cursor = if search.is_editing() { "▏" } else { "" };
        frame.render_widget(
            Paragraph::new(format!("Search: {}{cursor}", plain(search.query()))).style(text),
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
            .style(text)
            .highlight_symbol(bar)
            .highlight_style(active);
        // The list clamps the chosen result to what it shows; the model keeps that choice.
        self.search_list.select(search.selected());
        frame.render_stateful_widget(list, results, &mut self.search_list);
        self.model.set_result_selected(self.search_list.selected());
    }

    /// The one chrome line: the app's name on the left, and one right-aligned datum — the
    /// unread count in the accent, or the fixture warning. No version, no session state, no
    /// daemon name: those live in `?` and in the footer's right slot (R1).
    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let width = usize::from(area.width);
        let (right, style) = match self.source {
            Source::Unavailable => (
                "NO DAEMON".to_owned(),
                Style::default()
                    .fg(rgb(self.palette.warning()))
                    .add_modifier(Modifier::BOLD),
            ),
            Source::Daemon if self.model.status().unread > 0 => (
                format!("{} unread", self.model.status().unread),
                Style::default().fg(rgb(self.palette.accent())),
            ),
            Source::Daemon => (String::new(), Style::default()),
        };
        let brand = "nuncio";
        let mut spans = vec![Span::styled(
            brand,
            Style::default()
                .fg(rgb(self.palette.text()))
                .add_modifier(Modifier::BOLD),
        )];
        if !right.is_empty() {
            let right = fit(&right, width.saturating_sub(display_width(brand) + 1));
            let pad = width.saturating_sub(display_width(brand) + display_width(&right));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(right, style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::default().bg(rgb(self.palette.surface_chrome()))),
            area,
        );
    }

    /// The session state for the footer's right slot, and whether it is a warning (anything
    /// but `linked`).
    fn state_label(&self) -> (String, bool) {
        match self.source {
            Source::Unavailable => ("no daemon".to_owned(), true),
            Source::Daemon if !self.model.connected() => ("reconnecting…".to_owned(), true),
            Source::Daemon => {
                let status = self.model.status();
                let label = match &status.session {
                    Session::Unlinked => "not linked".to_owned(),
                    other => other.as_str().to_owned(),
                };
                let label = match status.stop_reason {
                    Some(reason) => format!("{} · {}", label, reason.describe()),
                    None => label,
                };
                (label, !status.linked)
            }
        }
    }

    /// The keys the footer offers where the focus is: the primary key and its verb (the key is
    /// drawn in the accent), then the optional ones, first in the list is dropped last.
    fn footer_hints(
        &self,
    ) -> (
        &'static str,
        &'static str,
        Vec<(&'static str, &'static str)>,
    ) {
        if self.model.transcript().search().is_some() {
            return (
                "Enter",
                "search/select",
                vec![("j/k", "move"), ("/", "edit"), ("Esc", "close")],
            );
        }
        if self.picking {
            return ("1-6", "react", vec![("x", "remove"), ("Esc", "cancel")]);
        }
        match self.pane {
            Pane::Chats => (
                "Enter",
                "open",
                vec![
                    ("j/k", "move"),
                    ("/", "search"),
                    ("[ ]", "filter"),
                    ("i", "write"),
                    ("l", "read"),
                    ("Tab", "pane"),
                ],
            ),
            Pane::Messages if self.model.transcript().cursor().is_some() => (
                "Enter",
                "write",
                vec![
                    ("j/k", "select"),
                    ("r", "reply"),
                    ("e", "react"),
                    ("R", "retry"),
                    ("o", "open"),
                    ("Esc", "chats"),
                ],
            ),
            Pane::Messages => (
                "Enter",
                "write",
                vec![
                    ("j/k", "select"),
                    ("Esc", "chats"),
                    ("/", "search"),
                    ("Tab", "pane"),
                ],
            ),
            Pane::Compose => (
                "Enter",
                "send",
                vec![("Alt-Enter", "newline"), ("Esc", "conversation")],
            ),
        }
    }

    /// One line: a notice if there is one, the pane's primary key in the accent, the keys that
    /// still fit, `? all keys`, and the session state on the right. A part is dropped from the
    /// tail — the optional keys first, then `? all keys`, then the primary key — so the state is
    /// the last thing to give way, and only when the line cannot hold it (R2).
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let width = usize::from(area.width);
        let (key, verb, hints) = self.footer_hints();
        let primary: Vec<(String, Style)> = vec![
            (
                key.to_owned(),
                Style::default().fg(rgb(self.palette.accent())),
            ),
            (format!(" {verb}"), self.muted()),
        ];
        let all_keys: Vec<(String, Style)> = vec![("? all keys".to_owned(), self.muted())];
        let notice = self.model.notice();

        let (state, warn) = self.state_label();
        let gap = self.muted();
        // What gives way, in order: the optional keys from the tail, then `? all keys`, then the
        // primary key. The state is the last thing to give way, and only by being cut (R2).
        let mut extras: Vec<Vec<(String, Style)>> = hints
            .into_iter()
            .map(|(key, what)| vec![(format!("{key} {what}"), self.muted())])
            .collect();
        let mut head: Vec<Vec<(String, Style)>> = Vec::new();
        if let Some(notice) = notice {
            head.push(vec![(
                notice.to_owned(),
                Style::default().fg(rgb(self.palette.warning())),
            )]);
        }
        head.push(primary.clone());
        let mut show_all_keys = true;
        let mut parts;
        loop {
            parts = head.clone();
            parts.extend(extras.iter().cloned());
            if show_all_keys {
                parts.push(all_keys.clone());
            }
            let spans = footer_spans(&parts, gap);
            if line_width(&spans) + 2 + display_width(&state) <= width {
                break;
            }
            if extras.pop().is_some() {
                continue;
            }
            if show_all_keys {
                show_all_keys = false;
                continue;
            }
            // The notice stays; the primary key is what is left to drop.
            if head.len() > 1 {
                head.pop();
                continue;
            }
            // A notice on its own can still be too long: it is the only part that may be cut.
            let room = width.saturating_sub(2 + display_width(&state));
            if let Some((text, _)) = head.first_mut().and_then(|part| part.first_mut()) {
                *text = fit(text, room);
            }
            parts = head.clone();
            parts.extend(extras.iter().cloned());
            if show_all_keys {
                parts.push(all_keys.clone());
            }
            break;
        }

        let mut spans = footer_spans(&parts, gap);
        // One leading blank, the hints, then the state right-aligned.
        let state = fit(&state, width.saturating_sub(2 + line_width(&spans)));
        let pad = width.saturating_sub(1 + line_width(&spans) + display_width(&state));
        let state_style = if warn {
            Style::default().fg(rgb(self.palette.warning()))
        } else {
            self.muted()
        };
        let mut line = vec![Span::raw(" ")];
        line.append(&mut spans);
        line.push(Span::raw(" ".repeat(pad)));
        line.push(Span::styled(state, state_style));
        frame.render_widget(
            Paragraph::new(Line::from(line))
                .style(Style::default().bg(rgb(self.palette.surface_chrome()))),
            area,
        );
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let key = self.muted();
        let what = Style::default().fg(rgb(self.palette.text()));
        let heading = what.add_modifier(Modifier::BOLD);
        let pictures = self.images.is_some();
        // With pictures off, `o` opens every media externally and `O` is the same key.
        let contexts: Vec<(&str, Vec<(&str, &str)>)> = KEYS
            .iter()
            .map(|(context, keys)| {
                let keys = keys
                    .iter()
                    .filter(|(keys, _)| pictures || *keys != "O")
                    .map(|&(keys, does)| match keys {
                        "o" if !pictures => (keys, "open its media externally"),
                        _ => (keys, does),
                    })
                    .collect();
                (*context, keys)
            })
            .collect();
        let width = Self::overlay_width(area);
        let inner_width = usize::from(width.saturating_sub(2));
        let tall: usize = 2 + contexts
            .iter()
            .map(|(_, keys)| 1 + keys.len())
            .sum::<usize>();
        let mut lines: Vec<Line> = vec![self.panel_title("Keys")];
        // Where the version and the session state live now that the header carries neither:
        // the daemon's build, the session's state and, with pictures on, their protocol.
        let status = self.model.status();
        let mut about = format!("{} · {}", status.daemon, status.session.as_str());
        if let Some(protocol) = self.protocol {
            about.push_str(&format!(" · pictures: {protocol}"));
        }
        lines.push(Line::from(Span::styled(format!("  {about}"), self.muted())));
        if tall <= usize::from(area.height) {
            // A row per key: what it does, then the key right-aligned and muted.
            for (context, keys) in &contexts {
                lines.push(Line::from(Span::styled(*context, heading)));
                for (keys, does) in keys {
                    let does = format!("  {}", plain(does));
                    let keys = plain(keys);
                    let pad = inner_width
                        .saturating_sub(display_width(&does))
                        .saturating_sub(display_width(&keys))
                        .max(1);
                    lines.push(Line::from(vec![
                        Span::styled(does, what),
                        Span::raw(" ".repeat(pad)),
                        Span::styled(keys, key),
                    ]));
                }
            }
        } else {
            // A heading per context, then its keys on one wrapped line, so it fits in 80x24.
            for (context, keys) in &contexts {
                lines.push(Line::from(Span::styled(*context, heading)));
                let mut spans = vec![Span::raw("  ")];
                for (n, (keys, does)) in keys.iter().enumerate() {
                    if n > 0 {
                        spans.push(Span::styled(" · ", self.muted()));
                    }
                    spans.push(Span::styled(plain(keys), key));
                    spans.push(Span::styled(format!(" {}", plain(does)), what));
                }
                lines.push(Line::from(spans));
            }
        }
        // As tall as the wrapped lines need, so the last keys are not cut off.
        let help = Paragraph::new(lines).wrap(Wrap { trim: false });
        let height = help.line_count(width.saturating_sub(2));
        let inner = self.panel(
            frame,
            area,
            true,
            width,
            u16::try_from(height).unwrap_or(u16::MAX),
        );
        frame.render_widget(help, inner);
    }

    fn draw_chats(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Block::new().style(Style::default().bg(rgb(self.palette.surface_chrome()))),
            area,
        );
        let [rule, bar, rows] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);
        let listed = self.listed();
        let mut title = if self.filter == Filter::All {
            format!("Chats ({})", self.chats().len())
        } else {
            format!("Chats · {} ({})", self.filter.name(), listed.len())
        };
        if self.narrow {
            title.push_str(" · Tab");
        }
        self.draw_rule_title(frame, rule, title, self.pane == Pane::Chats);
        self.draw_filter_bar(frame, bar);
        // Less the gutter: the bar, the dot and a space.
        let width = usize::from(area.width.saturating_sub(3));
        let highlighted = self.model.chat_list().highlighted();
        let items: Vec<ListItem> = listed
            .iter()
            .map(|&index| {
                let chat = &self.chats()[index];
                chat_item(
                    chat,
                    &self.palette,
                    width,
                    self.utc_offset,
                    highlighted == Some(index),
                )
            })
            .collect();
        if items.is_empty() {
            // An empty list and a filter that hides everything are two different things, and an
            // unlinked app is a third: it points at nuncio, never at the command line (S10).
            let hint = if self.source == Source::Unavailable {
                "   nunciod is not running · open nuncio to start it"
            } else if self.model.status().session == Session::Unlinked {
                "   No device linked · open nuncio to link it"
            } else if self.chats().is_empty() {
                "   No chats yet"
            } else {
                "   No chats under this filter · [ ]"
            };
            // The sentence is longer than a narrow list pane: wrapped, it stays readable instead
            // of losing its second half.
            frame.render_widget(
                Paragraph::new(Span::styled(hint, self.muted())).wrap(Wrap { trim: false }),
                rows,
            );
        }
        let list = List::new(items);
        // The list's state is the position on screen, not the model's index.
        let position = highlighted.and_then(|index| listed.iter().position(|&i| i == index));
        self.list.select(position);
        frame.render_stateful_widget(list, rows, &mut self.list);
        // The avatar and the pill are drawn again over the open chat's fill, which would take
        // their backgrounds.
        if let Some(position) = position
            && position >= self.list.offset()
        {
            let first =
                rows.y + u16::try_from(position - self.list.offset()).unwrap_or(0) * CHAT_ROWS;
            let row = first + 1;
            let chat = &self.chats()[listed[position]];
            paint_avatar(
                frame.buffer_mut(),
                Rect { y: first, ..rows },
                &plain(&chat_title(chat)),
                &self.palette,
            );
            let pill = unread_pill(chat.unread);
            if !pill.is_empty() && row < rows.bottom() {
                let width = usize::from(area.width.saturating_sub(3));
                let x =
                    rows.x + u16::try_from(width.saturating_sub(display_width(&pill))).unwrap_or(0);
                frame.buffer_mut().set_stringn(
                    x,
                    row,
                    &pill,
                    PILL,
                    Style::default()
                        .fg(rgb(self.palette.accent_on()))
                        .bg(rgb(self.palette.accent())),
                );
            }
        }
        self.map.chat_rows = rows;
        self.map.chat_offset = self.list.offset();
        self.map.chat_indices = listed;
    }

    /// The list's filter bar: All, Unread and Archived, the one in force a filled accent chip
    /// and the others muted. While the list is searched the bar is the search field instead.
    fn draw_filter_bar(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        let mut spans = Vec::new();
        if self.typing || !self.query.is_empty() {
            let cursor = if self.typing { "▏" } else { "" };
            let field = fit(&format!("search: {}{cursor}", self.query), width);
            spans.push(Span::styled(field, self.muted()));
        } else {
            for filter in Filter::EACH {
                // The filter in force is a filled chip, like the unread pill.
                let style = if filter == self.filter {
                    Style::default()
                        .fg(rgb(self.palette.accent_on()))
                        .bg(rgb(self.palette.accent()))
                        .add_modifier(Modifier::BOLD)
                } else {
                    self.muted()
                };
                spans.push(Span::styled(format!(" {} ", filter.name()), style));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_messages(&mut self, frame: &mut Frame, area: Rect) {
        let [rule, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        self.draw_conversation_header(frame, rule);
        // A free column on the right, as in the list.
        let inner = Rect {
            width: body.width.saturating_sub(1),
            ..body
        };
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

        // The composer is the last rows (up to five, growing upwards), under its rule.
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
                Some(message) => format!("replying to: {} ", snippet(message)),
                None => "replying ".to_owned(),
            });
        if !rule.is_empty() {
            let width = usize::from(area.width);
            let mut spans = Vec::new();
            let mut used = 0;
            // The pane with the keys draws the composer's hairline in the accent.
            let hint = if self.pane == Pane::Compose {
                Style::default().fg(rgb(self.palette.accent()))
            } else {
                self.line_style()
            };
            if let Some(quoted) = quoted {
                spans.push(Span::styled("─ ", self.line_style()));
                used += 2 + display_width(&quoted);
                spans.push(Span::styled(quoted, self.muted()));
            }
            // The keys under the composer are said once, in the footer: the rule only carries
            // the reply's quote.
            spans.push(Span::styled("─".repeat(width.saturating_sub(used)), hint));
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect {
                    width: area.width,
                    ..rule
                },
            );
            // The composer rule meets the line between the panes.
            if let Some(x) = area.x.checked_sub(1)
                && let Some(cell) = frame.buffer_mut().cell_mut((x, rule.y))
                && cell.symbol() == "│"
            {
                cell.set_symbol("├");
            }
        }
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
            let hint = Line::from(Span::styled("  i or Enter to write", self.muted()));
            frame.render_widget(Paragraph::new(hint), area);
            return;
        };
        // Nothing written yet: the field says what to do (the keys are in the footer).
        if self.pane == Pane::Compose && rows.len() == 1 && rows[0].is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("› ", self.muted()),
                    Span::styled(
                        "Write a message",
                        Style::default().fg(rgb(self.palette.text_faint())),
                    ),
                ])),
                area,
            );
            frame.set_cursor_position((area.x + 2, area.y));
            return;
        }
        // The rows shown: from the first, or scrolled so the cursor's row is the last one.
        let visible = usize::from(area.height);
        let top = (row + 1).saturating_sub(visible);
        let text = Style::default().fg(rgb(self.palette.text()));
        let lines: Vec<Line> = rows
            .into_iter()
            .enumerate()
            .skip(top)
            .take(visible)
            .map(|(index, shown)| {
                let prompt = if index == 0 { "› " } else { "  " };
                Line::from(vec![
                    Span::styled(prompt, self.muted()),
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
        }
        // Nothing to read is still a place: it says so and how to get out (R7).
        if self.model.selected_chat().is_none() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "No chat open · Tab switches panes",
                    self.muted(),
                )),
                area,
            );
            self.map.message_area = area;
            return;
        }
        let layout = self.conversation_layout(area.width);
        let total = layout.rows.len();
        let visible = usize::from(area.height);
        if let Some(start) = layout.selected.filter(|_| self.reveal) {
            // Scroll so the selected message's first row is on screen.
            let below = total.saturating_sub(start);
            self.scroll = self
                .scroll
                .clamp(below.saturating_sub(visible), below.saturating_sub(1));
        }
        self.reveal = false;
        let top = total.saturating_sub(visible);
        self.scroll = self.scroll.min(top);
        self.map.message_area = area;
        self.map.message_rows = if total < visible {
            let mut rows = vec![None; visible - total];
            rows.extend(layout.rows.iter().map(|row| row.owner));
            rows
        } else {
            layout.rows[top - self.scroll..]
                .iter()
                .take(visible)
                .map(|row| row.owner)
                .collect()
        };
        // The transcript row drawn on the area's first row (negative: a short one sits low).
        let first_row = if total < visible {
            -((visible - total) as i64)
        } else {
            (top - self.scroll) as i64
        };
        let first = first_row.max(0) as usize;
        let last = (first_row + visible as i64).clamp(0, total as i64) as usize;
        {
            let buffer = frame.buffer_mut();
            for (index, row) in layout.rows.iter().enumerate().take(last).skip(first) {
                let y = area.y + (index as i64 - first_row) as u16;
                // A bubble's background covers its whole block, padding included.
                if let Some(background) = row.bubble {
                    let block = Rect {
                        x: area.x + row.x.saturating_sub(1),
                        y,
                        width: row.width + 2,
                        height: 1,
                    };
                    buffer.set_style(block, Style::default().bg(background));
                }
                if let Some((ear_x, ear, colour)) = row.ear {
                    let glyph = match ear {
                        Ear::Incoming => "▐",
                        Ear::Outgoing => "▌",
                    };
                    buffer.set_stringn(area.x + ear_x, y, glyph, 1, Style::default().fg(colour));
                }
                buffer.set_line(area.x + row.x, y, &row.line, row.width);
            }
        }
        // The selected message is marked by one accent bar on the pane's left margin, over the
        // rows it owns: a fill would erase its slab's tint and its ear.
        if let Some(cursor) = self
            .model
            .transcript()
            .cursor()
            .filter(|_| self.pane != Pane::Chats)
        {
            let bar = Style::default().fg(rgb(self.palette.accent()));
            let buffer = frame.buffer_mut();
            for (y, owner) in self.map.message_rows.iter().enumerate() {
                if *owner == Some(cursor) {
                    buffer.set_stringn(area.x, area.y + y as u16, "▌", 1, bar);
                }
            }
        }
        // The scroll gutter: the free last column says whether there is more above or below.
        let gutter = area.right().saturating_sub(1);
        let muted = self.muted();
        if top > self.scroll {
            frame.buffer_mut().set_string(gutter, area.y, "▲", muted);
        }
        if self.scroll > 0 {
            frame
                .buffer_mut()
                .set_string(gutter, area.y + area.height - 1, "▼", muted);
        }
        self.draw_pictures(frame, area, &layout.boxes, first_row);
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
        first_row: i64,
    ) {
        let label_style = self.muted();
        let Some(jid) = self.model.selected_jid().map(str::to_owned) else {
            return;
        };
        // The colours a picture's transparent pixels take, per side.
        let incoming = self.palette.surface_chrome();
        let outgoing = self.palette.bubble_outgoing();
        let background = |from_me: bool| {
            let colour = if from_me { outgoing } else { incoming };
            [colour.r, colour.g, colour.b]
        };
        let Some(images) = self.images.as_mut() else {
            return;
        };
        let transcript = self.model.transcript();
        let messages = transcript.conversation(&jid).unwrap_or(&[]);
        // A full picture with no box yet (its message states no size) is decoded to learn its
        // size; one with a box is encoded only while the box is on screen (below).
        for (chat, id, path) in transcript.full_pictures() {
            let Some(message) = messages.iter().find(|m| m.id == id) else {
                continue;
            };
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
                background: background(message.from_me),
            };
            if images.wants(&key) {
                images.request(key, images::Origin::File(path.to_owned()), None);
            }
        }
        let visible = i64::from(area.height);
        // A picture that comes into view is fetched once, unasked: the thumbnail in the box is
        // WhatsApp's small copy, and only the full picture is sharp. A fetch that fails is not
        // retried by itself (the manual `o` stays available); nothing is requested twice.
        let mut wanted: Vec<String> = Vec::new();
        for picture in boxes {
            let Some(message) = messages.get(picture.index) else {
                continue;
            };
            let y = picture.line as i64 - first_row;
            if y >= visible || y + i64::from(picture.size.height) <= 0 {
                continue;
            }
            let id = message.id.clone();
            let seen = (jid.clone(), id.clone());
            if transcript.full_picture(&jid, &id).is_none()
                && !transcript.is_showing(&id)
                && !self.pictures_wanted.contains(&seen)
            {
                self.pictures_wanted.insert(seen);
                wanted.push(id);
            }
            let Ok(position_y) = i16::try_from(y) else {
                continue;
            };
            let position = SignedPosition {
                x: i16::try_from(picture.x).unwrap_or(0),
                y: position_y,
            };
            let background = background(message.from_me);
            let key = |variant| images::Key {
                chat: jid.clone(),
                id: message.id.clone(),
                variant,
                columns: area.width,
                background,
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
            // Nothing to draw inside the box yet (no thumbnail, or one that does not decode):
            // the box says what it is, at its own first row.
            if (0..visible).contains(&y) {
                let label = Rect {
                    x: area.x + picture.x,
                    y: area.y + y as u16,
                    width: picture.size.width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(format!("[{}]", plain(&message.kind))).style(label_style),
                    label,
                );
            }
        }
        // A picture message can arrive with no stated size (a sticker often does): it gets no box,
        // so its file is asked for here and the decoded size gives it one. Only the messages on
        // screen, once each, and never after a failure.
        let mut shown: Vec<usize> = self.map.message_rows.iter().flatten().copied().collect();
        shown.sort_unstable();
        shown.dedup();
        for index in shown {
            if boxes.iter().any(|picture| picture.index == index) {
                continue;
            }
            let Some(message) = messages.get(index) else {
                continue;
            };
            if !is_picture(&message.kind) || message.id.is_empty() {
                continue;
            }
            let id = message.id.clone();
            let seen = (jid.clone(), id.clone());
            if transcript.full_picture(&jid, &id).is_none()
                && !transcript.is_showing(&id)
                && !self.pictures_wanted.contains(&seen)
            {
                self.pictures_wanted.insert(seen);
                wanted.push(id);
            }
        }
        for id in wanted {
            // The model has to know, or the daemon's answer is not kept for the box.
            if self.model.show_picture(&jid, &id) {
                self.pending
                    .push(Action::Request(ClientFrame::download_media(
                        jid.clone(),
                        id,
                    )));
            }
        }
    }

    /// The background a message's bubble is drawn on: what a picture's transparent pixels take.
    fn bubble_background(&self, message: &Message) -> [u8; 3] {
        let colour = if message.from_me {
            self.palette.bubble_outgoing()
        } else {
            self.palette.surface_chrome()
        };
        [colour.r, colour.g, colour.b]
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
                    background: self.bubble_background(message),
                })
            })
    }

    /// The selected chat's conversation, oldest first, laid out `columns` wide; before it is
    /// loaded, its last message. Each message is a bubble: incoming ones start at the pane's
    /// first column, outgoing ones end at its last usable one (the free last column is the scroll
    /// gutter), and none is wider than [`BUBBLE_SHARE`] percent of the pane. The time sits inside,
    /// bottom right, in `text_faint`; nothing repeats the author, because whose a message is the
    /// side says.
    ///
    /// The rows are laid out here, one per screen row, so the scroll, the selection and the mouse
    /// use the same geometry the bubbles are drawn with (no `Paragraph` wraps them a second time).
    /// A day starts with its label in muted caps, after a blank row; every bubble after one ends
    /// after a blank row of its own.
    fn conversation_layout(&self, columns: u16) -> ConversationLayout {
        let hint = |text: &str| ConversationLayout {
            rows: vec![Row::plain(
                Line::from(Span::styled(text.to_owned(), self.muted())),
                columns,
            )],
            selected: None,
            boxes: Vec::new(),
        };
        let Some(chat) = self.model.selected_chat() else {
            let text = if self.chats().is_empty() {
                "No chats yet"
            } else {
                "No chat open · Enter opens the selected one"
            };
            return hint(text);
        };
        let loaded = self.model.transcript().conversation(&chat.jid);
        let messages: Vec<&Message> = match loaded {
            Some(messages) => messages.iter().collect(),
            None => chat.last_message.iter().collect(),
        };
        if messages.is_empty() {
            return hint("No messages yet · i writes the first one");
        }
        let body = Style::default().fg(rgb(self.palette.text()));
        let name = body.add_modifier(Modifier::BOLD);
        // A group's sender on the first message of a run: the accent, bold, never a colour of
        // its own (the palette has no per-person hues).
        let sender = Style::default()
            .fg(rgb(self.palette.accent()))
            .add_modifier(Modifier::BOLD);
        // The last column is the scroll gutter, then one margin on each side: a slab uses the
        // columns in between and its ear sits on the margin.
        let usable = columns.saturating_sub(1);
        let band = usable.saturating_sub(2);
        let share = (u32::from(band) * BUBBLE_SHARE / 100) as u16;
        let max_width = share.max(BUBBLE_MIN.min(band)).min(band);
        let inner_max = usize::from(max_width.saturating_sub(3));
        // The text keeps the share; the time and the state may take a slab up to the band.
        let ceiling = usize::from(band).saturating_sub(3).max(3);
        let incoming = rgb(self.palette.surface_chrome());
        let outgoing = rgb(self.palette.bubble_outgoing());
        let ear_incoming = rgb(self.palette.bubble_ear_incoming());
        let ear_outgoing = rgb(self.palette.bubble_ear_outgoing());

        // What a message needs, before the run it belongs to fixes the width they all share.
        struct Prepared {
            index: usize,
            chosen: bool,
            who: String,
            day: Option<time::Date>,
            content: Vec<(String, Part)>,
            quote: Option<(Option<String>, String)>,
            picture_at: usize,
            footer: Vec<Span<'static>>,
            footer_width: usize,
            inline: bool,
            needs: usize,
            picture: Option<ratatui::layout::Size>,
            reactions: Vec<Span<'static>>,
        }
        let mut prepared: Vec<Prepared> = Vec::with_capacity(messages.len());
        for (index, message) in messages.iter().enumerate() {
            let chosen = loaded.is_some() && self.model.transcript().cursor() == Some(index);
            let who = author(Some(chat), message);
            let day = local_day(message, self.utc_offset);
            // A run starts with a new author or a new day.
            let first_of_run = prepared
                .last()
                .is_none_or(|before: &Prepared| before.who != who || before.day != day);
            // The picture, when the terminal draws them and the message states a size (or its
            // full picture gave one): it sits inside the slab, above the caption, and the
            // `[kind]` label is then not written at all.
            let box_size = loaded
                .is_some()
                .then(|| self.picture_box(&chat.jid, message, inner_max as u16))
                .flatten();
            // An animated sticker shows its first frame; the marker says it moves. With the
            // picture drawn it is the caption's own marker; without one the `[kind]` label
            // carries it.
            let marker = message.kind == "sticker" && message.animated;
            let text = if message.kind == "text" {
                message.text.clone()
            } else if box_size.is_some() {
                if marker && message.text.is_empty() {
                    "▶".to_owned()
                } else if marker {
                    format!("▶ {}", message.text)
                } else {
                    message.text.clone()
                }
            } else {
                format!(
                    "[{}]{} {}",
                    message.kind,
                    if marker { " ▶" } else { "" },
                    message.text
                )
            };
            // A caption being fetched says so on its last row, never wrapping it to more rows
            // (the picture above never moves).
            let text = if message.kind != "text" && self.fetching(&chat.jid, message, columns) {
                downloading(&text, inner_max as u16)
            } else {
                text
            };
            // The slab's rows: the sender's name on the first message of a run in a group, the
            // quote where it replies to one, then the picture and the caption.
            let mut content: Vec<(String, Part)> = Vec::new();
            if first_of_run
                && !message.from_me
                && is_group(&chat.jid)
                && let Some(sender) = message.sender.as_deref().filter(|s| !s.is_empty())
            {
                content.extend(
                    wrap_text(&first_name(&plain(sender)), inner_max)
                        .into_iter()
                        .map(|row| (row, Part::Name)),
                );
            }
            // The quote is one inset row: the start of what was said, and who said it when that
            // is not obvious (a group, or one of mine) — a 1:1 message needs no name.
            let quote = message.reply_to.as_ref().map(|quoted_id| {
                match quoted(messages.iter().copied(), quoted_id) {
                    Some(original) => (
                        (original.from_me || (is_group(&chat.jid) && original.sender.is_some()))
                            .then(|| author(Some(chat), original)),
                        snippet(original),
                    ),
                    None => (None, "(a message not loaded)".to_owned()),
                }
            });
            // Where the picture goes: after the name and the quote, before the caption.
            let picture_at = content.len();
            // A caption that is only the picture's own kind has nothing left to say once the
            // picture is there; a fallback label stays caption enough.
            let footer = self.bubble_footer(message, chosen);
            let footer_width: usize = footer.iter().map(Span::width).sum();
            let mut caption: Vec<(String, Part)> = if box_size.is_some() && text.trim().is_empty() {
                Vec::new()
            } else {
                text.lines()
                    .flat_map(|line| wrap_text(&plain(line), inner_max))
                    .map(|row| (row, Part::Text))
                    .collect()
            };
            // The time keeps a slot at the right of the last row. A last row with no room for it
            // gives its last word a row of its own, so no word is ever cut and the time still sits
            // inside the slab; a word too long for that leaves the time on a row of its own.
            let mut inline = !caption.is_empty();
            for _ in 0..4 {
                if !inline {
                    break;
                }
                let Some((last, part)) = caption.last() else {
                    inline = false;
                    break;
                };
                let width = display_width(last);
                if width + 1 + footer_width <= ceiling {
                    break;
                }
                let Some((head, tail)) = last.rsplit_once(' ') else {
                    inline = false;
                    break;
                };
                if head.is_empty() || display_width(tail) + 1 + footer_width > ceiling {
                    inline = false;
                    break;
                }
                let (head, tail, part) = (head.to_owned(), tail.to_owned(), *part);
                *caption.last_mut().expect("a last row") = (head, part);
                caption.push((tail, part));
            }
            content.extend(caption);
            let caption_width = content
                .iter()
                .map(|(row, _)| display_width(row))
                .max()
                .unwrap_or(0);
            let last_width = content
                .iter()
                .rev()
                .find(|(_, part)| *part == Part::Text)
                .map_or(0, |(row, _)| display_width(row));
            let box_width = box_size.map_or(0, |size| usize::from(size.width));
            // The quote's inset row is part of the slab too: it may be the longest line.
            let quote_width = quote.as_ref().map_or(0, |(who, said)| {
                2 + who.as_ref().map_or(0, |who| display_width(who) + 2) + display_width(said)
            });
            let needs = if inline {
                caption_width.max(last_width + 1 + footer_width)
            } else {
                caption_width.max(footer_width)
            }
            .max(box_width)
            .max(quote_width);
            let reactions = message
                .reactions
                .iter()
                .map(|reaction| {
                    let style = if reaction.from_me { name } else { body };
                    // A chip: the hover tone behind the emoji, under the slab.
                    Span::styled(
                        format!("{} ", plain(&reaction.emoji)),
                        style.bg(rgb(self.palette.surface_hover())),
                    )
                })
                .collect();
            prepared.push(Prepared {
                index,
                chosen,
                who,
                day,
                content,
                quote,
                picture_at,
                footer,
                footer_width,
                inline,
                needs,
                picture: box_size,
                reactions,
            });
        }

        let mut rows = Vec::with_capacity(messages.len() * 3 + 1);
        // The top of the conversation is a place too: it says so when it is on screen, which
        // is where a short transcript leaves the eye with nothing to hold on to (R7).
        rows.push(Row::plain(
            Line::from(Span::styled("Beginning of conversation", self.muted())),
            columns,
        ));
        let mut selected = None;
        let mut boxes = Vec::new();
        // The day whose separator was drawn last, and whether a run was drawn already.
        let mut labelled: Option<time::Date> = None;
        let mut started = false;
        let mut start = 0;
        while start < prepared.len() {
            // A run: the messages of one author inside one day. They share one width.
            let mut end = start + 1;
            while end < prepared.len()
                && prepared[end].who == prepared[start].who
                && prepared[end].day == prepared[start].day
            {
                end += 1;
            }
            let run = &prepared[start..end];
            // A run shares one width. The time and state may take a slab past the share it keeps
            // for its text (never past the band), so a state word is never cut in half.
            let inner = run
                .iter()
                .map(|message| message.needs)
                .max()
                .unwrap_or(0)
                .max(
                    run.iter()
                        .map(|message| message.footer_width)
                        .max()
                        .unwrap_or(0),
                )
                .clamp(3, ceiling);
            let room = u16::try_from(inner).unwrap_or(0);
            // A new day gets a centred separator, after a blank row; the next author after a
            // blank row; one author's run follows itself with no gap at all.
            match run[0].day.filter(|day| labelled != Some(*day)) {
                Some(day) => {
                    if started {
                        rows.push(Row::plain(Line::default(), columns));
                    }
                    rows.push(Row::centred(
                        Line::from(Span::styled(day_label(day, self.today), self.muted())),
                        columns,
                    ));
                    labelled = Some(day);
                }
                None if started => rows.push(Row::plain(Line::default(), columns)),
                None => {}
            }
            started = true;
            for message in run {
                let source = messages[message.index];
                let owner = loaded.is_some().then_some(message.index);
                if message.chosen {
                    selected = Some(rows.len());
                }
                let background = if source.from_me { outgoing } else { incoming };
                let tone = if source.from_me {
                    self.palette.bubble_outgoing()
                } else {
                    self.palette.surface_chrome()
                };
                let (ear_side, ear_colour) = if source.from_me {
                    (Ear::Outgoing, ear_outgoing)
                } else {
                    (Ear::Incoming, ear_incoming)
                };
                // The ear sits on the margin (the band's left for theirs, its right for ours) and
                // runs the whole height of the message; the slab starts one cell inside it.
                let ear_x = if source.from_me { band } else { 1 };
                let text_x = if source.from_me {
                    band.saturating_sub(room).saturating_sub(1)
                } else {
                    3
                };
                let line = |text: &str, part: Part| {
                    Line::from(match part {
                        Part::Name => vec![Span::styled(text.to_owned(), sender)],
                        Part::Text => vec![Span::styled(text.to_owned(), body)],
                    })
                };
                // Each row with its own background: the slab's tone, but the quote's inset.
                let mut lines: Vec<(Line<'static>, Color)> = Vec::new();
                for (text, part) in &message.content[..message.picture_at] {
                    lines.push((line(text, *part), background));
                }
                // The quote: one inset row, a bar and the quoted author in the accent, then the
                // start of what they said in the muted role, clamped with `…`.
                if let Some((who, said)) = &message.quote {
                    let accent = Style::default().fg(rgb(self.palette.accent()));
                    let room_left = usize::from(room).saturating_sub(2);
                    // A name never takes more than half the row: the quoted text matters more.
                    let head = who
                        .as_ref()
                        .map(|who| fit(&format!("{who}:"), room_left / 2))
                        .unwrap_or_default();
                    let rest = room_left
                        .saturating_sub(display_width(&head) + usize::from(!head.is_empty()));
                    let body = fit(said, rest);
                    let mut spans = vec![Span::styled("▌ ", accent)];
                    if !head.is_empty() {
                        spans.push(Span::styled(head, accent.add_modifier(Modifier::BOLD)));
                        spans.push(Span::raw(" "));
                    }
                    spans.push(Span::styled(body, self.muted()));
                    lines.push((Line::from(spans), rgb(self.palette.quote_inset(tone))));
                }
                // The picture's own rows, ready for the image to be drawn over them.
                let picture_line = rows.len() + lines.len();
                if let Some(size) = message.picture {
                    lines.extend((0..size.height).map(|_| (Line::default(), background)));
                }
                let last = message.content.len().saturating_sub(1);
                for (index, (text, part)) in
                    message.content.iter().enumerate().skip(message.picture_at)
                {
                    let mut spans = line(text, *part).spans;
                    if message.inline && index == last {
                        let pad = usize::from(room)
                            .saturating_sub(display_width(text) + message.footer_width);
                        spans.push(Span::raw(" ".repeat(pad)));
                        spans.extend(message.footer.iter().cloned());
                    }
                    lines.push((Line::from(spans), background));
                }
                if !message.inline {
                    let pad = usize::from(room).saturating_sub(message.footer_width);
                    let mut spans = vec![Span::raw(" ".repeat(pad))];
                    spans.extend(message.footer.iter().cloned());
                    lines.push((Line::from(spans), background));
                }
                for (line, tint) in lines {
                    rows.push(Row {
                        line,
                        x: text_x,
                        width: room,
                        bubble: Some(tint),
                        ear: Some((ear_x, ear_side, ear_colour)),
                        owner,
                    });
                }
                if let Some(size) = message.picture {
                    boxes.push(PictureBox {
                        line: picture_line,
                        size,
                        index: message.index,
                        x: text_x,
                    });
                }
                // Reactions sit under the slab, as a chip row of their own.
                if !message.reactions.is_empty() {
                    let width: usize = message.reactions.iter().map(Span::width).sum();
                    let pad = usize::from(room).saturating_sub(width);
                    let mut spans = Vec::new();
                    if source.from_me {
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                    spans.extend(message.reactions.iter().cloned());
                    if !source.from_me {
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                    rows.push(Row {
                        line: Line::from(spans),
                        x: text_x,
                        width: room,
                        bubble: None,
                        ear: None,
                        owner,
                    });
                }
            }
            start = end;
        }
        ConversationLayout {
            rows,
            selected,
            boxes,
        }
    }

    /// The time inside a bubble, in the faint role, carrying the state where the ticks would go:
    /// a small clock while the send is queued and a `danger` `!` when it was not sent. Delivery
    /// and read receipts are not in the protocol, so they are never invented.
    fn bubble_footer(&self, message: &Message, chosen: bool) -> Vec<Span<'static>> {
        // The open message's time steps up from the faint role, so its row reads as selected.
        let faint = if chosen {
            Style::default().fg(rgb(self.palette.text()))
        } else {
            Style::default().fg(rgb(self.palette.text_faint()))
        };
        let unconfirmed = self.model.transcript().is_unconfirmed(message);
        let mut meta = meta_of(message, self.utc_offset, unconfirmed);
        if self.model.transcript().is_sending(message) {
            meta.push_str(" · sending");
        }
        let mut spans = vec![Span::styled(meta, faint)];
        match message.state {
            Some(MessageState::Queued) => spans.push(Span::styled(format!(" {CLOCK}"), faint)),
            Some(MessageState::NotSent) if !unconfirmed => spans.push(Span::styled(
                format!(" {NOT_SENT}"),
                Style::default().fg(rgb(self.palette.danger())),
            )),
            _ => {}
        }
        spans
    }

    /// Meta text: the `text_muted` role, the only tone with a 4.5:1 floor on both surfaces.
    /// `Modifier::DIM` is not used for text: the terminal decides how much it dims, so the
    /// contrast could not be held by a test (design plan §9.3, R3).
    fn muted(&self) -> Style {
        Style::default().fg(rgb(self.palette.text_muted()))
    }

    /// The one-cell lines: the header rule, the line between the panes, the composer rule.
    fn line_style(&self) -> Style {
        Style::default().fg(rgb(self.palette.border()))
    }
}

/// The transcript as laid out: one [`Row`] per screen row, the selected message's first row, and
/// the picture boxes.
struct ConversationLayout {
    rows: Vec<Row>,
    selected: Option<usize>,
    boxes: Vec<PictureBox>,
}

/// One row of the transcript: what it draws, where its bubble sits, and whose message it is.
struct Row {
    line: Line<'static>,
    /// The text's first column, counted from the pane's left edge.
    x: u16,
    /// The columns the text may use; the slab is one column wider on each side, and its ear sits
    /// one column further, on the margin.
    width: u16,
    /// The background of the slab this row is part of.
    bubble: Option<Color>,
    /// The slab's ear on this row: its column, side and tone. Full height, every row.
    ear: Option<(u16, Ear, Color)>,
    /// The loaded message the row belongs to (the selection and the mouse).
    owner: Option<usize>,
}

/// Which side a slab's ear sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ear {
    Incoming,
    Outgoing,
}

impl Row {
    /// A row of plain transcript: no bubble, the pane's whole width.
    fn plain(line: Line<'static>, columns: u16) -> Self {
        Self {
            line,
            x: 0,
            width: columns,
            bubble: None,
            ear: None,
            owner: None,
        }
    }

    /// A row of plain transcript centred in the pane: the day separators.
    fn centred(line: Line<'static>, columns: u16) -> Self {
        let width = u16::try_from(line.width()).unwrap_or(columns).min(columns);
        Self {
            x: columns.saturating_sub(width) / 2,
            width,
            ..Self::plain(line, columns)
        }
    }
}

/// What a row inside a slab is: a group sender's name, or the message's own text. The quote has
/// a row of its own (an inset) and is not a [`Part`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Name,
    Text,
}

/// A picture's box in the transcript: its first (blank) line, its size in cells, and the index of
/// its loaded message.
struct PictureBox {
    /// The first of its rows, inside its bubble.
    line: usize,
    size: ratatui::layout::Size,
    index: usize,
    /// Its first column, from the pane's left edge.
    x: u16,
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

/// `text` broken into rows of at most `width` terminal columns: at the last space that fits,
/// or between graphemes when a single word is wider than the row. Every row keeps a column.
///
/// This is the transcript's own wrapping (bubbles are laid out by hand, not by a `Paragraph`),
/// by the same widths ratatui draws with, so the geometry the scroll and the mouse use is the
/// geometry on screen.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        // Where the current row may break: the byte and column just after its last space.
        let mut space: Option<(usize, usize)> = None;
        for grapheme in line.graphemes(true) {
            let w = display_width(grapheme);
            if used > 0 && used + w > width {
                match space.take().filter(|_| !grapheme.trim().is_empty()) {
                    // Carry the word being written over to the next row.
                    Some((byte, column)) => {
                        let rest = row.split_off(byte);
                        rows.push(std::mem::replace(&mut row, rest));
                        used -= column;
                    }
                    None => {
                        rows.push(std::mem::take(&mut row));
                        used = 0;
                    }
                }
            }
            row.push_str(grapheme);
            used += w;
            if grapheme.trim().is_empty() {
                space = Some((row.len(), used));
            }
        }
        rows.push(row);
    }
    rows
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

/// The footer's parts joined with ` · `, each part one or more spans.
fn footer_spans(parts: &[Vec<(String, Style)>], gap: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", gap));
        }
        for (text, style) in part {
            spans.push(Span::styled(text.clone(), *style));
        }
    }
    spans
}

fn line_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum::<usize>()
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

/// The unread count as the list's pill: ` 3 `, ` 12 ` or ` 99+ ` (it always fills [`PILL`]), or
/// nothing when there is nothing unread.
fn unread_pill(unread: u32) -> String {
    match unread {
        0 => String::new(),
        n if n > 99 => " 99+ ".to_owned(),
        n => format!(" {n} "),
    }
}

/// How many tints an avatar takes: one per palette hue the theme names.
const AVATAR_TINTS: u8 = 4;
/// The columns a chat's avatar takes at the left of its two content rows: one blank, its three
/// cells, one blank.
const AVATAR: usize = 5;

/// A stable seed for a chat's avatar: the same title always gets the same tint.
fn avatar_seed(title: &str) -> u8 {
    title
        .bytes()
        .fold(0u8, |acc, byte| acc.wrapping_mul(31).wrapping_add(byte))
}

/// The two initials an avatar carries, one per row: the first letter of each of the first two
/// words (its first two letters when the title is one word), uppercased when they are Latin.
fn avatar_initials(title: &str) -> [char; 2] {
    let upper = |c: char| {
        if c.is_ascii_lowercase() {
            c.to_ascii_uppercase()
        } else {
            c
        }
    };
    // Only a narrow character: a wide one (a CJK ideograph) would push the columns after it.
    let mut words = title.split_whitespace().filter_map(|word| {
        word.chars()
            .find(|c| c.is_alphanumeric() && char_width(*c) == 1)
    });
    let first = upper(words.next().unwrap_or('?'));
    let second = match words.next() {
        Some(letter) => upper(letter),
        // One word: its second letter, when it has one.
        None => upper(
            title
                .chars()
                .filter(|c| c.is_alphanumeric() && char_width(*c) == 1)
                .nth(1)
                .unwrap_or(' '),
        ),
    };
    [first, second]
}

/// The six cells of a chat's avatar: a 3×2 rounded block whose corners are the tint on whatever is
/// behind them (no background of their own) and whose middle cells are the tint with one initial
/// on each.
fn avatar_cells(title: &str, palette: &Palette) -> [(char, Style); 6] {
    let seed = avatar_seed(title) % AVATAR_TINTS;
    let tint = rgb(palette.avatar_tint(seed));
    let ink = rgb(palette.avatar_ink(seed));
    let corner = |glyph: char| (glyph, Style::default().fg(tint));
    let letter = |glyph: char| (glyph, Style::default().fg(ink).bg(tint));
    let [first, second] = avatar_initials(title);
    [
        corner('▗'),
        letter(first),
        corner('▖'),
        corner('▝'),
        letter(second),
        corner('▘'),
    ]
}

/// One row (0 or 1) of a chat's avatar as spans: a blank column, its three cells, a blank column.
fn avatar_spans(title: &str, palette: &Palette, row: usize) -> Vec<Span<'static>> {
    let cells = avatar_cells(title, palette);
    let mut spans = vec![Span::raw(" ")];
    for (glyph, style) in &cells[row * 3..row * 3 + 3] {
        spans.push(Span::styled(glyph.to_string(), *style));
    }
    spans.push(Span::raw(" "));
    spans
}

/// Draw a chat's avatar over `(x, y)`, two rows tall: what the open chat's row fill would
/// otherwise erase.
fn paint_avatar(buffer: &mut Buffer, rows: Rect, title: &str, palette: &Palette) {
    let (x, y) = (rows.x + 1, rows.y);
    for (index, (glyph, style)) in avatar_cells(title, palette).iter().enumerate() {
        let column = x + u16::try_from(index % 3).unwrap_or(0);
        let row = y + u16::try_from(index / 3).unwrap_or(0);
        // A list cut by a very small terminal would put the cells outside its own rectangle.
        if column < rows.right() && row < rows.bottom() {
            buffer.set_stringn(column, row, glyph.to_string(), 1, *style);
        }
    }
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

fn rgb(colour: Rgb) -> Color {
    Color::Rgb(colour.r, colour.g, colour.b)
}

/// Whether a chat is a group: the wire's JID domain says so.
fn is_group(jid: &str) -> bool {
    jid.ends_with("@g.us")
}

/// The first word of a name: what a group's sender line and a quote's prefix carry, so a long
/// full name never eats a slab's width.
fn first_name(name: &str) -> String {
    name.split_whitespace().next().unwrap_or(name).to_owned()
}

/// Who sent the message: us, the group member the daemon names in `sender` (first name only),
/// or the chat.
fn author(chat: Option<&Chat>, message: &Message) -> String {
    let sender = message.sender.as_deref().filter(|s| !s.is_empty());
    match (message.from_me, sender, chat) {
        (true, _, _) => "you".to_owned(),
        (false, Some(sender), _) => first_name(&plain(sender)),
        (false, None, Some(chat)) => plain(&chat_title(chat)),
        (false, None, None) => "them".to_owned(),
    }
}

/// When the message was sent; a send of ours that is not sent yet says so, so a failure stays
/// visible, and one that was not confirmed (`unconfirmed`) says that instead, since it may have
/// been delivered.
fn meta_of(message: &Message, offset: time::UtcOffset, unconfirmed: bool) -> String {
    let state = match message.state {
        Some(MessageState::Queued) => " · queued",
        Some(MessageState::NotSent) if unconfirmed => " · not confirmed",
        Some(MessageState::NotSent) => " · not sent",
        Some(MessageState::Unknown) | None => "",
    };
    format!("{}{state}", local_hhmm(message, offset))
}

/// [`author`] and [`meta_of`]: `Ada · 17:55 · not sent`.
fn sent_by(
    chat: Option<&Chat>,
    message: &Message,
    offset: time::UtcOffset,
    unconfirmed: bool,
) -> String {
    format!(
        "{} · {}",
        author(chat, message),
        meta_of(message, offset, unconfirmed)
    )
}

/// The local day the message was sent on, at `offset` from UTC; `None` when its timestamp is
/// not RFC 3339.
fn local_day(message: &Message, offset: time::UtcOffset) -> Option<time::Date> {
    let (date, clock) = message.timestamp.split_once('T')?;
    let mut parts = date.splitn(3, '-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    let hours: u8 = clock.get(..2)?.parse().ok()?;
    let minutes: u8 = clock.get(3..5)?.parse().ok()?;
    let date =
        time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    let utc = time::PrimitiveDateTime::new(date, time::Time::from_hms(hours, minutes, 0).ok()?)
        .assume_utc();
    Some(utc.checked_to_offset(offset)?.date())
}

/// What a chat is, as the conversation header says it: its masked number when the daemon knows
/// one, then `contact` or `group` (from the JID's domain).
fn chat_subtitle(chat: &Chat) -> Option<String> {
    let kind = if is_group(&chat.jid) {
        "group"
    } else {
        "contact"
    };
    Some(
        match chat.phone.as_deref().filter(|phone| !phone.is_empty()) {
            Some(phone) => format!("{phone} · {kind}"),
            None => kind.to_owned(),
        },
    )
}

/// A day separator's label: `Today`, `Yesterday`, or the date in caps (`SUN 18 OCT 2026`).
fn day_label(day: time::Date, today: time::Date) -> String {
    if day == today {
        return "Today".to_owned();
    }
    if today.previous_day() == Some(day) {
        return "Yesterday".to_owned();
    }
    let short = |name: String| name.chars().take(3).collect::<String>();
    format!(
        "{} {} {} {}",
        short(day.weekday().to_string()),
        day.day(),
        short(day.month().to_string()),
        day.year()
    )
    .to_uppercase()
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

/// The fewest columns a chat's name keeps before the row gives up its time and markers.
const NARROW_NAME: usize = 6;
/// The unread pill's fixed slot on a chat row's second line: ` 3 `, ` 12 `, ` 99+ `.
const PILL: usize = 5;

/// A chat in the list, in three rows. Row one: the avatar (a 3×2 rounded block with its initials),
/// the name, the markers the chat has right after it, and the time in its own five-column slot at
/// the right edge. Row two: the avatar's second row, the last message cut with `…`, then the unread
/// count as an accent pill at the right. Row three: a thin rule in the border role, the separator. A chat with unread messages has its name and
/// time in bold, and the highlighted chat is filled across every row (`surface_active`), with the
/// keys on it or not, so the list always says which chat is open. A narrow list gives up the time
/// first, then the markers, and only then cuts the name below six columns.
fn chat_item(
    chat: &Chat,
    palette: &Palette,
    width: usize,
    offset: time::UtcOffset,
    selected: bool,
) -> ListItem<'static> {
    let text = Style::default().fg(rgb(palette.text()));
    let unread = chat.unread > 0;
    let ink = if unread {
        text.add_modifier(Modifier::BOLD)
    } else {
        text
    };
    let muted = Style::default().fg(rgb(palette.text_muted()));
    let pill_style = Style::default()
        .fg(rgb(palette.accent_on()))
        .bg(rgb(palette.accent()));
    let pill = unread_pill(chat.unread);
    // The markers follow the name, only the ones the chat has.
    let markers: String = [
        (chat.pinned, PINNED),
        (chat.muted, MUTED),
        (chat.archived, ARCHIVED),
    ]
    .iter()
    .filter_map(|&(on, marker)| on.then_some(marker))
    .collect();
    let markers = if markers.is_empty() {
        markers
    } else {
        format!(" {markers}")
    };
    // The time is its own column at the right edge of the first row.
    let time = chat
        .last_message
        .as_ref()
        .map_or_else(String::new, |message| local_hhmm(message, offset));
    // A narrow list gives up the time first, then the markers, before it cuts the name to
    // nothing: something of the chat's name has to stay readable.
    let mut time = time;
    let mut markers = markers;
    let mut time_width = display_width(&time);
    let mut named = width.saturating_sub(AVATAR + display_width(&markers) + 1 + time_width);
    if named < NARROW_NAME {
        time.clear();
        time_width = 0;
        named = width.saturating_sub(AVATAR + display_width(&markers));
    }
    if named < NARROW_NAME {
        markers.clear();
        named = width.saturating_sub(AVATAR);
    }
    let title_text = plain(&chat_title(chat));
    let name = fit(&title_text, named);
    let mut title = avatar_spans(&title_text, palette, 0);
    title.push(Span::styled(name.clone(), ink));
    if !markers.is_empty() {
        title.push(Span::styled(markers.clone(), muted));
    }
    let used = AVATAR + display_width(&name) + display_width(&markers);
    // The time is right-aligned in the row's last columns; the gap before it is the padding.
    let room = width.saturating_sub(used);
    if time_width > room {
        time.clear();
        time_width = 0;
    }
    title.push(Span::raw(" ".repeat(room.saturating_sub(time_width))));
    if time_width > 0 {
        title.push(Span::styled(time, ink));
    }

    // A group's preview says who wrote the last message (`Dana: deck ready`); a one-to-one chat
    // says it bare, because the chat is the person, and our own message needs no name either.
    let who = chat
        .last_message
        .as_ref()
        .filter(|message| !message.from_me && is_group(&chat.jid))
        .and_then(|message| message.sender.as_deref())
        .filter(|sender| !sender.is_empty())
        .map(|sender| first_name(&plain(sender)));
    // A message with no text (a non-text kind) shows its kind instead of an empty row.
    let detail = match chat.last_message.as_ref() {
        Some(message) => {
            let text = message.text.lines().next().unwrap_or_default();
            let said = plain(if text.is_empty() { &message.kind } else { text });
            match &who {
                Some(who) => format!("{who}: {said}"),
                None => said,
            }
        }
        None => "(no messages)".to_owned(),
    };
    let detail = fit(&detail, width.saturating_sub(AVATAR + PILL));
    let pad = width.saturating_sub(AVATAR + display_width(&detail) + PILL);
    let mut second = avatar_spans(&title_text, palette, 1);
    second.push(Span::styled(detail, muted));
    second.push(Span::raw(" ".repeat(pad)));
    if pill.is_empty() {
        second.push(Span::raw(" ".repeat(PILL)));
    } else {
        second.push(Span::raw(
            " ".repeat(PILL.saturating_sub(display_width(&pill))),
        ));
        second.push(Span::styled(pill, pill_style));
    }

    let rule = Line::from(Span::styled(
        "─".repeat(width + 3),
        Style::default().fg(rgb(palette.border())),
    ));
    let item = ListItem::new(vec![Line::from(title), Line::from(second), rule]);
    // The fill goes under the rows, so the pill keeps its own background.
    if selected {
        item.style(Style::default().bg(rgb(palette.surface_active())))
    } else {
        item
    }
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
            chat_indices: vec![0, 1, 2],
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
        // Each chat takes three rows (name, last message, its rule), counted from the list's
        // scroll.
        assert_eq!(hit_test(&map, 5, 2), Some(Target::Chats(Some(1))));
        assert_eq!(hit_test(&map, 30, 3), Some(Target::Chats(Some(1))));
        assert_eq!(hit_test(&map, 5, 4), Some(Target::Chats(Some(1))));
        assert_eq!(hit_test(&map, 5, 5), Some(Target::Chats(Some(2))));
        // Below the last chat, and on the border: the pane, no chat.
        assert_eq!(hit_test(&map, 5, 8), Some(Target::Chats(None)));
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
