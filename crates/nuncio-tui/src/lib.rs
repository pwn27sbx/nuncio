//! `nuncio-tui`'s rendering, as a library so it can be tested against a `TestBackend`
//! buffer without a terminal.
//!
//! [`App`] holds everything the screen needs: the chat list (from the daemon, or from
//! `proto/fixtures/chats_response.json` when the daemon is unreachable), the selection, the
//! Omarchy [`Palette`], and an optional inline image. S9's acceptance test is
//! `tests/render.rs`: it draws this at 80×24 and at odd sizes and asserts on the buffer.

use nuncio_client::{Chat, Message, Palette, Rgb, Snapshot};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use ratatui_image::protocol::StatefulProtocol;

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
    Chats,
    Messages,
}

/// Marks a pinned chat.
const PINNED: char = '★';
/// Marks an archived chat.
const ARCHIVED: char = '▤';

/// The chat list from `proto/fixtures/chats_response.json`.
///
/// Compiled in, so the client always renders when no daemon is reachable — which is what
/// makes S9 checkable with no daemon and no linked device. The fixture is shared with the
/// Go daemon and checked by `nuncio-client`'s contract tests.
pub fn fixture_snapshot() -> Snapshot {
    Snapshot::from_chats_json(include_str!("../../../proto/fixtures/chats_response.json"))
        .expect("the embedded chats fixture matches the wire types")
}

/// The terminal client's state.
pub struct App {
    snapshot: Snapshot,
    source: Source,
    palette: Palette,
    selected: usize,
    pane: Pane,
    list: ListState,
    image: Option<StatefulProtocol>,
}

impl App {
    pub fn new(snapshot: Snapshot, source: Source, palette: Palette) -> Self {
        let mut list = ListState::default();
        if !snapshot.chats.is_empty() {
            list.select(Some(0));
        }
        Self {
            snapshot,
            source,
            palette,
            selected: 0,
            pane: Pane::Chats,
            list,
            image: None,
        }
    }

    /// Attach an inline image (the `--image` debug flag) to show beside the chat list.
    pub fn with_image(mut self, image: StatefulProtocol) -> Self {
        self.image = Some(image);
        self
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
            Pane::Messages => Pane::Chats,
        };
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.snapshot.chats.len().saturating_sub(1));
        self.list.select(if self.snapshot.chats.is_empty() {
            None
        } else {
            Some(self.selected)
        });
    }

    fn selected_chat(&self) -> Option<&Chat> {
        self.snapshot.chats.get(self.selected)
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
        let line = Line::from(Span::styled(
            " q quit · j/k or ↑/↓ move · Tab pane ",
            self.muted(),
        ));
        frame.render_widget(Paragraph::new(line), area);
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
        let (image_area, text_area) = if self.image.is_some() {
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

        if text_area.is_empty() {
            return;
        }
        let styled = match self
            .selected_chat()
            .and_then(|chat| chat.last_message.as_ref())
        {
            Some(message) => vec![
                Line::from(Span::styled(
                    sent_by(self.selected_chat(), message),
                    self.muted(),
                )),
                Line::from(Span::styled(
                    message.text.clone(),
                    Style::default().fg(rgb(self.palette.foreground)),
                )),
            ],
            None => vec![Line::from(Span::styled("(no messages)", self.muted()))],
        };
        frame.render_widget(Paragraph::new(styled).wrap(Wrap { trim: true }), text_area);
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

/// Who sent the message: us, or the chat.
fn sent_by(chat: Option<&Chat>, message: &Message) -> String {
    let who = match (message.from_me, chat) {
        (true, _) => "you".to_owned(),
        (false, Some(chat)) => chat_title(chat),
        (false, None) => "them".to_owned(),
    };
    format!("{who} · {}", message.time_hhmm().unwrap_or("--:--"))
}

fn chat_item(chat: &Chat, palette: &Palette) -> ListItem<'static> {
    let markers = Span::styled(
        format!(
            "{}{} ",
            if chat.pinned { PINNED } else { ' ' },
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
        };
        assert_eq!(
            super::sent_by(Some(&chat("Ada", "a@b")), &message),
            "Ada · 17:55"
        );
    }
}
