//! What the window shows, as plain data: the chat list's rows, the transcript's bubbles, the
//! headers, the composer's banners and the search results, each read from the shared
//! [`Model`]. Every rule (which chat is selected, what state a send is in, what can be replied
//! to) is the model's; this only words it, so the widgets stay a thin drawing of these values
//! and the wording is tested without GTK.

use std::ffi::OsStr;
use std::path::PathBuf;

use nuncio_client::model::{MERGED_NOTICE, Model, chat_title, is_picture, quoted};
use nuncio_client::wire::MessageState;
use nuncio_client::{Chat, Message, Session, TitleBar};

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
    /// The last message's local `HH:MM`, as the row's label says it.
    pub time: String,
    /// The time as the row shows it (plan §6.2): `HH:MM` today, `Yesterday`, else the date.
    pub when: String,
    /// The count chip: the unread count, only when it is more than one (plan §5); one unread
    /// message is told by the marker and the name's tone alone.
    pub chip: Option<String>,
    /// A group chat: a glyph before the name.
    pub group: bool,
    /// Who wrote the last message of a group that is not muted, before the preview.
    pub author: Option<String>,
    /// How my last message stands, as a glyph before the preview; `None` when it is not mine.
    pub ticks: Option<&'static str>,
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
    /// A document message's file, drawn as a row in the bubble.
    pub document: Option<DocumentView>,
    /// Who wrote the quoted message, when it is loaded.
    pub quote_author: Option<String>,
    /// The day label drawn above the first bubble of each day (plan §6.3).
    pub day: Option<String>,
    /// The bubble starts an author group: its author line is drawn (theirs), with the wider gap.
    pub starts_group: bool,
    /// The meta line (time and state) is drawn under it: it ends its group, or its send is not
    /// sent yet or failed, which never waits for a later bubble to be said.
    pub shows_meta: bool,
    /// What a screen reader says for the bubble.
    pub label: String,
}

/// A document message's file, as its row in the bubble shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentView {
    /// The file's name, or `Document` when the message names none.
    pub name: String,
    /// Its size, as `12 KB`.
    pub size: String,
}

/// One reaction chip under a bubble: an emoji and how many sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionChip {
    pub emoji: String,
    pub count: usize,
    /// One of them is mine.
    pub mine: bool,
}

/// What an empty pane says (plan §6.3): a title line and one line of hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyState {
    pub title: &'static str,
    pub hint: &'static str,
}

/// A local calendar day, as days since 1970-01-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day(i64);

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

/// What the composer shows around the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerView {
    /// `Replying to …`, while the draft quotes a message.
    pub replying: Option<String>,
    /// Why nothing can be sent now (the open chat vanished: it was merged).
    pub blocked: Option<String>,
    /// The Send button, beside the draft.
    pub send: SendButton,
}

/// The composer's Send button (plan §6.4): shown only when there is text to send, so a screen
/// never has two filled buttons, and saying `Sending…`, not pressable, while a send of the open
/// chat waits for the daemon and nothing new is typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendButton {
    Hidden,
    Send,
    Sending,
}

impl SendButton {
    /// The button's label, when it is shown.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Hidden => None,
            Self::Send => Some("Send"),
            Self::Sending => Some("Sending…"),
        }
    }
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

/// A group chat's JID ends in `@g.us`.
fn is_group(jid: &str) -> bool {
    jid.ends_with("@g.us")
}

/// The glyph for how my send `message` stands in the list (plan §6.2); `None` when not mine.
fn ticks(message: &Message) -> Option<&'static str> {
    message.from_me.then_some(match message.state {
        Some(MessageState::Queued) => "◷",
        Some(MessageState::NotSent) => "✗",
        Some(MessageState::Unknown) | None => "✓",
    })
}

fn chat_row(chat: &Chat, offset_minutes: i32, today: Option<Day>) -> ChatRow {
    let title = chat_title(chat);
    let unread = (chat.unread > 0).then(|| chat.unread.to_string());
    let (preview, time) = match &chat.last_message {
        Some(message) => (one_line(message), local_hhmm(message, offset_minutes)),
        None => ("(no messages)".to_owned(), String::new()),
    };
    let group = is_group(&chat.jid);
    // In a group that is not muted, who wrote the last message, when the daemon says it.
    let author = chat
        .last_message
        .as_ref()
        .filter(|_| group && !chat.muted)
        .and_then(
            |message| match (message.from_me, message.sender.as_deref()) {
                (true, _) => Some("You".to_owned()),
                (false, Some(sender)) if !sender.is_empty() => Some(sender.to_owned()),
                (false, _) => None,
            },
        );
    let day = chat
        .last_message
        .as_ref()
        .and_then(|message| local_seconds(message, offset_minutes))
        .map(day_at);
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
    match (&chat.last_message, &author) {
        (Some(_), Some(author)) => {
            label.push_str(&format!(". Last message at {time}: {author}: {preview}"));
        }
        (Some(_), None) => label.push_str(&format!(". Last message at {time}: {preview}")),
        (None, _) => label.push_str(". No messages"),
    }
    ChatRow {
        jid: chat.jid.clone(),
        title,
        unread,
        pinned: chat.pinned,
        muted: chat.muted,
        archived: chat.archived,
        preview,
        when: row_when(&time, day, today),
        time,
        chip: (chat.unread > 1).then(|| chat.unread.to_string()),
        group,
        author,
        ticks: chat.last_message.as_ref().and_then(ticks),
        label,
    }
}

/// The chat list's rows, in the model's order (archived chats last).
pub fn chat_rows(model: &Model, offset_minutes: i32, today: Option<Day>) -> Vec<ChatRow> {
    model
        .chat_list()
        .chats()
        .iter()
        .map(|chat| chat_row(chat, offset_minutes, today))
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

/// Who wrote `message`, as a bubble names her: `You`, the group member the daemon names in
/// `sender`, or `theirs` (the chat's title).
fn author(message: &Message, theirs: &str) -> String {
    match (message.from_me, message.sender.as_deref()) {
        (true, _) => "You".to_owned(),
        (false, Some(sender)) if !sender.is_empty() => sender.to_owned(),
        (false, _) => theirs.to_owned(),
    }
}

/// The selected chat's loaded conversation as bubbles, oldest first, grouped by author and day
/// (plan §6.3); `today` words the day labels (`Today`, `Yesterday`).
pub fn bubbles(model: &Model, offset_minutes: i32, today: Option<Day>) -> Vec<Bubble> {
    let messages = model.loaded();
    let theirs = model
        .selected_chat()
        .map_or_else(|| "them".to_owned(), chat_title);
    let cursor = model.transcript().cursor();
    let authors: Vec<String> = messages.iter().map(|m| author(m, &theirs)).collect();
    let places = grouping(messages, &authors, offset_minutes, today);
    messages
        .iter()
        .zip(places)
        .enumerate()
        .map(|(index, (message, (day, starts_group, ends_group)))| {
            let mark = mark(model, message);
            let sender = authors[index].clone();
            let time = local_hhmm(message, offset_minutes);
            let original = message.reply_to.as_deref().map(|id| quoted(messages, id));
            let quote = original.map(|original| match original {
                Some(original) => one_line(original),
                None => "(a message not loaded)".to_owned(),
            });
            let quote_author = original.flatten().map(|original| author(original, &theirs));
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
                document: document_view(message),
                quote_author,
                day,
                starts_group,
                // A send not sent yet, or failed, says so under its own bubble.
                shows_meta: ends_group || mark.is_some_and(|mark| mark != SendMark::Sent),
                label,
            }
        })
        .collect()
}

/// Consecutive bubbles of one author at most this far apart share a group (plan §6.3).
pub const GROUP_GAP_SECONDS: i64 = 5 * 60;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

impl Day {
    /// The day `year`-`month`-`day` (Howard Hinnant's `days_from_civil`).
    pub fn from_civil(year: i32, month: u32, day: u32) -> Self {
        let year = i64::from(year) - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let month = i64::from(month);
        let day_of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        Self(era * 146_097 + day_of_era - 719_468)
    }

    /// Its year, month (1–12) and day of the month (`civil_from_days`).
    fn civil(self) -> (i64, usize, i64) {
        let z = self.0 + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let mp = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        (year, usize::try_from(month).unwrap_or(1), day)
    }

    /// `Tue 20 Oct`, with the year when `year` is set.
    fn date(self, year: bool) -> String {
        let (y, month, day) = self.civil();
        let weekday = WEEKDAYS[usize::try_from(self.0.rem_euclid(7)).unwrap_or_default()];
        let month = MONTHS[month - 1];
        if year {
            format!("{weekday} {day} {month} {y}")
        } else {
            format!("{weekday} {day} {month}")
        }
    }
}

/// The message's local time as seconds since 1970-01-01 at `offset_minutes` from UTC; `None`
/// when its timestamp is not RFC 3339.
fn local_seconds(message: &Message, offset_minutes: i32) -> Option<i64> {
    let (date, time) = message.timestamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day))) = (date.next(), date.next(), date.next())
    else {
        return None;
    };
    let field = |range: std::ops::Range<usize>| time.get(range)?.parse::<i64>().ok();
    let (hours, minutes) = (field(0..2)?, field(3..5)?);
    let seconds = field(6..8).unwrap_or(0);
    let day = Day::from_civil(
        i32::try_from(year).ok()?,
        u32::try_from(month).ok().filter(|m| (1..=12).contains(m))?,
        u32::try_from(day).ok().filter(|d| (1..=31).contains(d))?,
    );
    Some(day.0 * 86_400 + hours * 3600 + minutes * 60 + seconds + i64::from(offset_minutes) * 60)
}

fn day_at(seconds: i64) -> Day {
    Day(seconds.div_euclid(86_400))
}

/// A day as its label says it (plan §6.3): `Today`, `Yesterday`, `Tue 20 Oct` within this year,
/// else with its year (and always with it when today is not known).
pub fn day_label(day: Day, today: Option<Day>) -> String {
    match today {
        Some(today) if day == today => "Today".to_owned(),
        Some(today) if day.0 + 1 == today.0 => "Yesterday".to_owned(),
        Some(today) => day.date(today.civil().0 != day.civil().0),
        None => day.date(true),
    }
}

/// The time a chat row shows (plan §6.2): its `time` today (or when the day is not known),
/// `Yesterday`, else `20 Oct`.
fn row_when(time: &str, day: Option<Day>, today: Option<Day>) -> String {
    match (day, today) {
        (Some(day), Some(today)) if day.0 + 1 == today.0 => "Yesterday".to_owned(),
        (Some(day), Some(today)) if day != today => {
            let (_, month, date) = day.civil();
            format!("{date} {}", MONTHS[month - 1])
        }
        _ => time.to_owned(),
    }
}

/// The meta line under a bubble (plan §6.3): its time, and the state of a send of mine.
pub fn meta_line(bubble: &Bubble) -> String {
    match bubble.mark {
        Some(mark) => format!("{} · {}", bubble.time, mark.as_str()),
        None => bubble.time.clone(),
    }
}

/// The reaction chips under a bubble: one per emoji, in the order first seen, with its count.
pub fn reaction_chips(reactions: &[ReactionView]) -> Vec<ReactionChip> {
    let mut chips: Vec<ReactionChip> = Vec::new();
    for reaction in reactions {
        match chips.iter_mut().find(|chip| chip.emoji == reaction.emoji) {
            Some(chip) => {
                chip.count += 1;
                chip.mine |= reaction.mine;
            }
            None => chips.push(ReactionChip {
                emoji: reaction.emoji.clone(),
                count: 1,
                mine: reaction.mine,
            }),
        }
    }
    chips
}

/// What the transcript says when it has no bubble: why. `None` while the open chat's messages
/// are on their way, and when there are bubbles.
pub fn transcript_empty(model: &Model) -> Option<EmptyState> {
    let jid = model.selected_jid();
    let loaded = jid.and_then(|jid| model.transcript().conversation(jid));
    let hint = match loaded {
        Some(messages) if !messages.is_empty() => return None,
        Some(_) => "This chat has no messages",
        None if jid.is_some() && model.transcript().opening() == jid => return None,
        None => "Pick a chat to read it here",
    };
    Some(EmptyState {
        title: "No messages yet",
        hint,
    })
}

/// What the chat list says when `filter` keeps no chat.
pub fn list_empty(filter: Filter) -> EmptyState {
    match filter {
        Filter::All => EmptyState {
            title: "No chats",
            hint: "Chats appear here as the phone syncs them",
        },
        Filter::Unread => EmptyState {
            title: "No unread chats",
            hint: "Every chat is read",
        },
        Filter::Archived => EmptyState {
            title: "No archived chats",
            hint: "Chats archived on the phone are listed here",
        },
    }
}

/// A file size as a document row shows it: `900 B`, `12 KB`, `1.2 MB`.
fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // Display only: a size in tenths is exact enough as a float.
    #[allow(clippy::cast_precision_loss)]
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// A document message's row; `None` for any other kind.
fn document_view(message: &Message) -> Option<DocumentView> {
    if message.kind != "document" {
        return None;
    }
    let media = message.media.as_ref();
    let name = media
        .map(|media| media.file_name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Document".to_owned());
    Some(DocumentView {
        name,
        size: media.map_or_else(String::new, |media| file_size(media.size)),
    })
}

/// Where each bubble stands in its day and author group (plan §6.3), from the messages' local
/// times: `(day label, starts a group, ends a group)`.
fn grouping(
    messages: &[Message],
    authors: &[String],
    offset_minutes: i32,
    today: Option<Day>,
) -> Vec<(Option<String>, bool, bool)> {
    let times: Vec<Option<i64>> = messages
        .iter()
        .map(|message| local_seconds(message, offset_minutes))
        .collect();
    let starts: Vec<bool> = (0..messages.len())
        .map(|i| {
            let Some(before) = i.checked_sub(1) else {
                return true;
            };
            let same_author =
                messages[i].from_me == messages[before].from_me && authors[i] == authors[before];
            let close = match (times[before], times[i]) {
                (Some(before), Some(now)) => {
                    day_at(before) == day_at(now)
                        && (0..=GROUP_GAP_SECONDS).contains(&(now - before))
                }
                _ => false,
            };
            !(same_author && close)
        })
        .collect();
    let mut last_day = None;
    (0..messages.len())
        .map(|i| {
            let day = times[i].map(day_at);
            let label = (day.is_some() && day != last_day)
                .then(|| day.map(|day| day_label(day, today)))
                .flatten();
            if day.is_some() {
                last_day = day;
            }
            let ends = starts.get(i + 1).copied().unwrap_or(true);
            (label, starts[i], ends)
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

/// A filter of the rail (plan §5): which chats the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Unread,
    Archived,
}

impl Filter {
    /// The rail's filters, in its order.
    pub const EACH: [Self; 3] = [Self::All, Self::Unread, Self::Archived];

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Archived => "Archived",
        }
    }

    fn keeps(self, chat: &Chat) -> bool {
        match self {
            Self::All => true,
            Self::Unread => chat.unread > 0,
            Self::Archived => chat.archived,
        }
    }
}

/// The positions in the model's chat list of the chats `filter` shows, in the list's order.
/// `opened_from` is the filter the open conversation was opened from in the list: while that
/// filter is the one shown, the open chat stays listed, so reading an unread chat does not pull
/// its row away from under the cursor; no other filter lists it for that.
pub fn listed(model: &Model, filter: Filter, opened_from: Option<Filter>) -> Vec<usize> {
    let kept = (opened_from == Some(filter))
        .then(|| open_jid(model))
        .flatten();
    model
        .chat_list()
        .chats()
        .iter()
        .enumerate()
        .filter(|(_, chat)| filter.keeps(chat) || kept == Some(chat.jid.as_str()))
        .map(|(position, _)| position)
        .collect()
}

/// How many chats `filter` keeps, as the rail counts them (never the open chat's exception).
pub fn count(model: &Model, filter: Filter) -> usize {
    listed(model, filter, None).len()
}

/// The chat whose conversation is open, if one is.
fn open_jid(model: &Model) -> Option<&str> {
    model.is_open().then(|| model.selected_jid()).flatten()
}

/// The connection as the rail says it: one short word, and whether the device is linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailStatus {
    /// `linked`, `offline`, `unlinked`…, or `reconnecting…` while the daemon connection is lost.
    pub word: String,
    /// Linked and connected: the rail shows the accent square.
    pub linked: bool,
}

/// The rail's connection line.
pub fn rail_status(model: &Model) -> RailStatus {
    if !model.connected() {
        return RailStatus {
            word: "reconnecting…".to_owned(),
            linked: false,
        };
    }
    let session = &model.status().session;
    RailStatus {
        word: session.as_str().to_owned(),
        linked: *session == Session::Linked,
    }
}

/// The transcript header: the selected chat's name, or `No chat`.
pub fn conversation_title(model: &Model) -> String {
    model
        .selected_chat()
        .map_or_else(|| "No chat".to_owned(), chat_title)
}

/// The one-line strip under the transcript header: why WhatsApp stopped the session, or why
/// nunciod keeps the device closed (fixed text, never WhatsApp's own), while that holds. None
/// while the daemon connection is lost: reconnecting is ours to say, in the rail.
pub fn stop_strip(model: &Model) -> Option<String> {
    if !model.connected() {
        return None;
    }
    model
        .status()
        .stop_reason
        .map(|reason| reason.describe().to_owned())
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
    // A blank draft sends nothing ([`Model::send`]), so it gets no button either.
    let send = if !model.composer().text().trim().is_empty() {
        SendButton::Send
    } else if model
        .loaded()
        .iter()
        .any(|message| model.transcript().is_sending(message))
    {
        SendButton::Sending
    } else {
        SendButton::Hidden
    };
    ComposerView {
        replying,
        blocked,
        send,
    }
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
