//! The command palette (F1 or Ctrl+K), without GTK: which commands it lists in the app's current
//! state, how a typed query filters them, and what choosing one does.
//!
//! - It lists every chat (by [`chat_title`], as the list names it), searching the open chat, the
//!   selected message's reply, react, retry and open (only those the message's menu would enable:
//!   the same [`Bubble`] `can_*` rules), the settings and Quit.
//! - A query keeps the commands whose title holds its characters in order, ignoring case and
//!   spaces ([`score`]); the best matches come first.
//! - Nothing in it sends a message: there is no send command, and opening a chat or a link only
//!   ever fills the composer. A retry is the user's own explicit choice, as Shift+R is.

use nuncio_client::model::{Model, chat_title};

use crate::controller::{Command, Controller};
use crate::present::{self, Bubble};

/// What a palette entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open the chat with this JID, as a click in the list does.
    OpenChat(String),
    /// Search the open chat (Ctrl+F).
    Search,
    /// Reply to this message.
    Reply(Target),
    /// Choose a reaction for this message (the picker; nothing is sent until one is chosen).
    React(Target),
    /// One more attempt of this send of mine that ended not sent.
    Retry(Target),
    /// Fetch this media message's file and open it outside the app.
    OpenMedia(Target),
    /// The settings window.
    Settings,
    /// Quit the app (also in background mode).
    Quit,
}

/// The message a command was chosen for: its chat and its id (a send with no id yet by its
/// `client_id`). It is found again when the command runs, so a command chosen after another
/// chat opened, or after the conversation moved, never acts on a different message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub jid: String,
    pub id: String,
    pub client_id: Option<String>,
}

impl Target {
    /// Where this message is in the open conversation now, if it is there.
    pub fn locate(&self, model: &Model) -> Option<usize> {
        if !model.is_open() || model.selected_jid() != Some(self.jid.as_str()) {
            return None;
        }
        model.loaded().iter().position(|message| {
            if self.id.is_empty() {
                self.client_id.is_some()
                    && message.id.is_empty()
                    && message.client_id == self.client_id
            } else {
                message.id == self.id
            }
        })
    }
}

/// One command of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    /// The key that does the same outside the palette.
    pub shortcut: Option<&'static str>,
    pub action: Action,
}

impl Entry {
    fn new(title: impl Into<String>, shortcut: Option<&'static str>, action: Action) -> Self {
        Self {
            title: title.into(),
            shortcut,
            action,
        }
    }

    /// What a screen reader says for the entry.
    pub fn label(&self) -> String {
        match self.shortcut {
            Some(key) => format!("{}, shortcut {key}", self.title),
            None => self.title.clone(),
        }
    }
}

/// The palette's commands now. `message` is the loaded message the user had selected (the
/// focused bubble) when the palette opened; its actions are listed only where its menu enables
/// them. The chats come last, in the list's order.
pub fn entries(model: &Model, message: Option<usize>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let bubble: Option<Bubble> = message
        .filter(|_| model.is_open())
        .and_then(|index| present::bubbles(model, 0).into_iter().nth(index));
    let target = bubble.as_ref().and_then(|bubble| {
        let message = model.loaded().get(bubble.index)?;
        Some(Target {
            jid: model.selected_jid()?.to_owned(),
            id: message.id.clone(),
            client_id: message.client_id.clone(),
        })
    });
    if let (Some(bubble), Some(target)) = (bubble, target) {
        if bubble.can_target {
            entries.push(Entry::new(
                "Reply to the selected message",
                Some("r"),
                Action::Reply(target.clone()),
            ));
            entries.push(Entry::new(
                "React to the selected message…",
                Some("e"),
                Action::React(target.clone()),
            ));
        }
        if bubble.can_retry {
            entries.push(Entry::new(
                "Retry the selected message",
                Some("Shift+R"),
                Action::Retry(target.clone()),
            ));
        }
        if bubble.can_open {
            entries.push(Entry::new(
                "Open the selected media",
                Some("o"),
                Action::OpenMedia(target.clone()),
            ));
        }
    }
    if model.is_open() && model.selected_chat().is_some() {
        entries.push(Entry::new(
            "Search in this chat",
            Some("Ctrl+F"),
            Action::Search,
        ));
    }
    entries.push(Entry::new("Settings", Some("Ctrl+,"), Action::Settings));
    entries.push(Entry::new("Quit", Some("Ctrl+Q"), Action::Quit));
    entries.extend(model.chat_list().chats().iter().map(|chat| {
        Entry::new(
            format!("Chat: {}", chat_title(chat)),
            None,
            Action::OpenChat(chat.jid.clone()),
        )
    }));
    entries
}

fn lower(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// How well `text` matches `query`: `None` unless every character of the query (spaces left out)
/// appears in `text` in order, ignoring case. A higher score is a better match: a character
/// counts more right after the previous match and more again at the start of a word. An empty
/// query matches everything with 0.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query: Vec<char> = lower(query)
        .into_iter()
        .filter(|c| !c.is_whitespace())
        .collect();
    let text = lower(text);
    let Some(&first) = query.first() else {
        return Some(0);
    };
    // Every place the first character could start the match, each followed greedily: the best.
    (0..text.len())
        .filter(|&start| text[start] == first)
        .filter_map(|start| score_from(&query, &text, start))
        .max()
}

/// The score of matching `query` in `text` with its first character at `start` and each next one
/// at its first place after the previous.
fn score_from(query: &[char], text: &[char], start: usize) -> Option<u32> {
    let mut total = 0;
    let mut from = start;
    let mut previous: Option<usize> = None;
    for &wanted in query {
        let at = from + text[from..].iter().position(|&c| c == wanted)?;
        let word_start = at == 0 || !text[at - 1].is_alphanumeric();
        total += 1;
        if previous.is_some_and(|p| p + 1 == at) {
            total += 3;
        }
        if word_start {
            total += 5;
        }
        previous = Some(at);
        from = at + 1;
    }
    Some(total)
}

/// The positions in `entries` of the commands `query` matches, best first; equal matches keep
/// their order.
pub fn filter(entries: &[Entry], query: &str) -> Vec<usize> {
    let mut matched: Vec<(usize, u32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| score(query, &entry.title).map(|s| (index, s)))
        .collect();
    matched.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
    matched.into_iter().map(|(index, _)| index).collect()
}

/// What the window does once a command is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A chat was opened: show the conversation and carry these out.
    Opened(Vec<Command>),
    /// Carry these out.
    Commands(Vec<Command>),
    /// Reply to this message (the composer takes the focus).
    Reply(usize),
    /// Show the reaction picker on this message.
    React(usize),
    Search,
    Settings,
    Quit,
    /// The message the command was chosen for is no longer in the open conversation (another
    /// chat opened meanwhile): nothing was done.
    Gone,
}

/// Do what `action` says, through the same controller calls as its key or click.
pub fn run(controller: &mut Controller, action: &Action) -> Step {
    // A message command acts on its message where it is now, or not at all.
    let index = match action {
        Action::Retry(target)
        | Action::OpenMedia(target)
        | Action::Reply(target)
        | Action::React(target) => match target.locate(controller.model()) {
            Some(index) => index,
            None => return Step::Gone,
        },
        _ => 0,
    };
    match action {
        Action::OpenChat(jid) => Step::Opened(controller.open_jid(jid)),
        Action::Retry(_) => Step::Commands(controller.retry(index)),
        Action::OpenMedia(_) => Step::Commands(controller.open_media(index)),
        Action::Reply(_) => Step::Reply(index),
        Action::React(_) => Step::React(index),
        Action::Search => Step::Search,
        Action::Settings => Step::Settings,
        Action::Quit => Step::Quit,
    }
}

#[cfg(test)]
mod tests;
