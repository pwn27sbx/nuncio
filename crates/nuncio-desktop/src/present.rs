//! What the window shows, as plain data: the chat list's rows, the transcript's bubbles, the
//! headers, the composer's banners and the search results, each read from the shared
//! [`Model`]. Every rule (which chat is selected, what state a send is in, what can be replied
//! to) is the model's; this only words it, so the widgets stay a thin drawing of these values
//! and the wording is tested without GTK.

use std::ffi::OsStr;
use std::path::PathBuf;

use nuncio_client::model::{MERGED_NOTICE, Model, chat_title, is_picture, quoted};
use nuncio_client::wire::MessageState;
use nuncio_client::{Chat, Message, TitleBar};

/// How many characters of a quoted message, a preview or a search result are shown.
const SNIPPET: usize = 60;

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRow {
    pub jid: String,
    pub title: String,
    /// The unread count, when there is one.
    pub unread: Option<String>,
    pub pinned: bool,
    pub muted: bool,
    pub archived: bool,
    /// The last message's first line (`[image]` for a picture with no caption).
    pub preview: String,
    pub time: String,
    /// What a screen reader says for the row.
    pub label: String,
}

/// How a send of mine stands, as a bubble says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMark {
    Sending,
    Queued,
    Sent,
    NotSent,
    NotConfirmed,
}

impl SendMark {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Queued => "queued",
            Self::Sent => "sent",
            Self::NotSent => "not sent",
            Self::NotConfirmed => "not confirmed",
        }
    }

    /// A send that failed, or may have: shown in the error colour, and offered a retry.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::NotSent | Self::NotConfirmed)
    }
}

/// One reaction under a bubble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionView {
    pub emoji: String,
    pub mine: bool,
}

/// One message of the open conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bubble {
    /// Its index in the loaded conversation ([`Model::select_message`]).
    pub index: usize,
    pub mine: bool,
    pub sender: String,
    pub time: String,
    /// Set on my sends only.
    pub mark: Option<SendMark>,
    /// The quoted message's start, when this one replies to another.
    pub quote: Option<String>,
    /// `[image]`, `[sticker] ▶`, `[document]` …, for a media message; `downloading…` after it
    /// while its file is fetched to open it.
    pub media: Option<String>,
    /// The text (a media message's caption).
    pub text: String,
    pub reactions: Vec<ReactionView>,
    pub selected: bool,
    /// WhatsApp has this message, so it can be replied and reacted to.
    pub can_target: bool,
    /// A send that ended not sent (or not confirmed): one more attempt can be asked for.
    pub can_retry: bool,
    /// A media message whose file can be fetched and opened outside the app.
    pub can_open: bool,
    /// The picture an image or sticker message draws inside the bubble.
    pub picture: Option<PictureView>,
    /// What a screen reader says for the bubble.
    pub label: String,
}

/// The picture inside an image or sticker bubble: the thumbnail the message embeds at once, the
/// full file only once the user asked for it ([`Model::download_selected`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureView {
    pub chat: String,
    pub id: String,
    pub sticker: bool,
    /// An animated sticker: the label says `▶`; its thumbnail is drawn still, and its full picture
    /// plays once fetched ([`crate::pictures::decode_picture`]).
    pub animated: bool,
    /// The box the bubble reserves before anything is decoded ([`crate::pictures::picture_box`]),
    /// from the size the message states; `None` when it states none.
    pub size: Option<(u32, u32)>,
    /// The message embeds a thumbnail.
    pub thumb: bool,
    /// The full file, once fetched on request.
    pub full: Option<PathBuf>,
    /// The picture's alternative text: `Photo` or `Sticker`, and the caption.
    pub alt: String,
}

/// A header's two lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub subtitle: String,
}

/// What the composer shows around the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerView {
    /// `Replying to …`, while the draft quotes a message.
    pub replying: Option<String>,
    /// Why nothing can be sent now (the open chat vanished: it was merged).
    pub blocked: Option<String>,
}

/// The in-chat search's results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchView {
    pub awaiting: bool,
    /// `None` before the first answer.
    pub results: Option<Vec<String>>,
    pub selected: Option<usize>,
}

/// `text` cut to [`SNIPPET`] characters, ending in `…` when it was cut.
fn snippet(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > SNIPPET {
        line.chars().take(SNIPPET).collect::<String>() + "…"
    } else {
        line.to_owned()
    }
}

/// A media message's label: its kind in brackets, a moving sticker marked `▶`.
fn media_label(message: &Message) -> Option<String> {
    (message.kind != "text").then(|| {
        let marker = if message.kind == "sticker" && message.animated {
            " ▶"
        } else {
            ""
        };
        format!("[{}]{marker}", message.kind)
    })
}

/// A message in one line: its text, or its media label and caption.
fn one_line(message: &Message) -> String {
    let text = snippet(&message.text);
    match media_label(message) {
        Some(label) if text.is_empty() => label,
        Some(label) => format!("{label} {text}"),
        None => text,
    }
}

/// The message's time as `HH:MM` at `offset_minutes` from UTC (the wire's timestamps are UTC),
/// or `--:--` when it has none, as the terminal client shows it.
pub fn local_hhmm(message: &Message, offset_minutes: i32) -> String {
    let Some(utc) = message.time_hhmm() else {
        return "--:--".to_owned();
    };
    let (Ok(hours), Ok(minutes)) = (utc[..2].parse::<i32>(), utc[3..].parse::<i32>()) else {
        return "--:--".to_owned();
    };
    let local = (hours * 60 + minutes + offset_minutes).rem_euclid(24 * 60);
    format!("{:02}:{:02}", local / 60, local % 60)
}

fn chat_row(chat: &Chat, offset_minutes: i32) -> ChatRow {
    let title = chat_title(chat);
    let unread = (chat.unread > 0).then(|| chat.unread.to_string());
    let (preview, time) = match &chat.last_message {
        Some(message) => (one_line(message), local_hhmm(message, offset_minutes)),
        None => ("(no messages)".to_owned(), String::new()),
    };
    let mut label = title.clone();
    if let Some(unread) = &unread {
        label.push_str(&format!(", {unread} unread"));
    }
    for (on, word) in [
        (chat.pinned, "pinned"),
        (chat.muted, "muted"),
        (chat.archived, "archived"),
    ] {
        if on {
            label.push_str(&format!(", {word}"));
        }
    }
    match &chat.last_message {
        Some(_) => label.push_str(&format!(". Last message at {time}: {preview}")),
        None => label.push_str(". No messages"),
    }
    ChatRow {
        jid: chat.jid.clone(),
        title,
        unread,
        pinned: chat.pinned,
        muted: chat.muted,
        archived: chat.archived,
        preview,
        time,
        label,
    }
}

/// The chat list's rows, in the model's order (archived chats last).
pub fn chat_rows(model: &Model, offset_minutes: i32) -> Vec<ChatRow> {
    model
        .chat_list()
        .chats()
        .iter()
        .map(|chat| chat_row(chat, offset_minutes))
        .collect()
}

/// How my send `message` stands; `None` for a received message.
fn mark(model: &Model, message: &Message) -> Option<SendMark> {
    if !message.from_me {
        return None;
    }
    let transcript = model.transcript();
    Some(if transcript.is_sending(message) {
        SendMark::Sending
    } else {
        match message.state {
            Some(MessageState::Queued) => SendMark::Queued,
            Some(MessageState::NotSent) if transcript.is_unconfirmed(message) => {
                SendMark::NotConfirmed
            }
            Some(MessageState::NotSent) => SendMark::NotSent,
            Some(MessageState::Unknown) | None => SendMark::Sent,
        }
    })
}

/// The selected chat's loaded conversation as bubbles, oldest first. A bubble's sender (and its
/// accessible label) is `You`, the group member the daemon names in `sender`, or the chat's title.
pub fn bubbles(model: &Model, offset_minutes: i32) -> Vec<Bubble> {
    let messages = model.loaded();
    let theirs = model
        .selected_chat()
        .map_or_else(|| "them".to_owned(), chat_title);
    let cursor = model.transcript().cursor();
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let mark = mark(model, message);
            // In a group, the member the daemon names in `sender`; else the chat, as before.
            let sender = match (message.from_me, message.sender.as_deref()) {
                (true, _) => "You".to_owned(),
                (false, Some(sender)) if !sender.is_empty() => sender.to_owned(),
                (false, _) => theirs.clone(),
            };
            let time = local_hhmm(message, offset_minutes);
            let quote = message
                .reply_to
                .as_deref()
                .map(|id| match quoted(messages, id) {
                    Some(original) => one_line(original),
                    None => "(a message not loaded)".to_owned(),
                });
            let media = media_label(message).map(|label| {
                let transcript = model.transcript();
                if transcript.is_downloading(&message.id) || transcript.is_showing(&message.id) {
                    format!("{label} downloading…")
                } else {
                    label
                }
            });
            let reactions: Vec<ReactionView> = message
                .reactions
                .iter()
                .map(|r| ReactionView {
                    emoji: r.emoji.clone(),
                    mine: r.from_me,
                })
                .collect();
            let mut label = format!("{sender}, {time}");
            if let Some(mark) = mark {
                label.push_str(&format!(", {}", mark.as_str()));
            }
            if let Some(quote) = &quote {
                label.push_str(&format!(", replying to: {quote}"));
            }
            label.push_str(": ");
            // A picture reads as what it is (`Photo`, `Sticker`), then its caption.
            let spoken = match picture_word(message) {
                Some(word)
                    if media
                        .as_deref()
                        .is_some_and(|m| m.ends_with("downloading…")) =>
                {
                    Some(format!("{word}, downloading…"))
                }
                Some(word) => Some(word.to_owned()),
                None => media.clone(),
            };
            let separator = if picture_word(message).is_some() {
                ", "
            } else {
                " "
            };
            label.push_str(&match (&spoken, message.text.is_empty()) {
                (Some(media), true) => media.clone(),
                (Some(media), false) => format!("{media}{separator}{}", message.text),
                (None, _) => message.text.clone(),
            });
            if !reactions.is_empty() {
                let emoji: Vec<&str> = reactions.iter().map(|r| r.emoji.as_str()).collect();
                label.push_str(&format!(". Reactions: {}", emoji.join(" ")));
            }
            Bubble {
                index,
                mine: message.from_me,
                sender,
                time,
                mark,
                quote,
                media,
                text: message.text.clone(),
                reactions,
                selected: cursor == Some(index),
                // A send still waiting has no id yet, and one not sent has a state.
                can_target: !message.id.is_empty() && message.state.is_none(),
                // As `Model::retry_selected`: not while an attempt waits for its answer.
                can_retry: message.state == Some(MessageState::NotSent)
                    && message.client_id.is_some()
                    && mark != Some(SendMark::Sending),
                can_open: message.kind != "text" && !message.id.is_empty(),
                picture: picture_view(model, message),
                label,
            }
        })
        .collect()
}

/// What a screen reader calls a picture message: `Photo` or `Sticker`; `None` for any other.
fn picture_word(message: &Message) -> Option<&'static str> {
    match message.kind.as_str() {
        "image" => Some("Photo"),
        "sticker" => Some("Sticker"),
        _ => None,
    }
}

/// The picture of an image or sticker message of the open chat; `None` for any other message,
/// and for a picture with no id yet (nothing can be fetched for it).
fn picture_view(model: &Model, message: &Message) -> Option<PictureView> {
    if !is_picture(&message.kind) || message.id.is_empty() {
        return None;
    }
    let chat = model.selected_jid()?.to_owned();
    let sticker = message.kind == "sticker";
    let size = match (message.width, message.height) {
        (Some(width), Some(height)) => Some(crate::pictures::picture_box(width, height, sticker)),
        _ => None,
    };
    let kind = if sticker { "Sticker" } else { "Photo" };
    let alt = if message.text.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind}, {}", message.text)
    };
    Some(PictureView {
        full: model
            .transcript()
            .full_picture(&chat, &message.id)
            .map(PathBuf::from),
        chat,
        id: message.id.clone(),
        sticker,
        animated: sticker && message.animated,
        size,
        thumb: message.thumb.is_some(),
        alt,
    })
}

/// The connection as the headers say it: `reconnecting…` while the driver is, else the session,
/// followed by why WhatsApp stopped it when it did (fixed text, never WhatsApp's own).
pub fn connection(model: &Model) -> String {
    if !model.connected() {
        return "reconnecting…".to_owned();
    }
    let status = model.status();
    match status.stop_reason {
        Some(reason) => format!("{} · {}", status.session.as_str(), reason.describe()),
        None => status.session.as_str().to_owned(),
    }
}

/// The chat list's header: the app, the connection and the unread total.
pub fn sidebar_header(model: &Model) -> Header {
    let unread = model.status().unread;
    let subtitle = if unread > 0 {
        format!("{} · {unread} unread", connection(model))
    } else {
        connection(model)
    };
    Header {
        title: "nuncio".to_owned(),
        subtitle,
    }
}

/// The conversation's header: the selected chat and the connection.
pub fn conversation_header(model: &Model) -> Header {
    Header {
        title: model
            .selected_chat()
            .map_or_else(|| "No chat".to_owned(), chat_title),
        subtitle: connection(model),
    }
}

/// The banners around the draft.
pub fn composer_view(model: &Model) -> ComposerView {
    let replying = model
        .composer()
        .reply_to()
        .map(|id| match quoted(model.loaded(), id) {
            Some(message) => format!("Replying to: {}", one_line(message)),
            None => "Replying to a message".to_owned(),
        });
    let blocked = model.chat_list().orphan().map(|_| MERGED_NOTICE.to_owned());
    ComposerView { replying, blocked }
}

/// The search's results as one line each (`HH:MM  text`), while the search is open.
pub fn search_view(model: &Model, offset_minutes: i32) -> Option<SearchView> {
    let search = model.transcript().search()?;
    let results = search.results().map(|messages| {
        messages
            .iter()
            .map(|m| format!("{}  {}", local_hhmm(m, offset_minutes), one_line(m)))
            .collect()
    });
    Some(SearchView {
        awaiting: search.is_awaiting(),
        results,
        selected: search.selected(),
    })
}

/// Whether the window shows its title bar (window controls and title): `auto` hides it on a
/// tiling desktop, Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set, or Hyprland in
/// `XDG_CURRENT_DESKTOP`), as Sealwax does.
pub fn shows_title_bar(
    choice: TitleBar,
    hyprland_signature: Option<&OsStr>,
    current_desktop: Option<&str>,
) -> bool {
    match choice {
        TitleBar::Shown => true,
        TitleBar::Hidden => false,
        TitleBar::Auto => {
            hyprland_signature.is_none()
                && !current_desktop.is_some_and(|desktop| desktop.contains("Hyprland"))
        }
    }
}

#[cfg(test)]
mod tests;
