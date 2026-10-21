//! The view models a nuncio client shows, shared by the terminal client and the desktop app so
//! both follow the same verified rules. Pure: no I/O, no widgets. A UI turns user actions into
//! calls on [`Model`], sends the [`ClientFrame`]s it returns, and folds every reply and event back
//! in with [`Model::apply_reply`] and [`Model::apply`].
//!
//! - [`ChatList`]: the chats in the daemon's order, the selection by JID, one chat per contact
//!   (a phone-number chat merged into its LID chat is adopted, ADR 0001), the `mark_read`s in
//!   flight, and [`chat_title`].
//! - [`Transcript`]: the loaded conversations, my sends and their marks (sending, queued, not
//!   sent, not confirmed), reactions, quotes, the selected message, downloads and the search.
//! - [`Composer`]: the draft, edited by grapheme, and the message it replies to.
//!
//! The rules that span them live on [`Model`]: a draft never changes recipient (a chat that
//! vanished keeps the draft and blocks the send until the user chooses a chat), a reply is paired
//! with the request it answers before a chat is adopted, and a resync after a reconnect keeps the
//! selection and the sends the daemon never stored.

mod chats;
mod composer;
mod transcript;

use std::path::PathBuf;

pub(crate) use chats::title_from_jid;
pub use chats::{ChatList, MERGED_NOTICE, chat_title, is_alias_pair, sort_as_daemon};
pub use composer::Composer;
pub use transcript::{Search, Transcript, is_picture, quoted};

use crate::conn::Snapshot;
use crate::wire::{Chat, ClientFrame, DaemonFrame, Message, MessageState, Session};

/// How many messages a conversation loads (the wire maximum).
pub const MESSAGE_LIMIT: u16 = 200;
/// How many chats a resync asks for (the wire contract allows 1..200).
pub const CHAT_LIMIT: u16 = 200;
/// How many results an in-chat search asks for.
pub const SEARCH_LIMIT: u16 = 50;
/// The reactions a client's picker offers, in this order (an empty emoji removes mine).
pub const REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

/// What the UI must do after a frame was folded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send this request and fold its reply in with [`Model::apply_reply`].
    Request(ClientFrame),
    /// Open this downloaded media file with the desktop's opener.
    Open(PathBuf),
}

/// The daemon's state as the last `status` and events told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub daemon: String,
    pub linked: bool,
    pub session: Session,
    /// Unread messages in all chats.
    pub unread: u32,
}

/// A media download asked for by [`Model::download_selected`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub jid: String,
    pub id: String,
    /// To show the full picture inline (otherwise to open the file externally).
    pub inline: bool,
}

impl Download {
    /// The request that fetches it.
    pub fn frame(&self) -> ClientFrame {
        ClientFrame::download_media(self.jid.clone(), self.id.clone())
    }
}

/// Everything a client shows, changed only through its methods (see the module docs).
#[derive(Debug, Clone)]
pub struct Model {
    chats: ChatList,
    transcript: Transcript,
    composer: Composer,
    status: Status,
    /// A one-line notice for the user (an error from the daemon, a merged chat).
    notice: Option<String>,
    /// The selected chat's conversation is in use (the user is not in the chat list).
    open: bool,
    /// The UI's daemon connection is up.
    connected: bool,
    client_prefix: String,
    sends: u64,
    /// The view lost its place (another chat, or a send): a scrolled view goes back to the newest.
    view_reset: bool,
    /// The user can see the view ([`Model::set_seen`]).
    seen: bool,
}

impl Model {
    /// The model of `snapshot`, its chat list sorted with archived chats last (kept in the
    /// daemon's order otherwise). Sends get `client_id`s `<prefix>-1`, `<prefix>-2`, ...
    pub fn new(snapshot: Snapshot, client_prefix: impl Into<String>) -> Self {
        let Snapshot {
            daemon,
            linked,
            session,
            unread,
            chats,
        } = snapshot;
        let mut model = Self {
            chats: ChatList::default(),
            transcript: Transcript::default(),
            composer: Composer::default(),
            status: Status {
                daemon,
                linked,
                session,
                unread,
            },
            notice: None,
            open: false,
            connected: true,
            client_prefix: client_prefix.into(),
            sends: 0,
            view_reset: false,
            seen: true,
        };
        model.set_chats(chats);
        model
    }

    /// The prefix of the `client_id`s this model chooses. It must differ between runs, so a new
    /// send never repeats an old `client_id`.
    pub fn set_client_prefix(&mut self, prefix: impl Into<String>) {
        self.client_prefix = prefix.into();
    }

    pub fn chat_list(&self) -> &ChatList {
        &self.chats
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    /// The draft, to edit. Sending it is [`Model::send`].
    pub fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Take the notice to show it once (a toast), leaving none.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    /// Whether the selected chat's conversation is in use rather than the chat list.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The user went into the selected chat's conversation (`true`) or back to the chat list.
    /// A new message in an open chat is marked read at once, and a chat that vanishes while open
    /// keeps the selection.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Whether the user can see the view (a desktop window that is active and not minimized).
    pub fn is_seen(&self) -> bool {
        self.seen
    }

    /// The UI's window became seen (`true`) or hidden (`false`). A new message in the open chat
    /// is marked read only while the view is seen (read receipts only on an explicit view); seen
    /// again, the open chat's unread messages are marked read then. A terminal client never calls
    /// it: its view is always seen.
    pub fn set_seen(&mut self, seen: bool) -> Option<Effect> {
        self.seen = seen;
        let jid = self.chats.selected_jid()?.to_owned();
        let unread = self.chats.get(&jid).is_some_and(|chat| chat.unread > 0);
        (seen && unread && self.is_open_chat(&jid))
            .then(|| self.mark_read(&jid))
            .flatten()
    }

    /// Whether the view lost its place since the last call (another chat was selected, or a
    /// send was made): a view scrolled back into the conversation goes back to the newest.
    pub fn take_view_reset(&mut self) -> bool {
        std::mem::take(&mut self.view_reset)
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    /// The UI lost (`false`) or regained (`true`) its daemon connection. Nothing sent on the
    /// lost connection will be answered, so nothing waits for it.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
        if !connected {
            self.transcript.opening = None;
            self.transcript.resyncing = None;
            self.chats.marking.clear();
            self.transcript.showing.clear();
        }
    }

    /// What to ask the daemon again after a reconnect, since events were missed: the status,
    /// the chat list and the selected chat's messages. The draft, the selected chat and the
    /// selected message are kept.
    pub fn resync(&mut self) -> Vec<ClientFrame> {
        let mut frames = vec![ClientFrame::status(), ClientFrame::chats(CHAT_LIMIT)];
        self.transcript.resyncing = self.chats.selected_jid().map(str::to_owned);
        if let Some(jid) = &self.transcript.resyncing {
            frames.push(ClientFrame::messages(jid.clone(), MESSAGE_LIMIT));
        }
        frames
    }

    /// The selected chat (the vanished open chat while there is one).
    pub fn selected_chat(&self) -> Option<&Chat> {
        self.chats.selected_chat()
    }

    pub fn selected_jid(&self) -> Option<&str> {
        self.chats.selected_jid()
    }

    /// The user chose the chat at list position `index` (clamped to the list).
    pub fn select_chat(&mut self, index: usize) {
        let index = index.min(self.chats.chats.len().saturating_sub(1));
        // The user chose a chat: a vanished chat no longer reclaims the selection.
        self.chats.orphan = None;
        if index != self.chats.selected {
            self.view_reset = true;
            self.transcript.cursor = None;
        }
        self.chats.selected = index;
    }

    /// Select the next chat, stopping at the last one.
    pub fn select_next_chat(&mut self) {
        self.select_chat(self.chats.selected.saturating_add(1));
    }

    /// Select the previous chat, stopping at the first one.
    pub fn select_previous_chat(&mut self) {
        self.select_chat(self.chats.selected.saturating_sub(1));
    }

    /// Replace the chat list, archived chats last, keeping the selection on the same chat. Aliases
    /// the list names are adopted first ([`Model::adopt_listed`]). When the selected chat is gone
    /// (the daemon merged it into its LID chat), the selection follows it to the chat an alias or
    /// a reply named; otherwise, while its conversation is open or with a draft written, it stays
    /// on the vanished chat (`orphan`), with the draft intact, and nothing is sent until the user
    /// chooses a chat again: a draft never changes recipient.
    fn set_chats(&mut self, mut chats: Vec<Chat>) {
        self.adopt_listed(&chats);
        self.keep_pending(&mut chats);
        let selected = self.chats.selected_jid().map(str::to_owned);
        let previous = self.chats.selected_chat().cloned();
        self.chats.orphan = None;
        chats.sort_by_key(|chat| chat.archived);
        self.chats.chats = chats;
        let mut kept = selected.as_deref().and_then(|jid| self.chats.position(jid));
        if kept.is_none()
            && let Some(jid) = selected.as_deref()
        {
            let canonical = self.chats.aliases.get(jid).cloned();
            match canonical.and_then(|c| self.chats.position(&c).map(|i| (c, i))) {
                Some((canonical, index)) => {
                    self.rename_keys(jid, &canonical);
                    if self.notice.as_deref() == Some(MERGED_NOTICE) {
                        self.notice = None;
                    }
                    kept = Some(index);
                }
                // A draft never changes recipient, even from the chat list.
                None if self.open || !self.composer.is_empty() => {
                    self.chats.orphan = previous;
                    self.notice = Some(MERGED_NOTICE.to_owned());
                    return;
                }
                None => {}
            }
        }
        if kept.is_none() {
            self.view_reset = true;
            self.transcript.cursor = None;
        }
        self.chats.selected = kept.unwrap_or(0);
    }

    /// The daemon names in a chat's `aliases` the phone-number chats it merged into it (ADR 0001),
    /// in `chats` and in every `event_chat`. Each one this model holds anywhere (a list entry, the
    /// selected or vanished chat, a loaded conversation, a pending request) is adopted: its state
    /// moves to that chat, and [`Model::set_chats`] then keeps the selection on it. A number the
    /// same list also lists as its own chat is never adopted (a recycled number), and an earlier
    /// adoption of it is forgotten.
    fn adopt_listed(&mut self, chats: &[Chat]) {
        let listed = |jid: &str| chats.iter().any(|c| c.jid == jid);
        self.chats.aliases.retain(|alias, _| !listed(alias));
        let pairs: Vec<(String, String)> = chats
            .iter()
            .flat_map(|chat| {
                chat.aliases
                    .iter()
                    .filter(|alias| is_alias_pair(alias, &chat.jid) && !listed(alias))
                    .map(|alias| (alias.clone(), chat.jid.clone()))
            })
            .collect();
        for (alias, canonical) in pairs {
            if self.holds(&alias) {
                self.chats.aliases.insert(alias.clone(), canonical.clone());
                self.rename_keys(&alias, &canonical);
            }
        }
    }

    /// Keep in a new list `chats` the chats opened from outside the list that the daemon does
    /// not list yet ([`Model::open_jid`]); one it lists now, or lists as an alias of another
    /// chat, is no longer pending.
    fn keep_pending(&mut self, chats: &mut Vec<Chat>) {
        let aliases = &self.chats.aliases;
        self.chats.pending.retain(|jid| {
            !aliases.contains_key(jid)
                && !chats
                    .iter()
                    .any(|chat| chat.jid == *jid || chat.aliases.contains(jid))
        });
        let mut pending: Vec<&String> = self.chats.pending.iter().collect();
        pending.sort();
        for jid in pending {
            let entry = self.chats.get(jid).cloned();
            chats.push(entry.unwrap_or_else(|| chats::empty_chat(jid)));
        }
    }

    /// Whether the model holds anything keyed by the chat `jid`.
    fn holds(&self, jid: &str) -> bool {
        self.chats.position(jid).is_some()
            || self.chats.selected_jid() == Some(jid)
            || self.transcript.conversations.contains_key(jid)
            || self.chats.marking.contains(jid)
            || self.transcript.opening.as_deref() == Some(jid)
            || self.transcript.resyncing.as_deref() == Some(jid)
    }

    /// The daemon answered a request about the phone-number chat `alias` with its own reply naming
    /// the chat `canonical` (ADR 0001): from now on the model keys that conversation by
    /// `canonical`. Its list entry is renamed (or dropped when `canonical` is already listed), its
    /// loaded messages, pending `mark_read` and requests move, and a selection on it, or on the
    /// vanished open chat, follows it. When `canonical` is not listed (more chats than the list
    /// holds, or a number recycled after the merge), the vanished open chat stays vanished,
    /// renamed `canonical`, and nothing is sent until the user chooses a chat: the selection never
    /// falls to another list index.
    /// Only a phone-number JID answered as a LID JID is adopted. [`Model::apply_reply`] calls it,
    /// pairing a request with its own reply.
    pub fn adopt(&mut self, alias: &str, canonical: &str) {
        if !is_alias_pair(alias, canonical) {
            return;
        }
        self.chats
            .aliases
            .insert(alias.to_owned(), canonical.to_owned());
        let selected = self.chats.selected_jid() == Some(alias);
        let current = self.chats.selected_jid().map(str::to_owned);
        let vanished = self.chats.orphan.as_ref().is_some_and(|c| c.jid == alias);
        let previous = self.chats.selected_chat().cloned();
        if vanished {
            self.chats.orphan = None;
        }
        self.rename_keys(alias, canonical);
        self.chats.rename_entry(alias, canonical);
        let target = if selected {
            Some(canonical.to_owned())
        } else {
            current
        };
        // Keep the selection on the chat it was on; removing an entry can shift it.
        if self.chats.orphan.is_none() {
            let index = target.and_then(|jid| self.chats.position(&jid));
            // A listed selection on `alias` was renamed above, so only the vanished chat can
            // miss: the chat it became is not listed and the old index is now another contact.
            if index.is_none() && vanished {
                self.chats.orphan = previous.map(|chat| Chat {
                    jid: canonical.to_owned(),
                    ..chat
                });
                self.notice = Some(MERGED_NOTICE.to_owned());
                return;
            }
            if vanished && self.notice.as_deref() == Some(MERGED_NOTICE) {
                self.notice = None;
            }
            self.chats.selected = index.unwrap_or_else(|| {
                self.chats
                    .selected
                    .min(self.chats.chats.len().saturating_sub(1))
            });
        }
    }

    /// Fold the daemon's `reply` to `request` in, as [`Model::apply`] does. When the request named
    /// a phone-number chat and its own reply names a LID chat, the daemon served it as that
    /// contact's LID chat (ADR 0001), and the model adopts the LID JID first. Nothing else adopts
    /// a chat: a reply is paired with the request it answers, never guessed.
    pub fn apply_reply(&mut self, request: &ClientFrame, reply: &DaemonFrame) -> Option<Effect> {
        let asked = match request {
            ClientFrame::Messages { jid, .. }
            | ClientFrame::Search { jid, .. }
            | ClientFrame::React { jid, .. }
            | ClientFrame::MarkRead { jid, .. }
            | ClientFrame::DownloadMedia { jid, .. }
            | ClientFrame::Focus { jid, .. } => Some(jid),
            _ => None,
        };
        let answered = match reply {
            DaemonFrame::Messages { jid, .. }
            | DaemonFrame::Reacted { jid, .. }
            | DaemonFrame::MarkedRead { jid, .. }
            | DaemonFrame::MediaReady { jid, .. }
            | DaemonFrame::Focused { jid, .. } => Some(jid),
            _ => None,
        };
        if let (Some(asked), Some(answered)) = (asked, answered)
            && asked != answered
        {
            self.adopt(asked, answered);
        }
        self.apply(reply)
    }

    /// Move everything keyed by the chat `alias` to `canonical`.
    fn rename_keys(&mut self, alias: &str, canonical: &str) {
        self.transcript.rename_chat(alias, canonical);
        self.chats.rename_marking(alias, canonical);
    }

    /// The phone-number chat that `event_message` about `jid` answers: a send of mine (its exact
    /// `client_id`) made to that chat, published by the daemon under its LID chat.
    fn send_answered(&self, frame: &DaemonFrame) -> Option<(String, String)> {
        let DaemonFrame::EventMessage { jid, message, .. } = frame else {
            return None;
        };
        let (alias, _) = self.transcript.locate_send(message.client_id.as_deref()?)?;
        (alias != *jid).then(|| (alias, jid.clone()))
    }

    /// Open the selected chat: its conversation is in use and its messages are asked for. Closes
    /// the search.
    pub fn open_selected(&mut self) -> Option<ClientFrame> {
        let jid = self.chats.selected_jid()?.to_owned();
        self.open = true;
        self.transcript.search = None;
        self.transcript.opening = Some(jid.clone());
        Some(ClientFrame::messages(jid, MESSAGE_LIMIT))
    }

    /// Open the chat `jid` named from outside the chat list (a notification, a `whatsapp:` link):
    /// the listed chat, else the listed chat it was merged into (an adopted alias, or one a chat
    /// lists in its `aliases`), else a new empty entry for it, placed before the archived chats.
    /// A new entry stays listed through every new chat list until the daemon lists the chat (the
    /// first send creates it there) or a reply names the chat it became. Then it is opened as
    /// [`Model::open_selected`] does, so it is marked read once its messages are shown.
    pub fn open_jid(&mut self, jid: &str) -> Option<ClientFrame> {
        let previous = self.chats.selected_jid().map(str::to_owned);
        let index = match self.locate(jid) {
            Some(index) => index,
            None => {
                let at = self
                    .chats
                    .chats
                    .iter()
                    .position(|chat| chat.archived)
                    .unwrap_or(self.chats.chats.len());
                self.chats.chats.insert(at, chats::empty_chat(jid));
                self.chats.pending.insert(jid.to_owned());
                at
            }
        };
        self.select_chat(index);
        if self.chats.selected_jid() != previous.as_deref() {
            // An entry inserted at the old index is still another chat.
            self.view_reset = true;
            self.transcript.cursor = None;
        }
        self.open_selected()
    }

    /// The list position of the chat `jid` names: itself, else the chat it was merged into.
    fn locate(&self, jid: &str) -> Option<usize> {
        self.chats
            .position(jid)
            .or_else(|| {
                self.chats
                    .canonical(jid)
                    .and_then(|canonical| self.chats.position(canonical))
            })
            .or_else(|| {
                self.chats.chats.iter().position(|chat| {
                    is_alias_pair(jid, &chat.jid) && chat.aliases.iter().any(|alias| alias == jid)
                })
            })
    }

    /// Mark chat `jid` read here and on WhatsApp, unless a `mark_read` for it is in flight.
    fn mark_read(&mut self, jid: &str) -> Option<Effect> {
        self.chats
            .begin_mark_read(jid)
            .then(|| Effect::Request(ClientFrame::mark_read(jid)))
    }

    /// The chat whose conversation the user has open (not just selected in the list).
    fn is_open_chat(&self, jid: &str) -> bool {
        self.open && self.chats.selected_jid() == Some(jid)
    }

    /// The selected chat's loaded conversation, oldest first (empty before it is loaded).
    pub fn loaded(&self) -> &[Message] {
        self.chats
            .selected_jid()
            .and_then(|jid| self.transcript.conversation(jid))
            .unwrap_or(&[])
    }

    /// The selected message of the selected chat.
    pub fn selected_message(&self) -> Option<&Message> {
        self.loaded().get(self.transcript.cursor?)
    }

    /// Select the loaded message at `index`; false (nothing changes) when there is none.
    pub fn select_message(&mut self, index: usize) -> bool {
        let found = index < self.loaded().len();
        if found {
            self.transcript.cursor = Some(index);
        }
        found
    }

    /// The selected message's id, when WhatsApp has that message (received, or a send of mine
    /// that was sent), so it can be replied or reacted to. A send that is still sending, queued
    /// or not sent has no WhatsApp message to point at: the notice says so (`what` is the verb,
    /// e.g. "reacted to") and `None` is returned. No message selected is just `None`.
    pub fn target_id(&mut self, what: &str) -> Option<String> {
        let message = self.selected_message()?;
        if message.id.is_empty() || message.state.is_some() || self.transcript.is_sending(message) {
            self.notice = Some(format!("that message was not sent, so it cannot be {what}"));
            return None;
        }
        Some(message.id.clone())
    }

    /// Reply to the selected message: the draft quotes it. False when it cannot be replied to
    /// ([`Model::target_id`]).
    pub fn reply_to_selected(&mut self) -> bool {
        match self.target_id("replied to") {
            Some(id) => {
                self.composer.set_reply_to(id);
                true
            }
            None => false,
        }
    }

    /// React to the selected message with `emoji` (empty removes mine).
    pub fn react_selected(&mut self, emoji: &str) -> Option<ClientFrame> {
        let id = self.target_id("reacted to")?;
        let jid = self.chats.selected_jid()?.to_owned();
        Some(ClientFrame::react(jid, id, emoji))
    }

    /// Select the newest message, then one older each time; true when a message is selected.
    pub fn select_older(&mut self) -> bool {
        let len = self.loaded().len();
        if len > 0 {
            let cursor = self.transcript.cursor;
            self.transcript.cursor = Some(cursor.map_or(len - 1, |i| i.saturating_sub(1)));
        }
        len > 0
    }

    /// Select one newer message, stopping at the newest; true when a message was selected.
    pub fn select_newer(&mut self) -> bool {
        let len = self.loaded().len();
        match self.transcript.cursor {
            Some(i) => {
                self.transcript.cursor = Some((i + 1).min(len.saturating_sub(1)));
                true
            }
            None => false,
        }
    }

    /// One more attempt of the selected send that ended not sent. Never automatic, and one at a
    /// time: while an attempt waits for its answer, asking again asks for nothing.
    pub fn retry_selected(&mut self) -> Option<ClientFrame> {
        let message = self.selected_message()?;
        if message.state != Some(MessageState::NotSent) || self.transcript.is_sending(message) {
            return None;
        }
        let client_id = message.client_id.clone()?;
        self.transcript.sending.insert(client_id.clone());
        Some(ClientFrame::retry(client_id))
    }

    /// Ask for the selected media message's file: to show a picture in full inline when the UI
    /// draws pictures (`pictures`) and the user did not ask to open it `external`ly, else to open
    /// it once downloaded. A text message, one with no id, and a picture already being fetched
    /// ask for nothing. A new inline fetch forgets the picture fetched before.
    pub fn download_selected(&mut self, pictures: bool, external: bool) -> Option<Download> {
        let message = self.selected_message()?;
        if message.kind == "text" || message.id.is_empty() {
            return None;
        }
        let inline = !external && pictures && is_picture(&message.kind);
        let id = message.id.clone();
        let jid = self.chats.selected_jid()?.to_owned();
        if inline {
            if !self.transcript.showing.insert(id.clone()) {
                return None;
            }
            self.transcript.full.remove(&(jid.clone(), id.clone()));
        } else {
            self.transcript.downloading.insert(id.clone());
        }
        Some(Download { jid, id, inline })
    }

    /// Open the in-chat search on the selected chat, its query being typed; false with no chat.
    pub fn open_search(&mut self) -> bool {
        if self.chats.selected_chat().is_none() {
            return false;
        }
        self.transcript.search = Some(Search {
            editing: true,
            ..Search::default()
        });
        true
    }

    pub fn close_search(&mut self) {
        self.transcript.search = None;
    }

    /// The search, to change (no search, nothing).
    fn search_mut(&mut self) -> Option<&mut Search> {
        self.transcript.search.as_mut()
    }

    /// Type into the search query (only while it is being edited).
    pub fn search_type(&mut self, c: char) {
        if let Some(search) = self.search_mut().filter(|s| s.editing) {
            search.query.push(c);
        }
    }

    /// Erase the query's last character (only while it is being edited).
    pub fn search_backspace(&mut self) {
        if let Some(search) = self.search_mut().filter(|s| s.editing) {
            search.query.pop();
        }
    }

    /// Edit the query again.
    pub fn search_edit(&mut self) {
        if let Some(search) = self.search_mut() {
            search.editing = true;
        }
    }

    /// Search the selected chat for the query, unless it is blank.
    pub fn submit_search(&mut self) -> Option<ClientFrame> {
        let jid = self.chats.selected_jid()?.to_owned();
        let search = self.transcript.search.as_mut()?;
        let query = search.query.trim();
        if query.is_empty() {
            return None;
        }
        search.editing = false;
        search.awaiting = true;
        Some(ClientFrame::search(jid, query, SEARCH_LIMIT))
    }

    /// Move to the next result (the first with none chosen).
    pub fn select_next_result(&mut self) {
        if let Some(search) = self.search_mut() {
            search.selected = Some(search.selected.map_or(0, |i| i.saturating_add(1)));
        }
    }

    /// Move to the previous result (the last with none chosen).
    pub fn select_previous_result(&mut self) {
        if let Some(search) = self.search_mut() {
            search.selected = Some(search.selected.map_or(usize::MAX, |i| i.saturating_sub(1)));
        }
    }

    /// The chosen result as the view settled it (clamped to what it lists).
    pub fn set_result_selected(&mut self, selected: Option<usize>) {
        if let Some(search) = self.search_mut() {
            search.selected = selected;
        }
    }

    /// Close the search and select the chosen result in the conversation; true when it was
    /// selected. A result older than the loaded messages says so in the notice.
    pub fn jump_to_result(&mut self) -> bool {
        let Some(search) = self.transcript.search.take() else {
            return false;
        };
        let results = search.results.unwrap_or_default();
        let Some(result) = search.selected.and_then(|i| results.get(i)) else {
            return false;
        };
        match self.loaded().iter().position(|m| m.id == result.id) {
            Some(index) => {
                self.transcript.cursor = Some(index);
                self.open = true;
                true
            }
            None => {
                self.notice = Some("that message is older than the loaded ones".to_owned());
                false
            }
        }
    }

    /// Send the draft to the selected chat as one message (a reply when it quotes one). It is
    /// shown at once as "sending" until the daemon answers. Nothing is sent for a blank draft,
    /// or while the selected chat vanished: the draft stays and the notice asks to choose the
    /// chat again.
    pub fn send(&mut self) -> Option<ClientFrame> {
        if self.composer.text().trim().is_empty() {
            return None;
        }
        if self.chats.orphan.is_some() {
            self.notice = Some(MERGED_NOTICE.to_owned());
            return None;
        }
        let jid = self.chats.selected_jid()?.to_owned();
        let text = self.composer.take();
        self.sends += 1;
        let client_id = format!("{}-{}", self.client_prefix, self.sends);
        self.transcript.sending.insert(client_id.clone());
        let reply_to = self.composer.take_reply_to();
        self.transcript
            .conversations
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
                reply_to: reply_to.clone(),
                reactions: Vec::new(),
                thumb: None,
                thumb_mime: None,
                width: None,
                height: None,
                animated: false,
            });
        self.view_reset = true;
        Some(match reply_to {
            Some(quoted) => ClientFrame::send_reply(jid, text, client_id, quoted),
            None => ClientFrame::send(jid, text, client_id),
        })
    }

    /// Fold one daemon frame (a reply or an event) in. An `event_focus` selects and opens the
    /// focused chat and asks for its messages; the messages of a chat just opened with unread
    /// messages, or a new message in the open chat, ask for a `mark_read` (one in flight per
    /// chat); a `media_ready` the user asked to open externally is to be opened.
    pub fn apply(&mut self, frame: &DaemonFrame) -> Option<Effect> {
        if let Some((alias, canonical)) = self.send_answered(frame) {
            self.adopt(&alias, &canonical);
        }
        match frame {
            DaemonFrame::Status {
                daemon,
                linked,
                session,
                unread,
                ..
            } => {
                self.status.daemon = daemon.clone();
                self.status.linked = *linked;
                self.status.session = session.clone();
                self.status.unread = *unread;
            }
            DaemonFrame::Chats { chats, .. } => self.set_chats(chats.clone()),
            DaemonFrame::Messages { jid, messages, .. }
                if self.chats.selected_jid() == Some(jid.as_str())
                    && self.transcript.search.as_ref().is_some_and(|s| s.awaiting) =>
            {
                if let Some(search) = self.transcript.search.as_mut() {
                    search.awaiting = false;
                    search.selected = (!messages.is_empty()).then_some(0);
                    search.results = Some(messages.clone());
                }
            }
            DaemonFrame::Messages { jid, messages, .. } => {
                let resynced = self.transcript.resyncing.as_deref() == Some(jid.as_str());
                if resynced {
                    self.transcript.resyncing = None;
                }
                let selected_id = self
                    .transcript
                    .cursor
                    .and_then(|i| self.transcript.conversations.get(jid)?.get(i))
                    .map(|m| m.id.clone());
                let (loaded, cursor) = self.transcript.load(jid, messages, resynced, selected_id);
                if self.chats.selected_jid() == Some(jid.as_str()) {
                    // A resync keeps the selected message where it still is; anything else
                    // starts with none selected.
                    self.transcript.cursor = cursor;
                }
                self.transcript.conversations.insert(jid.clone(), loaded);
                // The chat the user opened is read once its messages are shown.
                if self.transcript.opening.as_deref() == Some(jid.as_str()) {
                    self.transcript.opening = None;
                    if self.chats.get(jid).is_some_and(|chat| chat.unread > 0) {
                        return self.mark_read(jid);
                    }
                }
            }
            DaemonFrame::MarkedRead { jid, unread, .. } => {
                self.status.unread = *unread;
                self.chats.marked_read(jid);
            }
            DaemonFrame::Sent {
                client_id,
                state,
                id,
                timestamp,
                ..
            } => {
                self.transcript
                    .settle(client_id, state, id.as_deref(), timestamp.as_deref());
            }
            DaemonFrame::SendFailed { client_id, .. } => self.transcript.send_failed(client_id),
            DaemonFrame::Error { message, .. } => self.notice = Some(message.clone()),
            DaemonFrame::Reacted { jid, id, emoji, .. } => {
                self.transcript.reacted(jid, id, emoji);
            }
            // The full picture asked for to show inline.
            DaemonFrame::MediaReady { jid, id, path, .. } if self.transcript.showing.remove(id) => {
                self.transcript
                    .full
                    .insert((jid.clone(), id.clone()), PathBuf::from(path));
            }
            DaemonFrame::MediaReady { id, path, .. } if self.transcript.downloading.remove(id) => {
                return Some(Effect::Open(PathBuf::from(path)));
            }
            DaemonFrame::EventMessage { jid, message, .. } => {
                // A new message in the open chat is read at once while it is seen, as the
                // official clients do.
                if self.message_event(jid, message)
                    && !message.from_me
                    && self.seen
                    && self.is_open_chat(jid)
                {
                    return self.mark_read(jid);
                }
            }
            DaemonFrame::EventUnread {
                unread,
                jid,
                chat_unread,
                ..
            } => {
                self.status.unread = *unread;
                self.chats.set_unread(jid, *chat_unread);
            }
            DaemonFrame::EventConnection { session, .. } => {
                self.status.session = session.clone();
            }
            // App-state sync changed a chat's pin, archive or mute: take the chat as it now is and
            // sort the list again as the daemon lists it (so an unarchived chat goes back to its
            // place; archived chats last), keeping the selection.
            DaemonFrame::EventChat { chat, .. } => {
                let chats = self.chats.with_event_chat(chat);
                self.set_chats(chats);
            }
            DaemonFrame::EventFocus { jid, .. } => {
                let index = self.chats.position(jid)?;
                self.select_chat(index);
                return self.open_selected().map(Effect::Request);
            }
            _ => {}
        }
        None
    }

    /// The daemon refused `frame` with an `error` (or it was never sent): show `message` in the
    /// notice and end what the request left waiting (a download, a search, an open, a resync, a
    /// `mark_read`).
    pub fn request_failed(&mut self, frame: &ClientFrame, message: impl Into<String>) {
        self.notice = Some(message.into());
        match frame {
            ClientFrame::DownloadMedia { id, .. } => {
                self.transcript.downloading.remove(id);
                self.transcript.showing.remove(id);
            }
            ClientFrame::Search { .. } => {
                if let Some(search) = self.transcript.search.as_mut() {
                    search.awaiting = false;
                    search.editing = true;
                }
            }
            // A chat whose messages never came is not being opened or resynced any more.
            ClientFrame::Messages { jid, .. } => {
                if self.transcript.opening.as_ref() == Some(jid) {
                    self.transcript.opening = None;
                }
                if self.transcript.resyncing.as_ref() == Some(jid) {
                    self.transcript.resyncing = None;
                }
            }
            // The chat stays unread; nothing retries it (the next open does).
            ClientFrame::MarkRead { jid, .. } => {
                self.chats.marking.remove(jid);
            }
            _ => {}
        }
    }

    /// The daemon answered my send (or retry) `client_id` with `not_confirmed`: WhatsApp's
    /// acknowledgement timed out, so it may have been delivered. It is shown as not confirmed, not
    /// as not sent, and a retry is still offered: it reuses its WhatsApp id, so it can never
    /// become a second message.
    pub fn send_not_confirmed(&mut self, client_id: &str) {
        self.transcript.not_confirmed(client_id);
    }

    /// A new message, or a new state of one already shown (same `id`, or same `client_id` for
    /// a send of mine). True when it was added to the loaded conversation as a new message.
    fn message_event(&mut self, jid: &str, message: &Message) -> bool {
        self.transcript.send_event(message);
        if !self.chats.set_last_message(jid, message) {
            let chats = self.chats.with_new_chat(jid, message);
            self.set_chats(chats);
        }
        self.transcript.add_or_update(jid, message)
    }
}

#[cfg(test)]
mod tests;
