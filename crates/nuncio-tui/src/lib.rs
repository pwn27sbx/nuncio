//! `nuncio-tui`'s model and rendering, as a library so it can be tested against a
//! `TestBackend` buffer without a terminal or a daemon.
//!
//! [`App`] is the pure state: the chat list (archived chats sectioned last), the loaded
//! conversations, the composer, the selection and scroll, and the Omarchy [`Palette`]. It changes
//! only through [`App::handle_key`] and [`App::apply`] (a reply or an event frame), which return
//! an [`Action`] for the I/O loop. [`Daemon`] is that I/O: a request connection and a subscribed
//! event connection to `nunciod`. The keys are listed in [`KEYS`] and in `README.md`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use nuncio_client::wire::MessageState;
use nuncio_client::{
    Chat, ClientFrame, DaemonFrame, Message, Palette, Reaction, Rgb, SendState, Snapshot,
};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use ratatui_image::protocol::StatefulProtocol;
use unicode_segmentation::UnicodeSegmentation;

mod daemon;
mod reconnect;

pub use daemon::{Daemon, open_externally};
pub use reconnect::{Backoff, Reconnect};

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

/// Marks a pinned chat.
const PINNED: char = '★';
/// Marks a muted chat.
const MUTED: char = '⊘';
/// Marks an archived chat.
const ARCHIVED: char = '▤';

/// How many messages a conversation loads (the wire maximum).
const MESSAGE_LIMIT: u16 = 200;
/// How many chats a resync asks for (the wire contract allows 1..200).
const CHAT_LIMIT: u16 = 200;
/// How many lines PageUp/PageDown scroll.
const PAGE: usize = 10;
/// How many results an in-chat search asks for.
const SEARCH_LIMIT: u16 = 50;
/// The reactions the picker offers, on keys `1`..`6`; `x` removes mine.
pub const REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];
/// How many characters of a quoted message are shown.
const SNIPPET: usize = 40;
/// How many rows the composer grows to; a longer draft scrolls with the cursor.
const COMPOSER_ROWS: usize = 5;

/// Every key, by pane; drawn by the `?` help screen and documented in the crate README.
pub const KEYS: &[(&str, &str)] = &[
    ("j/k ↑/↓", "chats: move · conversation: scroll a line"),
    (
        "Enter l →",
        "open the chat, marking it read on WhatsApp (blue ticks)",
    ),
    ("i", "write a message to the selected chat"),
    ("Enter", "in the composer: send"),
    ("Alt-Enter C-j", "in the composer: a new line"),
    (
        "←/→ Home/End",
        "in the composer: move the cursor · Bksp/Delete erase",
    ),
    ("K/J", "conversation: select an older / newer message"),
    ("r", "reply to the selected sent or received message"),
    (
        "e",
        "react to it: 1-6 pick an emoji, x remove mine, Esc cancel",
    ),
    ("R", "retry the selected not sent or not confirmed send"),
    ("o", "download the selected media and open it externally"),
    (
        "/",
        "search this chat; Enter search, j/k move, Enter select",
    ),
    (
        "Esc",
        "unselect · composer → conversation → chats; quit from the chats",
    ),
    ("PgUp/PgDn", "scroll a page"),
    ("g/G Home/End", "scroll to the oldest / newest message"),
    ("Tab", "switch between chats and conversation"),
    ("?", "show or hide this help"),
    ("q Ctrl-C", "quit"),
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

/// The terminal client's state: everything the screen needs, changed only by
/// [`App::handle_key`] and [`App::apply`], so it is testable without a terminal or a daemon.
pub struct App {
    snapshot: Snapshot,
    source: Source,
    palette: Palette,
    selected: usize,
    pane: Pane,
    list: ListState,
    image: Option<StatefulProtocol>,
    /// Loaded conversations by JID, oldest first.
    conversations: HashMap<String, Vec<Message>>,
    /// Lines scrolled up from the newest message; clamped when drawn.
    scroll: usize,
    composer: String,
    /// The composer's cursor: a byte offset into `composer`, on a grapheme boundary.
    caret: usize,
    /// `client_id`s of sends that have no reply yet.
    sending: HashSet<String>,
    client_prefix: String,
    sends: u64,
    help: bool,
    notice: Option<String>,
    /// The selected message: an index into the open chat's loaded conversation.
    cursor: Option<usize>,
    /// Scroll the selected message into view on the next draw.
    reveal: bool,
    /// The id of the message the composer's text will quote.
    reply_to: Option<String>,
    /// The reaction picker is open on the selected message.
    picking: bool,
    search: Option<Search>,
    /// Ids of media messages whose download was asked for with `o` and not answered yet.
    downloading: HashSet<String>,
    /// The chat opened with Enter whose `messages` reply has not come yet.
    opening: Option<String>,
    /// JIDs of chats with a `mark_read` sent and not answered yet (at most one per chat).
    marking: HashSet<String>,
    /// `client_id`s of my sends whose acknowledgement timed out (`not_confirmed`): they may have
    /// been delivered, so they are shown as not confirmed rather than not sent.
    unconfirmed: HashSet<String>,
    utc_offset: time::UtcOffset,
    /// The driver's daemon connection is up (it is `false` while the driver reconnects).
    connected: bool,
    /// The chat whose `messages` were asked for again after a reconnect: its reply keeps the
    /// selected message and the sends the daemon never stored (as one not sent while
    /// disconnected).
    resyncing: Option<String>,
}

/// The in-chat search overlay.
#[derive(Default)]
struct Search {
    query: String,
    /// The query is being typed (otherwise j/k move through the results).
    editing: bool,
    /// A `search` was sent; the next `messages` reply for the chat is its results.
    awaiting: bool,
    /// Newest first, as the daemon answers.
    results: Option<Vec<Message>>,
    list: ListState,
}

impl App {
    /// The chat list is shown with archived chats last (kept in the daemon's order otherwise).
    pub fn new(snapshot: Snapshot, source: Source, palette: Palette) -> Self {
        let mut app = Self {
            snapshot,
            source,
            palette,
            selected: 0,
            pane: Pane::Chats,
            list: ListState::default(),
            image: None,
            conversations: HashMap::new(),
            scroll: 0,
            composer: String::new(),
            caret: 0,
            sending: HashSet::new(),
            client_prefix: "tui".to_owned(),
            sends: 0,
            help: false,
            notice: None,
            cursor: None,
            reveal: false,
            reply_to: None,
            picking: false,
            search: None,
            downloading: HashSet::new(),
            opening: None,
            marking: HashSet::new(),
            unconfirmed: HashSet::new(),
            utc_offset: time::UtcOffset::UTC,
            connected: true,
            resyncing: None,
        };
        let chats = std::mem::take(&mut app.snapshot.chats);
        app.set_chats(chats);
        app
    }

    /// Attach an inline image (the `--image` debug flag) to show beside the chat list.
    pub fn with_image(mut self, image: StatefulProtocol) -> Self {
        self.image = Some(image);
        self
    }

    /// The prefix of the `client_id`s this app chooses: `<prefix>-1`, `<prefix>-2`, ...
    /// It must differ between runs, so a new send never repeats an old `client_id`.
    pub fn with_client_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.client_prefix = prefix.into();
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
        self.notice = Some(notice.into());
    }

    pub fn chats(&self) -> &[Chat] {
        &self.snapshot.chats
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// Whether the driver is connected to the daemon (`false` while it reconnects).
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// The driver lost (`false`) or regained (`true`) its daemon connection. While it is lost
    /// the header says `reconnecting…` instead of the session.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// What to ask the daemon again after a reconnect, since events were missed: the status,
    /// the chat list and the selected chat's messages. The draft, the selected chat, the open
    /// pane and the selected message are kept.
    pub fn resync(&mut self) -> Vec<ClientFrame> {
        let mut frames = vec![ClientFrame::status(), ClientFrame::chats(CHAT_LIMIT)];
        self.resyncing = self.selected_jid().map(str::to_owned);
        if let Some(jid) = &self.resyncing {
            frames.push(ClientFrame::messages(jid.clone(), MESSAGE_LIMIT));
        }
        frames
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The text being written.
    pub fn composer(&self) -> &str {
        &self.composer
    }

    /// The JID of the selected chat, if any.
    pub fn selected_jid(&self) -> Option<&str> {
        self.selected_chat().map(|chat| chat.jid.as_str())
    }

    /// Select the next chat, stopping at the last one.
    pub fn select_next(&mut self) {
        self.select(self.selected.saturating_add(1));
    }

    /// Select the previous chat, stopping at the first one.
    pub fn select_previous(&mut self) {
        self.select(self.selected.saturating_sub(1));
    }

    pub fn toggle_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Chats => Pane::Messages,
            Pane::Messages | Pane::Compose => Pane::Chats,
        };
    }

    fn select(&mut self, index: usize) {
        let index = index.min(self.snapshot.chats.len().saturating_sub(1));
        if index != self.selected {
            self.scroll = 0;
            self.cursor = None;
        }
        self.selected = index;
        self.list.select(if self.snapshot.chats.is_empty() {
            None
        } else {
            Some(self.selected)
        });
    }

    fn selected_chat(&self) -> Option<&Chat> {
        self.snapshot.chats.get(self.selected)
    }

    /// Replace the chat list, archived chats last, keeping the selection on the same chat.
    fn set_chats(&mut self, mut chats: Vec<Chat>) {
        let selected = self.selected_jid().map(str::to_owned);
        chats.sort_by_key(|chat| chat.archived);
        self.snapshot.chats = chats;
        let kept =
            selected.and_then(|jid| self.snapshot.chats.iter().position(|chat| chat.jid == jid));
        if kept.is_none() {
            self.scroll = 0;
            self.cursor = None;
        }
        self.selected = kept.unwrap_or(0);
        self.list.select(if self.snapshot.chats.is_empty() {
            None
        } else {
            Some(self.selected)
        });
    }

    /// Open the selected chat: show its conversation and ask the daemon for its messages.
    fn open_selected(&mut self) -> Option<Action> {
        let jid = self.selected_jid()?.to_owned();
        self.pane = Pane::Messages;
        self.search = None;
        self.opening = Some(jid.clone());
        Some(Action::Request(ClientFrame::messages(jid, MESSAGE_LIMIT)))
    }

    /// Mark chat `jid` read here and on WhatsApp, unless a `mark_read` for it is in flight.
    fn mark_read(&mut self, jid: &str) -> Option<Action> {
        self.marking
            .insert(jid.to_owned())
            .then(|| Action::Request(ClientFrame::mark_read(jid)))
    }

    /// The chat whose conversation the user has open (not just selected in the list).
    fn is_open(&self, jid: &str) -> bool {
        self.pane != Pane::Chats && self.selected_jid() == Some(jid)
    }

    /// The open chat's loaded conversation, oldest first.
    fn loaded(&self) -> &[Message] {
        self.selected_jid()
            .and_then(|jid| self.conversations.get(jid))
            .map_or(&[], Vec::as_slice)
    }

    fn selected_message(&self) -> Option<&Message> {
        self.loaded().get(self.cursor?)
    }

    /// The selected message's id, when WhatsApp has that message (received, or a send of mine
    /// that was sent), so it can be replied or reacted to. A send that is still sending, queued
    /// or not sent has no WhatsApp message to point at: the footer says so (`what` is the verb,
    /// e.g. "reacted to") and `None` is returned. No message selected is just `None`.
    fn target_id(&mut self, what: &str) -> Option<String> {
        let message = self.selected_message()?;
        if message.id.is_empty() || message.state.is_some() || self.is_sending(message) {
            self.notice = Some(format!("that message was not sent, so it cannot be {what}"));
            return None;
        }
        Some(message.id.clone())
    }

    /// `K`: select the newest message, then one older each time.
    fn select_older(&mut self) {
        let len = self.loaded().len();
        if len > 0 {
            self.cursor = Some(self.cursor.map_or(len - 1, |i| i.saturating_sub(1)));
            self.reveal = true;
        }
    }

    /// `J`: select one newer message, stopping at the newest.
    fn select_newer(&mut self) {
        let len = self.loaded().len();
        if let Some(i) = self.cursor {
            self.cursor = Some((i + 1).min(len.saturating_sub(1)));
            self.reveal = true;
        }
    }

    /// `R`: one more attempt of the selected send that ended not sent. Never automatic.
    fn retry_selected(&mut self) -> Option<Action> {
        let message = self.selected_message()?;
        if message.state != Some(MessageState::NotSent) {
            return None;
        }
        let client_id = message.client_id.clone()?;
        self.sending.insert(client_id.clone());
        Some(Action::Request(ClientFrame::retry(client_id)))
    }

    /// `o`: ask the daemon for the selected media message's file.
    fn download_selected(&mut self) -> Option<Action> {
        let message = self.selected_message()?;
        if message.kind == "text" || message.id.is_empty() {
            return None;
        }
        let id = message.id.clone();
        let jid = self.selected_jid()?.to_owned();
        self.downloading.insert(id.clone());
        Some(Action::Request(ClientFrame::download_media(jid, id)))
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
        let id = self.target_id("reacted to")?;
        let jid = self.selected_jid()?.to_owned();
        Some(Action::Request(ClientFrame::react(jid, id, emoji)))
    }

    fn search_key(&mut self, key: KeyEvent) -> Option<Action> {
        let jid = self.selected_jid()?.to_owned();
        let search = self.search.as_mut()?;
        match key.code {
            KeyCode::Esc => self.search = None,
            KeyCode::Backspace if search.editing => {
                search.query.pop();
            }
            KeyCode::Enter if search.editing => {
                let query = search.query.trim();
                if !query.is_empty() {
                    search.editing = false;
                    search.awaiting = true;
                    return Some(Action::Request(ClientFrame::search(
                        jid,
                        query,
                        SEARCH_LIMIT,
                    )));
                }
            }
            KeyCode::Char(c) if search.editing => search.query.push(c),
            KeyCode::Char('/') => search.editing = true,
            KeyCode::Char('j') | KeyCode::Down => search.list.select_next(),
            KeyCode::Char('k') | KeyCode::Up => search.list.select_previous(),
            KeyCode::Enter => self.jump_to_result(),
            _ => {}
        }
        None
    }

    /// Close the search and select the chosen result in the conversation, if it is loaded.
    fn jump_to_result(&mut self) {
        let Some(search) = self.search.take() else {
            return;
        };
        let results = search.results.unwrap_or_default();
        let Some(result) = search.list.selected().and_then(|i| results.get(i)) else {
            return;
        };
        match self.loaded().iter().position(|m| m.id == result.id) {
            Some(index) => {
                self.cursor = Some(index);
                self.reveal = true;
                self.pane = Pane::Messages;
            }
            None => self.notice = Some("that message is older than the loaded ones".to_owned()),
        }
    }

    /// One key press. Returns what the I/O loop must do, if anything.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if self.pane == Pane::Compose {
            return self.compose_key(key);
        }
        if self.search.is_some() {
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
        match (self.pane, key.code) {
            (_, KeyCode::Char('?')) => self.help = true,
            (_, KeyCode::Char('q')) | (Pane::Chats, KeyCode::Esc) => return Some(Action::Quit),
            (_, KeyCode::Tab) => self.toggle_pane(),
            (_, KeyCode::Char('i')) if self.selected_chat().is_some() => {
                self.reply_to = None;
                self.pane = Pane::Compose;
            }
            (_, KeyCode::Char('/')) if self.selected_chat().is_some() => {
                self.search = Some(Search {
                    editing: true,
                    ..Search::default()
                });
            }
            (Pane::Chats, KeyCode::Char('j') | KeyCode::Down) => self.select_next(),
            (Pane::Chats, KeyCode::Char('k') | KeyCode::Up) => self.select_previous(),
            (Pane::Chats, KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right) => {
                return self.open_selected();
            }
            (Pane::Messages, KeyCode::Esc) if self.cursor.is_some() => self.cursor = None,
            (Pane::Messages, KeyCode::Char('K')) => self.select_older(),
            (Pane::Messages, KeyCode::Char('J')) => self.select_newer(),
            (Pane::Messages, KeyCode::Char('r')) => {
                if let Some(id) = self.target_id("replied to") {
                    self.reply_to = Some(id);
                    self.pane = Pane::Compose;
                }
            }
            (Pane::Messages, KeyCode::Char('e')) => {
                self.picking = self.target_id("reacted to").is_some();
            }
            (Pane::Messages, KeyCode::Char('R')) => return self.retry_selected(),
            (Pane::Messages, KeyCode::Char('o')) => return self.download_selected(),
            (Pane::Messages, KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left) => {
                self.pane = Pane::Chats;
            }
            (Pane::Messages, KeyCode::Enter) => self.pane = Pane::Compose,
            (Pane::Messages, KeyCode::Char('k') | KeyCode::Up) => self.scroll_up(1),
            (Pane::Messages, KeyCode::Char('j') | KeyCode::Down) => self.scroll_down(1),
            (_, KeyCode::PageUp) => self.scroll_up(PAGE),
            (_, KeyCode::PageDown) => self.scroll_down(PAGE),
            (Pane::Messages, KeyCode::Char('g') | KeyCode::Home) => self.scroll = usize::MAX,
            (Pane::Messages, KeyCode::Char('G') | KeyCode::End) => self.scroll = 0,
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
        match key.code {
            KeyCode::Esc => {
                self.reply_to = None;
                self.pane = Pane::Messages;
            }
            KeyCode::Enter if alt => self.insert('\n'),
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Enter => return self.send(),
            KeyCode::Backspace => {
                if let Some(start) = previous_grapheme(&self.composer, self.caret) {
                    self.composer.replace_range(start..self.caret, "");
                    self.caret = start;
                }
            }
            KeyCode::Delete => {
                if let Some(end) = next_grapheme(&self.composer, self.caret) {
                    self.composer.replace_range(self.caret..end, "");
                }
            }
            KeyCode::Left => {
                if let Some(start) = previous_grapheme(&self.composer, self.caret) {
                    self.caret = start;
                }
            }
            KeyCode::Right => {
                if let Some(end) = next_grapheme(&self.composer, self.caret) {
                    self.caret = end;
                }
            }
            KeyCode::Home => {
                self.caret = self.composer[..self.caret]
                    .rfind('\n')
                    .map_or(0, |newline| newline + 1);
            }
            KeyCode::End => {
                self.caret = self.composer[self.caret..]
                    .find('\n')
                    .map_or(self.composer.len(), |newline| self.caret + newline);
            }
            KeyCode::Char(c) => self.insert(c),
            _ => {}
        }
        None
    }

    /// Put `c` into the draft at the cursor and move the cursor after it.
    fn insert(&mut self, c: char) {
        self.composer.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Send the composer's text to the selected chat as one message. It is shown at once as
    /// "sending" until the daemon answers.
    fn send(&mut self) -> Option<Action> {
        if self.composer.trim().is_empty() {
            return None;
        }
        let jid = self.selected_jid()?.to_owned();
        let text = std::mem::take(&mut self.composer);
        self.caret = 0;
        self.sends += 1;
        let client_id = format!("{}-{}", self.client_prefix, self.sends);
        self.sending.insert(client_id.clone());
        self.conversations
            .entry(jid.clone())
            .or_default()
            .push(Message {
                id: String::new(),
                kind: "text".to_owned(),
                text: text.clone(),
                from_me: true,
                timestamp: String::new(),
                media: None,
                state: None,
                client_id: Some(client_id.clone()),
                reply_to: self.reply_to.clone(),
                reactions: Vec::new(),
            });
        self.pane = Pane::Messages;
        self.scroll = 0;
        Some(Action::Request(match self.reply_to.take() {
            Some(quoted) => ClientFrame::send_reply(jid, text, client_id, quoted),
            None => ClientFrame::send(jid, text, client_id),
        }))
    }

    /// Fold one daemon frame (a reply or an event) into the model. An `event_focus` asks for
    /// the focused chat's messages; the messages of a chat just opened with unread messages, or
    /// a new message in the open chat, ask for a `mark_read` (one in flight per chat).
    pub fn apply(&mut self, frame: &DaemonFrame) -> Option<Action> {
        match frame {
            DaemonFrame::Status {
                daemon,
                linked,
                session,
                unread,
                ..
            } => {
                self.snapshot.daemon = daemon.clone();
                self.snapshot.linked = *linked;
                self.snapshot.session = session.clone();
                self.snapshot.unread = *unread;
            }
            DaemonFrame::Chats { chats, .. } => self.set_chats(chats.clone()),
            DaemonFrame::Messages { jid, messages, .. }
                if self.selected_jid() == Some(jid.as_str())
                    && self.search.as_ref().is_some_and(|s| s.awaiting) =>
            {
                if let Some(search) = self.search.as_mut() {
                    search.awaiting = false;
                    search.list.select((!messages.is_empty()).then_some(0));
                    search.results = Some(messages.clone());
                }
            }
            DaemonFrame::Messages { jid, messages, .. } => {
                let resynced = self.resyncing.as_deref() == Some(jid.as_str());
                if resynced {
                    self.resyncing = None;
                }
                let selected_id = self
                    .cursor
                    .and_then(|i| self.conversations.get(jid)?.get(i))
                    .map(|m| m.id.clone());
                let mut loaded: Vec<Message> = messages.iter().rev().cloned().collect();
                // Sends still waiting for a reply are not stored yet; keep them visible. After a
                // reconnect, so are sends that never reached the daemon (not sent): a failure
                // stays on screen until the user retries it.
                if let Some(old) = self.conversations.get(jid) {
                    let unstored = |m: &Message| {
                        resynced
                            && m.id.is_empty()
                            && m.client_id.as_ref().is_some_and(|id| {
                                !loaded.iter().any(|l| l.client_id.as_ref() == Some(id))
                            })
                    };
                    let kept: Vec<Message> = old
                        .iter()
                        .filter(|m| self.is_sending(m) || unstored(m))
                        .cloned()
                        .collect();
                    loaded.extend(kept);
                }
                if self.selected_jid() == Some(jid.as_str()) {
                    // A resync keeps the selected message where it still is; anything else
                    // starts with none selected.
                    self.cursor = selected_id
                        .filter(|id| resynced && !id.is_empty())
                        .and_then(|id| loaded.iter().position(|m| m.id == id));
                }
                self.conversations.insert(jid.clone(), loaded);
                // The chat the user opened is read once its messages are shown (S8).
                if self.opening.as_deref() == Some(jid.as_str()) {
                    self.opening = None;
                    let unread = self.snapshot.chats.iter().find(|c| &c.jid == jid);
                    if unread.is_some_and(|chat| chat.unread > 0) {
                        return self.mark_read(jid);
                    }
                }
            }
            DaemonFrame::MarkedRead { jid, unread, .. } => {
                self.marking.remove(jid);
                self.snapshot.unread = *unread;
                if let Some(chat) = self.snapshot.chats.iter_mut().find(|c| &c.jid == jid) {
                    chat.unread = 0;
                }
            }
            DaemonFrame::Sent {
                client_id,
                state,
                id,
                timestamp,
                ..
            } => {
                self.sending.remove(client_id);
                self.unconfirmed.remove(client_id);
                self.settle(client_id, state, id.as_deref(), timestamp.as_deref());
            }
            DaemonFrame::SendFailed { client_id, .. } => {
                self.sending.remove(client_id);
                self.unconfirmed.remove(client_id);
                if let Some(message) = self.find_send(client_id) {
                    message.state = Some(MessageState::NotSent);
                }
            }
            DaemonFrame::Error { message, .. } => self.notice = Some(message.clone()),
            DaemonFrame::Reacted { jid, id, emoji, .. } => {
                let target = self
                    .conversations
                    .get_mut(jid)
                    .and_then(|messages| messages.iter_mut().find(|m| &m.id == id));
                if let Some(message) = target {
                    message.reactions.retain(|r| !r.from_me);
                    if !emoji.is_empty() {
                        let mine = Reaction {
                            emoji: emoji.clone(),
                            from_me: true,
                        };
                        message.reactions.insert(0, mine);
                    }
                }
            }
            DaemonFrame::MediaReady { id, path, .. } if self.downloading.remove(id) => {
                return Some(Action::Open(PathBuf::from(path)));
            }
            DaemonFrame::EventMessage { jid, message, .. } => {
                // A new message in the open chat is read at once, as the official clients do.
                if self.message_event(jid, message) && !message.from_me && self.is_open(jid) {
                    return self.mark_read(jid);
                }
            }
            DaemonFrame::EventUnread {
                unread,
                jid,
                chat_unread,
                ..
            } => {
                self.snapshot.unread = *unread;
                if let Some(chat) = self.snapshot.chats.iter_mut().find(|c| &c.jid == jid) {
                    chat.unread = *chat_unread;
                }
            }
            DaemonFrame::EventConnection { session, .. } => {
                self.snapshot.session = session.clone();
            }
            // App-state sync changed a chat's pin, archive or mute: take the chat as it now is and
            // sort the list again as the daemon lists it (newest message first, so an unarchived
            // chat goes back to its place; archived chats last), keeping the selection.
            DaemonFrame::EventChat { chat, .. } => {
                let mut chats = self.snapshot.chats.clone();
                match chats.iter_mut().find(|c| c.jid == chat.jid) {
                    Some(shown) => *shown = chat.clone(),
                    None => chats.push(chat.clone()),
                }
                // RFC 3339 UTC timestamps sort as text; a chat with no message sorts last; the
                // same time sorts by JID, as the daemon's `ORDER BY ... DESC, c.jid` does.
                chats.sort_by(|a, b| newest(b).cmp(newest(a)).then_with(|| a.jid.cmp(&b.jid)));
                self.set_chats(chats);
            }
            DaemonFrame::EventFocus { jid, .. } => {
                let index = self.snapshot.chats.iter().position(|c| &c.jid == jid)?;
                self.select(index);
                self.help = false;
                return self.open_selected();
            }
            _ => {}
        }
        None
    }

    /// The daemon refused `frame` with an `error`: show it in the footer and end what the
    /// request left waiting (a download's pending open, a search's "searching…").
    pub fn request_failed(&mut self, frame: &ClientFrame, message: impl Into<String>) {
        self.notice = Some(message.into());
        match frame {
            ClientFrame::DownloadMedia { id, .. } => {
                self.downloading.remove(id);
            }
            ClientFrame::Search { .. } => {
                if let Some(search) = self.search.as_mut() {
                    search.awaiting = false;
                    search.editing = true;
                }
            }
            // The chat stays unread; nothing retries it (the next open does).
            ClientFrame::MarkRead { jid, .. } => {
                self.marking.remove(jid);
            }
            _ => {}
        }
    }

    /// The daemon answered my send (or retry) `client_id` with `not_confirmed`: WhatsApp's
    /// acknowledgement timed out, so it may have been delivered. It is shown as not confirmed, not
    /// as not sent, and `R` still retries it: a retry reuses its WhatsApp id, so it can never
    /// become a second message.
    pub fn send_not_confirmed(&mut self, client_id: &str) {
        self.sending.remove(client_id);
        let Some(message) = self.find_send(client_id) else {
            return;
        };
        message.state = Some(MessageState::NotSent);
        self.unconfirmed.insert(client_id.to_owned());
    }

    fn is_unconfirmed(&self, message: &Message) -> bool {
        message.state == Some(MessageState::NotSent)
            && message
                .client_id
                .as_ref()
                .is_some_and(|id| self.unconfirmed.contains(id))
    }

    fn is_sending(&self, message: &Message) -> bool {
        message
            .client_id
            .as_ref()
            .is_some_and(|id| self.sending.contains(id))
    }

    /// The JID and index of my send with this `client_id`.
    fn locate_send(&self, client_id: &str) -> Option<(String, usize)> {
        self.conversations.iter().find_map(|(jid, messages)| {
            messages
                .iter()
                .position(|m| m.client_id.as_deref() == Some(client_id))
                .map(|index| (jid.clone(), index))
        })
    }

    fn find_send(&mut self, client_id: &str) -> Option<&mut Message> {
        let (jid, index) = self.locate_send(client_id)?;
        self.conversations.get_mut(&jid)?.get_mut(index)
    }

    /// Apply a `sent` reply to my send.
    fn settle(&mut self, client_id: &str, state: &SendState, id: Option<&str>, at: Option<&str>) {
        let Some((jid, index)) = self.locate_send(client_id) else {
            return;
        };
        let Some(messages) = self.conversations.get_mut(&jid) else {
            return;
        };
        match state {
            SendState::Sent => {
                let id = id.unwrap_or_default();
                // Its event may have come first: then the message is already listed.
                if !id.is_empty() && messages.iter().any(|m| m.id == id) {
                    messages.remove(index);
                    return;
                }
                let message = &mut messages[index];
                message.id = id.to_owned();
                message.timestamp = at.unwrap_or_default().to_owned();
                message.state = None;
                message.client_id = None;
            }
            SendState::Queued => messages[index].state = Some(MessageState::Queued),
            SendState::Unknown => messages[index].state = Some(MessageState::Unknown),
        }
    }

    /// A new message, or a new state of one already shown (same `id`, or same `client_id` for
    /// a send of mine). True when it was added to the loaded conversation as a new message.
    fn message_event(&mut self, jid: &str, message: &Message) -> bool {
        if let Some(client_id) = &message.client_id {
            self.sending.remove(client_id);
            // The daemon publishes a not-confirmed send as not_sent (before or after its reply),
            // so only a later queued or sent state clears the mark.
            if message.state != Some(MessageState::NotSent) {
                self.unconfirmed.remove(client_id);
            }
        }
        if let Some(chat) = self.snapshot.chats.iter_mut().find(|c| c.jid == jid) {
            chat.last_message = Some(message.clone());
        } else {
            let mut chats = self.snapshot.chats.clone();
            chats.push(Chat {
                jid: jid.to_owned(),
                name: String::new(),
                unread: 0,
                archived: false,
                pinned: false,
                muted: false,
                last_message: Some(message.clone()),
            });
            self.set_chats(chats);
        }
        let Some(messages) = self.conversations.get_mut(jid) else {
            return false;
        };
        let same = |m: &Message| {
            (!message.id.is_empty() && m.id == message.id)
                || (message.client_id.is_some() && m.client_id == message.client_id)
        };
        match messages.iter_mut().find(|m| same(m)) {
            Some(existing) => {
                *existing = message.clone();
                false
            }
            None => {
                messages.push(message.clone());
                true
            }
        }
    }

    /// Draw the whole screen into `frame`.
    pub fn draw(&mut self, frame: &mut Frame) {
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
        if self.search.is_some() {
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
        let chat = self.selected_chat().cloned();
        let palette = self.palette.clone();
        let muted = self.muted();
        let Some(search) = self.search.as_mut() else {
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
        let cursor = if search.editing { "▏" } else { "" };
        frame.render_widget(
            Paragraph::new(format!("Search: {}{cursor}", plain(&search.query)))
                .style(Style::default().fg(rgb(palette.foreground))),
            prompt,
        );
        let items: Vec<ListItem> = match (&search.results, search.awaiting) {
            (_, true) => vec![ListItem::new(Span::styled("searching…", muted))],
            (None, false) => vec![ListItem::new(Span::styled("Enter to search", muted))],
            (Some(found), false) if found.is_empty() => {
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
        frame.render_stateful_widget(list, results, &mut search.list);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let status = match self.source {
            Source::Daemon if !self.connected => Span::styled(
                format!(
                    " · {} · reconnecting… · {} unread",
                    self.snapshot.daemon, self.snapshot.unread
                ),
                Style::default()
                    .fg(rgb(self.palette.yellow))
                    .add_modifier(Modifier::BOLD),
            ),
            Source::Daemon => Span::styled(
                format!(
                    " · {} · {} · {} unread",
                    self.snapshot.daemon,
                    self.snapshot.session.as_str(),
                    self.snapshot.unread
                ),
                self.muted(),
            ),
            Source::Fixture => Span::styled(
                format!(
                    " · FIXTURE DATA · no daemon · {} unread",
                    self.snapshot.unread
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
        let keys = match self.pane {
            _ if self.search.is_some() => " Enter search/select · j/k move · / edit · Esc close ",
            _ if self.picking => " 1-6 react · x remove mine · Esc cancel ",
            Pane::Chats => " q quit · j/k move · Enter open · i write · / search · ? help ",
            Pane::Messages if self.cursor.is_some() => {
                " r reply · e react · R retry · o open media · J/K select · Esc unselect "
            }
            Pane::Messages => " Esc chats · j/k scroll · K select · i write · / search · ? help ",
            Pane::Compose if self.reply_to.is_some() => {
                " Enter send reply · Alt-Enter newline · ←/→ move · Esc cancel "
            }
            Pane::Compose => " Enter send · Alt-Enter newline · ←/→ move · Esc cancel ",
        };
        // The notice comes first, so a narrow terminal still shows it.
        let mut spans = Vec::new();
        if let Some(notice) = &self.notice {
            spans.push(Span::styled(
                format!(" {notice} ·"),
                Style::default().fg(rgb(self.palette.red)),
            ));
        }
        spans.push(Span::styled(keys, self.muted()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line> = KEYS
            .iter()
            .map(|(keys, what)| {
                Line::from(vec![
                    Span::styled(
                        format!("{keys:>13} "),
                        Style::default().fg(rgb(self.palette.accent)),
                    ),
                    Span::styled(*what, Style::default().fg(rgb(self.palette.foreground))),
                ])
            })
            .collect();
        // As tall as the wrapped lines need, so the last keys are not cut off.
        let help = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(rgb(self.palette.background)))
            .block(bordered(&self.palette).title(" Keys "));
        // `line_count` counts the block's rows but wraps at the width it is given: the inner one.
        let height = help.line_count(area.width.min(72).saturating_sub(2));
        let popup = popup(area, 72, u16::try_from(height).unwrap_or(u16::MAX));
        frame.render_widget(Clear, popup);
        frame.render_widget(help, popup);
    }

    fn draw_chats(&mut self, frame: &mut Frame, area: Rect) {
        let block = bordered(&self.palette).title(format!(" Chats ({}) ", self.chats().len()));
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
            .highlight_style(
                Style::default()
                    .bg(rgb(self.palette.accent))
                    .fg(rgb(self.palette.background))
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_stateful_widget(list, area, &mut self.list);
    }

    fn draw_messages(&mut self, frame: &mut Frame, area: Rect) {
        let title = self
            .selected_chat()
            .map(chat_title)
            .unwrap_or_else(|| "no chat selected".to_owned());
        let block = bordered(&self.palette).title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(rgb(self.palette.foreground))
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
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
            .reply_to
            .as_ref()
            .filter(|_| self.pane == Pane::Compose)
            .map(|id| match self.loaded().iter().find(|m| &m.id == id) {
                Some(message) => format!("── replying to: {} ", snippet(message)),
                None => "── replying ".to_owned(),
            });
        let rule_text = quoted.unwrap_or_default() + &"─".repeat(usize::from(rule.width));
        frame.render_widget(Paragraph::new(rule_text).style(self.muted()), rule);
        self.draw_composer(frame, composer, draft);
    }

    /// The draft laid out for a composer `width` columns wide, after the 2-column prompt, plus
    /// the cursor's row and column; `None` when there is no draft to show (the hint instead).
    fn draft_rows(&self, width: u16) -> Option<DraftRows> {
        (self.pane == Pane::Compose || !self.composer.is_empty()).then(|| {
            layout_draft(
                &self.composer,
                self.caret,
                usize::from(width.saturating_sub(2)),
            )
        })
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect, draft: Option<DraftRows>) {
        if area.is_empty() {
            return;
        }
        let Some((rows, (row, column))) = draft else {
            let hint = Line::from(Span::styled("  i to write a message", self.muted()));
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
        let (lines, selected) = self.conversation_lines();
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
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total = paragraph.line_count(area.width);
        let top = total.saturating_sub(visible);
        self.scroll = self.scroll.min(top);
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
    }

    /// The selected chat's conversation, oldest first; before it is loaded, its last message.
    /// Also the index of the selected message's header line.
    fn conversation_lines(&self) -> (Vec<Line<'static>>, Option<usize>) {
        let Some(chat) = self.selected_chat() else {
            return (Vec::new(), None);
        };
        let loaded = self.conversations.get(&chat.jid);
        let messages: Vec<&Message> = match loaded {
            Some(messages) => messages.iter().collect(),
            None => chat.last_message.iter().collect(),
        };
        if messages.is_empty() {
            return (
                vec![Line::from(Span::styled("(no messages)", self.muted()))],
                None,
            );
        }
        let body = Style::default().fg(rgb(self.palette.foreground));
        let mut lines = Vec::with_capacity(messages.len() * 2);
        let mut selected = None;
        for (index, message) in messages.iter().enumerate() {
            let chosen = loaded.is_some() && self.cursor == Some(index);
            let mut header = if chosen {
                selected = Some(lines.len());
                format!("› {}", self.header_of(chat, message))
            } else {
                self.header_of(chat, message)
            };
            if self.is_sending(message) {
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
            if let Some(quoted) = &message.reply_to {
                let quote = match messages.iter().find(|m| &m.id == quoted) {
                    Some(original) => snippet(original),
                    None => "(a message not loaded)".to_owned(),
                };
                lines.push(Line::from(Span::styled(format!("> {quote}"), self.muted())));
            }
            let text = if message.kind == "text" {
                message.text.clone()
            } else {
                format!("[{}] {}", message.kind, message.text)
            };
            lines.extend(
                text.lines()
                    .map(|line| Line::from(Span::styled(plain(line), body))),
            );
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
        }
        (lines, selected)
    }

    fn header_of(&self, chat: &Chat, message: &Message) -> String {
        sent_by(
            Some(chat),
            message,
            self.utc_offset,
            self.is_unconfirmed(message),
        )
    }

    fn muted(&self) -> Style {
        Style::default()
            .fg(rgb(self.palette.foreground))
            .add_modifier(Modifier::DIM)
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

/// The start of the grapheme before byte `at` of `text`, if there is one.
fn previous_grapheme(text: &str, at: usize) -> Option<usize> {
    text.grapheme_indices(true)
        .map(|(start, _)| start)
        .take_while(|&start| start < at)
        .last()
}

/// The end of the grapheme after byte `at` of `text`, if there is one.
fn next_grapheme(text: &str, at: usize) -> Option<usize> {
    (at < text.len()).then(|| {
        text.grapheme_indices(true)
            .map(|(start, _)| start)
            .find(|&start| start > at)
            .unwrap_or(text.len())
    })
}

/// The timestamp of a chat's newest message, or "" for a chat with none.
fn newest(chat: &Chat) -> &str {
    chat.last_message
        .as_ref()
        .map_or("", |message| message.timestamp.as_str())
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

fn rgb(colour: Rgb) -> Color {
    Color::Rgb(colour.r, colour.g, colour.b)
}

/// The name to show for a chat: its display name, or the JID's user part when that is
/// empty (the wire contract allows an empty name).
fn chat_title(chat: &Chat) -> String {
    if chat.name.is_empty() {
        chat.jid
            .split('@')
            .next()
            .unwrap_or(chat.jid.as_str())
            .to_owned()
    } else {
        plain(&chat.name)
    }
}

/// Who sent the message (us, or the chat) and when; a send of ours that is not sent yet says
/// so, so a failure stays visible, and one that was not confirmed (`unconfirmed`) says that
/// instead, since it may have been delivered.
fn sent_by(
    chat: Option<&Chat>,
    message: &Message,
    offset: time::UtcOffset,
    unconfirmed: bool,
) -> String {
    let who = match (message.from_me, chat) {
        (true, _) => "you".to_owned(),
        (false, Some(chat)) => chat_title(chat),
        (false, None) => "them".to_owned(),
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

/// A chat in the list: its markers, name and unread badge, then its last message cut to `width`
/// columns with `…` so the ` · HH:MM` after it is always whole.
fn chat_item(
    chat: &Chat,
    palette: &Palette,
    width: usize,
    offset: time::UtcOffset,
) -> ListItem<'static> {
    let markers = Span::styled(
        format!(
            "{}{}{} ",
            if chat.pinned { PINNED } else { ' ' },
            if chat.muted { MUTED } else { ' ' },
            if chat.archived { ARCHIVED } else { ' ' }
        ),
        Style::default().fg(rgb(palette.accent)),
    );
    let mut title = vec![
        markers,
        Span::styled(
            chat_title(chat),
            Style::default()
                .fg(rgb(palette.foreground))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if chat.unread > 0 {
        title.push(Span::raw(" "));
        title.push(Span::styled(
            format!("({})", chat.unread),
            Style::default()
                .bg(rgb(palette.red))
                .fg(rgb(palette.background))
                .add_modifier(Modifier::BOLD),
        ));
    }

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
    use super::chat_title;
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
            last_message: None,
        }
    }

    #[test]
    fn a_chat_with_no_name_shows_its_jid_user_part() {
        assert_eq!(
            chat_title(&chat("", "15551230001@s.whatsapp.net")),
            "15551230001"
        );
        assert_eq!(
            chat_title(&chat("Ada Lovelace", "15551230001@s.whatsapp.net")),
            "Ada Lovelace"
        );
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
