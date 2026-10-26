//! The chat list as a client shows it: the chats in the daemon's order (archived last), the
//! selected chat (by position, kept on the same JID across a new list), the phone-number chats the
//! daemon merged into a LID chat (ADR 0001), a vanished open chat, and the `mark_read`s in flight.
//!
//! [`ChatList`] holds that state and its single-list rules; the rules that also move a
//! conversation or a draft (adopting an alias, a chat that vanished with a draft written) live in
//! [`Model`](super::Model), which owns the list.

use std::collections::{HashMap, HashSet};

use crate::wire::{Chat, Message};

/// The notice when the open chat vanished from the chat list (the daemon merged it).
pub const MERGED_NOTICE: &str = "that chat was merged; choose it again before sending";

/// A phone-number chat JID and a LID chat JID: the only pair the daemon merges (ADR 0001).
pub fn is_alias_pair(alias: &str, canonical: &str) -> bool {
    alias.ends_with("@s.whatsapp.net") && canonical.ends_with("@lid")
}

/// The name a chat shows (wire "Chat names", ADR 0002): its `name`, else its `phone` (the number
/// masked to its last four digits), else, as `nuncio chats` does, a phone-number chat's own number
/// masked the same way; otherwise a placeholder: `unnamed group` for a group, `hidden number` for
/// anyone else. A LID's digits are never shown: they are a hidden id, not a number.
///
/// The text is the chat's own: a client that cannot draw some characters (a terminal and emoji
/// sequences) filters it when it draws it.
pub fn chat_title(chat: &Chat) -> String {
    if !chat.name.is_empty() {
        return chat.name.clone();
    }
    match &chat.phone {
        Some(phone) if !phone.is_empty() => phone.clone(),
        _ => title_from_jid(&chat.jid),
    }
}

/// The title of a chat known only by its JID: a phone number masked, else a placeholder.
pub(crate) fn title_from_jid(jid: &str) -> String {
    match jid.split_once('@') {
        Some((number, "s.whatsapp.net")) => mask_phone(number),
        Some((_, "g.us")) => "unnamed group".to_owned(),
        _ => "hidden number".to_owned(),
    }
}

/// `…` and the last four digits, or `…` alone for four digits or fewer (the daemon's
/// `ipc.MaskPhone`).
fn mask_phone(number: &str) -> String {
    let digits: Vec<char> = number.chars().collect();
    if digits.len() <= 4 {
        return "…".to_owned();
    }
    let last: String = digits[digits.len() - 4..].iter().collect();
    format!("…{last}")
}

/// The timestamp of a chat's newest message, or "" for a chat with none.
fn newest(chat: &Chat) -> &str {
    chat.last_message
        .as_ref()
        .map_or("", |message| message.timestamp.as_str())
}

/// Sort `chats` as the daemon lists them: newest message first (RFC 3339 UTC timestamps sort as
/// text; a chat with no message sorts last), the same time by JID, as the daemon's
/// `ORDER BY ... DESC, c.jid` does.
pub fn sort_as_daemon(chats: &mut [Chat]) {
    chats.sort_by(|a, b| newest(b).cmp(newest(a)).then_with(|| a.jid.cmp(&b.jid)));
}

/// A chat for `jid` that nothing but its JID is known of yet.
pub(crate) fn empty_chat(jid: &str) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: String::new(),
        unread: 0,
        archived: false,
        pinned: false,
        muted: false,
        aliases: Vec::new(),
        phone: None,
        last_message: None,
    }
}

/// The chat list (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct ChatList {
    pub(crate) chats: Vec<Chat>,
    pub(crate) selected: usize,
    /// The open chat that a new chat list no longer has (the daemon merged it): it stays the
    /// selected chat, and nothing is sent to it until the user chooses a chat again or a reply
    /// to a request about it names the chat it became.
    pub(crate) orphan: Option<Chat>,
    /// Phone-number JIDs the daemon merged into another chat (its LID chat), from a chat's
    /// `aliases` or a reply to a request naming them, with that canonical JID.
    pub(crate) aliases: HashMap<String, String>,
    /// JIDs of chats with a `mark_read` sent and not answered yet (at most one per chat).
    pub(crate) marking: HashSet<String>,
    /// Chats opened from outside the list ([`Model::open_jid`](super::Model::open_jid)) that the
    /// daemon does not list yet: kept listed until it does.
    pub(crate) pending: HashSet<String>,
}

/// A filter of the chat list (design plan §5): which chats the list shows. One definition for
/// both clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Unread,
    Archived,
}

impl Filter {
    /// The filters in their order.
    pub const EACH: [Self; 3] = [Self::All, Self::Unread, Self::Archived];

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Archived => "Archived",
        }
    }

    /// Whether `chat` is listed under this filter.
    pub fn keeps(self, chat: &Chat) -> bool {
        match self {
            Self::All => true,
            Self::Unread => chat.unread > 0,
            Self::Archived => chat.archived,
        }
    }
}

impl ChatList {
    /// The chats as shown: the daemon's order, archived chats last.
    pub fn chats(&self) -> &[Chat] {
        &self.chats
    }

    /// The selected position in [`ChatList::chats`] (meaningless while [`ChatList::orphan`]).
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The selected chat: the vanished open chat while there is one, else the listed one.
    pub fn selected_chat(&self) -> Option<&Chat> {
        self.orphan
            .as_ref()
            .or_else(|| self.chats.get(self.selected))
    }

    /// The JID of the selected chat, if any.
    pub fn selected_jid(&self) -> Option<&str> {
        self.selected_chat().map(|chat| chat.jid.as_str())
    }

    /// The list row to highlight: none while the selected chat is not listed (it vanished) or
    /// the list is empty.
    pub fn highlighted(&self) -> Option<usize> {
        (self.orphan.is_none() && !self.chats.is_empty()).then_some(self.selected)
    }

    /// The selected chat when it vanished from the list (merged by the daemon): nothing can be
    /// sent until the user chooses a chat again.
    pub fn orphan(&self) -> Option<&Chat> {
        self.orphan.as_ref()
    }

    /// The listed chat `jid`.
    pub fn get(&self, jid: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.jid == jid)
    }

    /// The position of the listed chat `jid`.
    pub fn position(&self, jid: &str) -> Option<usize> {
        self.chats.iter().position(|chat| chat.jid == jid)
    }

    /// The chat the phone-number chat `alias` was merged into, when it was adopted.
    pub fn canonical(&self, alias: &str) -> Option<&str> {
        self.aliases.get(alias).map(String::as_str)
    }

    /// Whether a `mark_read` for chat `jid` is in flight.
    pub fn is_marking(&self, jid: &str) -> bool {
        self.marking.contains(jid)
    }

    /// Start a `mark_read` for `jid`: false when one is already in flight.
    pub(crate) fn begin_mark_read(&mut self, jid: &str) -> bool {
        self.marking.insert(jid.to_owned())
    }

    /// The daemon answered `marked_read` for `jid`: it is read.
    pub(crate) fn marked_read(&mut self, jid: &str) {
        self.marking.remove(jid);
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
            chat.unread = 0;
        }
    }

    /// An `event_unread` for `jid`: its own unread count.
    pub(crate) fn set_unread(&mut self, jid: &str, unread: u32) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
            chat.unread = unread;
        }
    }

    /// A new message in the listed chat `jid` is its last one; false when `jid` is not listed.
    pub(crate) fn set_last_message(&mut self, jid: &str, message: &Message) -> bool {
        match self.chats.iter_mut().find(|c| c.jid == jid) {
            Some(chat) => {
                chat.last_message = Some(message.clone());
                true
            }
            None => false,
        }
    }

    /// The list after an `event_chat`: the chat as it now is, the numbers merged into it no
    /// longer listed as chats of their own (ADR 0001), sorted as the daemon lists them.
    pub(crate) fn with_event_chat(&self, chat: &Chat) -> Vec<Chat> {
        let mut chats = self.chats.clone();
        chats.retain(|c| !chat.aliases.contains(&c.jid) || !is_alias_pair(&c.jid, &chat.jid));
        match chats.iter_mut().find(|c| c.jid == chat.jid) {
            Some(shown) => *shown = chat.clone(),
            None => chats.push(chat.clone()),
        }
        sort_as_daemon(&mut chats);
        chats
    }

    /// The list with a chat for `jid` added, that only a message named so far.
    pub(crate) fn with_new_chat(&self, jid: &str, message: &Message) -> Vec<Chat> {
        let mut chats = self.chats.clone();
        chats.push(Chat {
            last_message: Some(message.clone()),
            ..empty_chat(jid)
        });
        chats
    }

    /// The listed entry for the phone-number chat `alias` becomes `canonical`'s: renamed, or
    /// dropped when `canonical` is already listed. A pending `alias` is a pending `canonical`.
    pub(crate) fn rename_entry(&mut self, alias: &str, canonical: &str) {
        if self.pending.remove(alias) {
            self.pending.insert(canonical.to_owned());
        }
        if self.chats.iter().any(|c| c.jid == canonical) {
            self.chats.retain(|c| c.jid != alias);
        } else if let Some(entry) = self.chats.iter_mut().find(|c| c.jid == alias) {
            entry.jid = canonical.to_owned();
        }
    }

    /// A pending `mark_read` for `alias` is now one for `canonical`.
    pub(crate) fn rename_marking(&mut self, alias: &str, canonical: &str) {
        if self.marking.remove(alias) {
            self.marking.insert(canonical.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn at(jid: &str, timestamp: &str) -> Chat {
        let mut c = chat("", jid);
        c.last_message = Some(
            serde_json::from_value(serde_json::json!({
                "id": "M", "kind": "text", "text": "", "from_me": false, "timestamp": timestamp,
            }))
            .expect("a message"),
        );
        c
    }

    #[test]
    fn a_chats_name_wins_over_its_phone() {
        let mut ada = chat("Ada Lovelace", "100000000000001@lid");
        ada.phone = Some("…0001".to_owned());
        assert_eq!(chat_title(&ada), "Ada Lovelace");
    }

    #[test]
    fn a_chat_with_no_name_shows_its_masked_phone() {
        let mut ada = chat("", "100000000000001@lid");
        ada.phone = Some("…0001".to_owned());
        assert_eq!(chat_title(&ada), "…0001");
        let mut pn = chat("", "15551230001@s.whatsapp.net");
        pn.phone = Some("…0001".to_owned());
        assert_eq!(chat_title(&pn), "…0001");
    }

    #[test]
    fn a_chat_with_neither_shows_a_placeholder_never_its_jid_digits() {
        assert_eq!(
            chat_title(&chat("", "100000000000001@lid")),
            "hidden number"
        );
        assert_eq!(
            chat_title(&chat("", "120363000000000001@g.us")),
            "unnamed group"
        );
    }

    #[test]
    fn a_phone_number_chat_with_neither_shows_its_own_number_masked_as_nuncio_chats_does() {
        // ipc.MaskPhone: "…" and the last four digits; "…" alone for four digits or fewer.
        assert_eq!(chat_title(&chat("", "15551230005@s.whatsapp.net")), "…0005");
        assert_eq!(chat_title(&chat("", "1234@s.whatsapp.net")), "…");
    }

    #[test]
    fn the_title_keeps_the_names_own_characters() {
        // Filtering emoji for a terminal is the terminal's business, at draw time.
        assert_eq!(chat_title(&chat("Ana 👋🏽", "a@lid")), "Ana 👋🏽");
    }

    #[test]
    fn chats_sort_newest_first_then_by_jid_with_no_message_last() {
        let mut chats = vec![
            chat("", "e@lid"),
            at("b@lid", "2026-10-20T10:00:00Z"),
            at("a@lid", "2026-10-20T10:00:00Z"),
            at("c@lid", "2026-10-20T11:00:00Z"),
        ];
        sort_as_daemon(&mut chats);
        let order: Vec<&str> = chats.iter().map(|c| c.jid.as_str()).collect();
        assert_eq!(order, ["c@lid", "a@lid", "b@lid", "e@lid"]);
    }

    #[test]
    fn an_event_chat_drops_only_the_phone_numbers_merged_into_it() {
        let list = ChatList {
            chats: vec![
                chat("Ana", "15551230001@s.whatsapp.net"),
                chat("Grp", "120363000000000001@g.us"),
            ],
            ..ChatList::default()
        };
        let mut ana = chat("Ana", "99000000000001@lid");
        // A group JID is never an alias, even when a chat names it.
        ana.aliases = vec![
            "15551230001@s.whatsapp.net".to_owned(),
            "120363000000000001@g.us".to_owned(),
        ];
        let jids: Vec<String> = list
            .with_event_chat(&ana)
            .into_iter()
            .map(|c| c.jid)
            .collect();
        assert_eq!(jids, ["120363000000000001@g.us", "99000000000001@lid"]);
    }
}
