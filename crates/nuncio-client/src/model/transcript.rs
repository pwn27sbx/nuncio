//! The loaded conversations and everything pending about them: my sends waiting for a reply,
//! sends whose acknowledgement timed out (not confirmed), the selected message, the chat being
//! opened or resynced, media downloads, and the in-chat search.
//!
//! [`Transcript`] holds that state and its per-conversation rules; [`Model`](super::Model) folds
//! the daemon's frames into it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::wire::{Message, MessageState, Reaction, SendState};

/// The kinds a client draws as pictures.
pub fn is_picture(kind: &str) -> bool {
    matches!(kind, "image" | "sticker")
}

/// The message `id` among `messages` (what a reply quotes), if it is loaded.
pub fn quoted<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
    id: &str,
) -> Option<&'a Message> {
    messages.into_iter().find(|m| m.id == id)
}

/// Whether `a` and `b` are the same message: the same WhatsApp id, or the same `client_id` (a send
/// of mine before it has an id).
fn same_message(a: &Message, b: &Message) -> bool {
    (!a.id.is_empty() && b.id == a.id) || (a.client_id.is_some() && b.client_id == a.client_id)
}

/// The in-chat search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub(crate) query: String,
    pub(crate) editing: bool,
    pub(crate) awaiting: bool,
    pub(crate) results: Option<Vec<Message>>,
    pub(crate) selected: Option<usize>,
}

impl Search {
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The query is being typed (otherwise the results are being moved through).
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// A `search` was sent; the next `messages` reply for the chat is its results.
    pub fn is_awaiting(&self) -> bool {
        self.awaiting
    }

    /// The results, newest first as the daemon answers; `None` before the first search.
    pub fn results(&self) -> Option<&[Message]> {
        self.results.as_deref()
    }

    /// The chosen result. Moving goes past the ends as a list widget's state does (one past the
    /// last, or `usize::MAX` from none): the view clamps it to what it draws and hands it back
    /// with [`Model::set_result_selected`](super::Model::set_result_selected).
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
}

/// The transcript (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    /// Loaded conversations by JID, oldest first.
    pub(crate) conversations: HashMap<String, Vec<Message>>,
    /// `client_id`s of sends that have no reply yet.
    pub(crate) sending: HashSet<String>,
    /// `client_id`s of my sends whose acknowledgement timed out (`not_confirmed`): they may have
    /// been delivered, so they are shown as not confirmed rather than not sent.
    pub(crate) unconfirmed: HashSet<String>,
    /// The selected message: an index into the open chat's loaded conversation.
    pub(crate) cursor: Option<usize>,
    /// The chat opened whose `messages` reply has not come yet.
    pub(crate) opening: Option<String>,
    /// The chat whose `messages` were asked for again after a reconnect: its reply keeps the
    /// selected message and the sends the daemon never stored.
    pub(crate) resyncing: Option<String>,
    /// Ids of media messages whose download was asked for to open externally, not answered yet.
    pub(crate) downloading: HashSet<String>,
    /// Ids of pictures whose full file was asked for to show inline, not answered yet.
    pub(crate) showing: HashSet<String>,
    /// The full file of a picture fetched to show inline, by chat JID and message id.
    pub(crate) full: HashMap<(String, String), PathBuf>,
    pub(crate) search: Option<Search>,
}

impl Transcript {
    /// The loaded conversation of chat `jid`, oldest first; `None` before it is loaded.
    pub fn conversation(&self, jid: &str) -> Option<&[Message]> {
        self.conversations.get(jid).map(Vec::as_slice)
    }

    /// The selected message's index in the open chat's loaded conversation.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Whether `message` is a send of mine still waiting for the daemon's reply.
    pub fn is_sending(&self, message: &Message) -> bool {
        message
            .client_id
            .as_ref()
            .is_some_and(|id| self.sending.contains(id))
    }

    /// Whether `message` is a send of mine shown as not sent whose acknowledgement only timed
    /// out: it may have been delivered, so it is "not confirmed".
    pub fn is_unconfirmed(&self, message: &Message) -> bool {
        message.state == Some(MessageState::NotSent)
            && message
                .client_id
                .as_ref()
                .is_some_and(|id| self.unconfirmed.contains(id))
    }

    /// The chat opened whose messages have not come yet.
    pub fn opening(&self) -> Option<&str> {
        self.opening.as_deref()
    }

    /// The chat whose messages were asked for again after a reconnect.
    pub fn resyncing(&self) -> Option<&str> {
        self.resyncing.as_deref()
    }

    /// Whether the download of media message `id`, to open it externally, is pending.
    pub fn is_downloading(&self, id: &str) -> bool {
        self.downloading.contains(id)
    }

    /// Whether the full picture of message `id`, to show inline, is being fetched.
    pub fn is_showing(&self, id: &str) -> bool {
        self.showing.contains(id)
    }

    /// The fetched full picture of message `id` in chat `jid`.
    pub fn full_picture(&self, jid: &str, id: &str) -> Option<&Path> {
        self.full
            .get(&(jid.to_owned(), id.to_owned()))
            .map(PathBuf::as_path)
    }

    /// Every fetched full picture: chat JID, message id, file.
    pub fn full_pictures(&self) -> impl Iterator<Item = (&str, &str, &Path)> {
        self.full
            .iter()
            .map(|((jid, id), path)| (jid.as_str(), id.as_str(), path.as_path()))
    }

    /// The in-chat search, while it is open.
    pub fn search(&self) -> Option<&Search> {
        self.search.as_ref()
    }

    /// The JID and index of my send with this `client_id`.
    pub(crate) fn locate_send(&self, client_id: &str) -> Option<(String, usize)> {
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

    /// My send `client_id` was refused or never reached the daemon: not sent.
    pub(crate) fn send_failed(&mut self, client_id: &str) {
        self.sending.remove(client_id);
        self.unconfirmed.remove(client_id);
        if let Some(message) = self.find_send(client_id) {
            message.state = Some(MessageState::NotSent);
        }
    }

    /// My send `client_id` was answered `not_confirmed`: shown as not confirmed.
    pub(crate) fn not_confirmed(&mut self, client_id: &str) {
        self.sending.remove(client_id);
        let Some(message) = self.find_send(client_id) else {
            return;
        };
        message.state = Some(MessageState::NotSent);
        self.unconfirmed.insert(client_id.to_owned());
    }

    /// Apply a `sent` reply to my send.
    pub(crate) fn settle(
        &mut self,
        client_id: &str,
        state: &SendState,
        id: Option<&str>,
        at: Option<&str>,
    ) {
        self.sending.remove(client_id);
        self.unconfirmed.remove(client_id);
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

    /// The marks an `event_message` about a send of mine changes: it is no longer waiting, and
    /// only a later queued or sent state clears a not-confirmed mark (the daemon publishes a
    /// not-confirmed send as `not_sent`, before or after its reply).
    pub(crate) fn send_event(&mut self, message: &Message) {
        if let Some(client_id) = &message.client_id {
            self.sending.remove(client_id);
            if message.state != Some(MessageState::NotSent) {
                self.unconfirmed.remove(client_id);
            }
        }
    }

    /// A new message in the loaded conversation `jid`, or a new state of one already there; true
    /// when it was added as a new message. Nothing happens to a conversation not loaded.
    pub(crate) fn add_or_update(&mut self, jid: &str, message: &Message) -> bool {
        let Some(messages) = self.conversations.get_mut(jid) else {
            return false;
        };
        match messages.iter_mut().find(|m| same_message(message, m)) {
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

    /// My reaction to message `id` of chat `jid` is now `emoji` (empty: removed); it goes first.
    pub(crate) fn reacted(&mut self, jid: &str, id: &str, emoji: &str) {
        let target = self
            .conversations
            .get_mut(jid)
            .and_then(|messages| messages.iter_mut().find(|m| m.id == id));
        if let Some(message) = target {
            message.reactions.retain(|r| !r.from_me);
            if !emoji.is_empty() {
                let mine = Reaction {
                    emoji: emoji.to_owned(),
                    from_me: true,
                };
                message.reactions.insert(0, mine);
            }
        }
    }

    /// A `messages` reply loads chat `jid` (`newest_first`, as the daemon answers). Sends still
    /// waiting for a reply are not stored yet, so they stay; after a reconnect (`resynced`), so do
    /// sends that never reached the daemon (not sent), until the user retries them. The new
    /// conversation, oldest first, and the index the previously selected message (`selected_id`)
    /// now has, when a resync keeps it.
    pub(crate) fn load(
        &mut self,
        jid: &str,
        newest_first: &[Message],
        resynced: bool,
        selected_id: Option<String>,
    ) -> (Vec<Message>, Option<usize>) {
        let mut loaded: Vec<Message> = newest_first.iter().rev().cloned().collect();
        if let Some(old) = self.conversations.get(jid) {
            let unstored = |m: &Message| {
                resynced
                    && m.id.is_empty()
                    && m.client_id
                        .as_ref()
                        .is_some_and(|id| !loaded.iter().any(|l| l.client_id.as_ref() == Some(id)))
            };
            let kept: Vec<Message> = old
                .iter()
                .filter(|m| self.is_sending(m) || unstored(m))
                .cloned()
                .collect();
            loaded.extend(kept);
        }
        let cursor = selected_id
            .filter(|id| resynced && !id.is_empty())
            .and_then(|id| loaded.iter().position(|m| m.id == id));
        (loaded, cursor)
    }

    /// Move everything keyed by the chat `alias` to `canonical`.
    pub(crate) fn rename_chat(&mut self, alias: &str, canonical: &str) {
        if let Some(moved) = self.conversations.remove(alias) {
            let into = self.conversations.entry(canonical.to_owned()).or_default();
            for message in moved {
                if !into.iter().any(|m| same_message(&message, m)) {
                    into.push(message);
                }
            }
        }
        let moved: Vec<(String, String)> = self
            .full
            .keys()
            .filter(|(jid, _)| jid == alias)
            .cloned()
            .collect();
        for key in moved {
            if let Some(path) = self.full.remove(&key) {
                self.full.insert((canonical.to_owned(), key.1), path);
            }
        }
        for pending in [&mut self.opening, &mut self.resyncing] {
            if pending.as_deref() == Some(alias) {
                *pending = Some(canonical.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine(client_id: &str, state: Option<MessageState>) -> Message {
        let mut message: Message = serde_json::from_value(serde_json::json!({
            "id": "", "kind": "text", "text": "hi", "from_me": true, "timestamp": "",
        }))
        .expect("a message");
        message.client_id = Some(client_id.to_owned());
        message.state = state;
        message
    }

    fn with_send(client_id: &str) -> Transcript {
        let mut transcript = Transcript::default();
        transcript
            .conversations
            .insert("a@lid".to_owned(), vec![mine(client_id, None)]);
        transcript.sending.insert(client_id.to_owned());
        transcript
    }

    #[test]
    fn a_not_sent_event_keeps_the_not_confirmed_mark_and_a_sent_one_clears_it() {
        let mut transcript = with_send("c-1");
        transcript.not_confirmed("c-1");
        let shown = |t: &Transcript| t.is_unconfirmed(&t.conversations["a@lid"][0]);
        assert!(shown(&transcript));
        transcript.send_event(&mine("c-1", Some(MessageState::NotSent)));
        assert!(shown(&transcript), "the daemon publishes it as not_sent");
        transcript.send_event(&mine("c-1", Some(MessageState::Queued)));
        assert!(!transcript.unconfirmed.contains("c-1"));
    }

    #[test]
    fn a_resync_keeps_a_send_the_daemon_never_stored() {
        let mut transcript = with_send("c-1");
        transcript.send_failed("c-1");
        let (loaded, _) = transcript.load("a@lid", &[], true, None);
        assert_eq!(loaded.len(), 1, "the not sent send stays until retried");
        let (loaded, _) = transcript.load("a@lid", &[], false, None);
        assert!(
            loaded.is_empty(),
            "outside a resync only waiting sends stay"
        );
    }

    #[test]
    fn my_reaction_replaces_my_old_one_and_goes_first() {
        let mut transcript = Transcript::default();
        let mut message = mine("c-1", None);
        message.id = "M1".to_owned();
        message.reactions = vec![
            Reaction {
                emoji: "😂".to_owned(),
                from_me: false,
            },
            Reaction {
                emoji: "👍".to_owned(),
                from_me: true,
            },
        ];
        transcript
            .conversations
            .insert("a@lid".to_owned(), vec![message]);
        transcript.reacted("a@lid", "M1", "🙏");
        let emoji: Vec<&str> = transcript.conversations["a@lid"][0]
            .reactions
            .iter()
            .map(|r| r.emoji.as_str())
            .collect();
        assert_eq!(emoji, ["🙏", "😂"]);
        transcript.reacted("a@lid", "M1", "");
        assert_eq!(transcript.conversations["a@lid"][0].reactions.len(), 1);
    }
}
