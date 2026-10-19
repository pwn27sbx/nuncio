//! `nuncio-tui`'s model and rendering, as a library so it can be tested against a
//! `TestBackend` buffer without a terminal or a daemon.
//!
//! [`App`] is the pure state: the chat list (archived chats sectioned last), the loaded
//! conversations, the composer, the selection and scroll, and the Omarchy [`Palette`]. It changes
//! only through [`App::handle_key`] and [`App::apply`] (a reply or an event frame), which return
//! an [`Action`] for the I/O loop. [`Daemon`] is that I/O: a request connection and a subscribed
//! event connection to `nunciod`. The keys are listed in [`KEYS`] and in `README.md`.

use std::collections::{HashMap, HashSet};

use nuncio_client::wire::MessageState;
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message, Palette, Rgb, SendState, Snapshot};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use ratatui_image::protocol::StatefulProtocol;

mod daemon;

pub use daemon::Daemon;

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
}

/// Marks a pinned chat.
const PINNED: char = '★';
/// Marks a muted chat.
const MUTED: char = '⊘';
/// Marks an archived chat.
const ARCHIVED: char = '▤';

/// How many messages a conversation loads (the wire maximum).
const MESSAGE_LIMIT: u16 = 200;
/// How many lines PageUp/PageDown scroll.
const PAGE: usize = 10;

/// Every key, by pane; drawn by the `?` help screen and documented in the crate README.
pub const KEYS: &[(&str, &str)] = &[
    ("j/k ↑/↓", "chats: move · conversation: scroll a line"),
    ("Enter l →", "open the selected chat"),
    ("i", "write a message to the selected chat"),
    ("Enter", "in the composer: send"),
    (
        "Esc",
        "composer → conversation → chats; quit from the chats",
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
    /// `client_id`s of sends that have no reply yet.
    sending: HashSet<String>,
    client_prefix: String,
    sends: u64,
    help: bool,
    notice: Option<String>,
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
            sending: HashSet::new(),
            client_prefix: "tui".to_owned(),
            sends: 0,
            help: false,
            notice: None,
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
        Some(Action::Request(ClientFrame::messages(jid, MESSAGE_LIMIT)))
    }

    /// One key press. Returns what the I/O loop must do, if anything.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if self.pane == Pane::Compose {
            return self.compose_key(key);
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
            (_, KeyCode::Char('i')) if self.selected_chat().is_some() => self.pane = Pane::Compose,
            (Pane::Chats, KeyCode::Char('j') | KeyCode::Down) => self.select_next(),
            (Pane::Chats, KeyCode::Char('k') | KeyCode::Up) => self.select_previous(),
            (Pane::Chats, KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right) => {
                return self.open_selected();
            }
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

    fn compose_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => self.pane = Pane::Messages,
            KeyCode::Backspace => {
                self.composer.pop();
            }
            KeyCode::Enter => return self.send(),
            KeyCode::Char(c) => self.composer.push(c),
            _ => {}
        }
        None
    }

    /// Send the composer's text to the selected chat as one message. It is shown at once as
    /// "sending" until the daemon answers.
    fn send(&mut self) -> Option<Action> {
        if self.composer.trim().is_empty() {
            return None;
        }
        let jid = self.selected_jid()?.to_owned();
        let text = std::mem::take(&mut self.composer);
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
            });
        self.pane = Pane::Messages;
        self.scroll = 0;
        Some(Action::Request(ClientFrame::send(jid, text, client_id)))
    }

    /// Fold one daemon frame (a reply or an event) into the model. An `event_focus` asks for
    /// the focused chat's messages.
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
            DaemonFrame::Messages { jid, messages, .. } => {
                let mut loaded: Vec<Message> = messages.iter().rev().cloned().collect();
                // Sends still waiting for a reply are not stored yet; keep them visible.
                if let Some(old) = self.conversations.get(jid) {
                    loaded.extend(old.iter().filter(|m| self.is_sending(m)).cloned());
                }
                self.conversations.insert(jid.clone(), loaded);
            }
            DaemonFrame::Sent {
                client_id,
                state,
                id,
                timestamp,
                ..
            } => {
                self.sending.remove(client_id);
                self.settle(client_id, state, id.as_deref(), timestamp.as_deref());
            }
            DaemonFrame::SendFailed { client_id, .. } => {
                self.sending.remove(client_id);
                if let Some(message) = self.find_send(client_id) {
                    message.state = Some(MessageState::NotSent);
                }
            }
            DaemonFrame::Error { message, .. } => self.notice = Some(message.clone()),
            DaemonFrame::EventMessage { jid, message, .. } => self.message_event(jid, message),
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
    /// a send of mine).
    fn message_event(&mut self, jid: &str, message: &Message) {
        if let Some(client_id) = &message.client_id {
            self.sending.remove(client_id);
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
            return;
        };
        let same = |m: &Message| {
            (!message.id.is_empty() && m.id == message.id)
                || (message.client_id.is_some() && m.client_id == message.client_id)
        };
        match messages.iter_mut().find(|m| same(m)) {
            Some(existing) => *existing = message.clone(),
            None => messages.push(message.clone()),
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
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let status = match self.source {
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
            Pane::Chats => " q quit · j/k move · Enter open · i write · Tab pane · ? help ",
            Pane::Messages => " Esc chats · j/k scroll · g/G top/bottom · i write · ? help ",
            Pane::Compose => " Enter send · Esc cancel ",
        };
        let mut spans = vec![Span::styled(keys, self.muted())];
        if let Some(notice) = &self.notice {
            spans.push(Span::styled(
                format!(" {notice}"),
                Style::default().fg(rgb(self.palette.red)),
            ));
        }
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
        let height = (lines.len() as u16 + 2).min(area.height);
        let width = area.width.min(72);
        let popup = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(rgb(self.palette.background)))
                .block(bordered(&self.palette).title(" Keys ")),
            popup,
        );
    }

    fn draw_chats(&mut self, frame: &mut Frame, area: Rect) {
        let block = bordered(&self.palette).title(format!(" Chats ({}) ", self.chats().len()));
        let items: Vec<ListItem> = self
            .chats()
            .iter()
            .map(|chat| chat_item(chat, &self.palette))
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

        // The composer is the last row, under a rule, inside the same border.
        let [text_area, rule, composer] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(rest);
        self.draw_conversation(frame, text_area);
        frame.render_widget(
            Paragraph::new("─".repeat(usize::from(rule.width))).style(self.muted()),
            rule,
        );
        self.draw_composer(frame, composer);
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let line = if self.pane == Pane::Compose || !self.composer.is_empty() {
            Line::from(vec![
                Span::styled("› ", Style::default().fg(rgb(self.palette.accent))),
                Span::styled(
                    self.composer.as_str(),
                    Style::default().fg(rgb(self.palette.foreground)),
                ),
            ])
        } else {
            Line::from(Span::styled("  i to write a message", self.muted()))
        };
        // Keep the end of a long draft (and the cursor) in view.
        let width = line.width() as u16;
        let offset = (width + 1).saturating_sub(area.width);
        frame.render_widget(Paragraph::new(line).scroll((0, offset)), area);
        if self.pane == Pane::Compose {
            frame.set_cursor_position((area.x + (width - offset).min(area.width - 1), area.y));
        }
    }

    fn draw_conversation(&mut self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let lines = self.conversation_lines();
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total = paragraph.line_count(area.width);
        let visible = usize::from(area.height);
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
    fn conversation_lines(&self) -> Vec<Line<'static>> {
        let Some(chat) = self.selected_chat() else {
            return Vec::new();
        };
        let loaded = self.conversations.get(&chat.jid);
        let messages: Vec<&Message> = match loaded {
            Some(messages) => messages.iter().collect(),
            None => chat.last_message.iter().collect(),
        };
        if messages.is_empty() {
            return vec![Line::from(Span::styled("(no messages)", self.muted()))];
        }
        let body = Style::default().fg(rgb(self.palette.foreground));
        let mut lines = Vec::with_capacity(messages.len() * 2);
        for message in messages {
            let mut header = sent_by(Some(chat), message);
            if self.is_sending(message) {
                header.push_str(" · sending");
            }
            let header_style = match message.state {
                Some(MessageState::NotSent) => Style::default().fg(rgb(self.palette.red)),
                _ if message.from_me => Style::default().fg(rgb(self.palette.accent)),
                _ => self.muted(),
            };
            lines.push(Line::from(Span::styled(header, header_style)));
            let text = if message.kind == "text" {
                message.text.clone()
            } else {
                format!("[{}] {}", message.kind, message.text)
            };
            lines.extend(
                text.lines()
                    .map(|line| Line::from(Span::styled(line.to_owned(), body))),
            );
        }
        lines
    }

    fn muted(&self) -> Style {
        Style::default()
            .fg(rgb(self.palette.foreground))
            .add_modifier(Modifier::DIM)
    }
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
        chat.name.clone()
    }
}

/// Who sent the message (us, or the chat) and when; a send of ours that is not sent yet says
/// so, so a failure stays visible.
fn sent_by(chat: Option<&Chat>, message: &Message) -> String {
    let who = match (message.from_me, chat) {
        (true, _) => "you".to_owned(),
        (false, Some(chat)) => chat_title(chat),
        (false, None) => "them".to_owned(),
    };
    let state = match message.state {
        Some(MessageState::Queued) => " · queued",
        Some(MessageState::NotSent) => " · not sent",
        Some(MessageState::Unknown) | None => "",
    };
    format!("{who} · {}{state}", message.time_hhmm().unwrap_or("--:--"))
}

fn chat_item(chat: &Chat, palette: &Palette) -> ListItem<'static> {
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
    let snippet = match chat.last_message.as_ref() {
        Some(message) if message.text.is_empty() => message.kind.clone(),
        Some(message) => message.text.clone(),
        None => "(no messages)".to_owned(),
    };
    let detail = match chat.last_message.as_ref() {
        Some(message) => format!("{snippet} · {}", message.time_hhmm().unwrap_or("--:--")),
        None => snippet,
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
    fn message_times_are_shown_as_utc_hh_mm() {
        let message = Message {
            id: "3EB0AAA1".to_owned(),
            kind: "text".to_owned(),
            text: "hello".to_owned(),
            from_me: false,
            timestamp: "2026-10-18T17:55:04Z".to_owned(),
            media: None,
            state: None,
            client_id: None,
        };
        assert_eq!(
            super::sent_by(Some(&chat("Ada", "a@b")), &message),
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
        };
        assert_eq!(super::sent_by(None, &message), "you · 17:56 · not sent");
        message.state = Some(nuncio_client::wire::MessageState::Queued);
        assert_eq!(super::sent_by(None, &message), "you · 17:56 · queued");
        message.state = None;
        assert_eq!(super::sent_by(None, &message), "you · 17:56");
    }
}
