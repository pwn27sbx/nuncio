//! Which new message deserves a desktop notification, what it says, and which chat its `default`
//! action opens. Pure: the app owns the notification daemon and the window, this decides.
//!
//! No notification is shown for:
//!
//! - a message of mine (`from_me`), which covers every republish of a send of mine as it changes
//!   state and my own reactions on it;
//! - a message already notified once: the daemon publishes a message again as `event_message`
//!   when a reaction on it changes ([`ViewState::notified`] remembers what was shown);
//! - a muted chat;
//! - the chat on screen: the focused chat while the window is visible (a focused chat in a hidden
//!   window still notifies);
//! - anything while `[notifications] enabled = false`.
//!
//! An archived chat that is not muted still notifies, as WhatsApp does.

use std::collections::{HashSet, VecDeque};

use crate::config::Config;
use crate::wire::{Chat, ClientFrame, DaemonFrame};

/// How many notified message ids [`Notified`] remembers before forgetting the oldest.
pub const NOTIFIED_CAPACITY: usize = 1024;

/// What the app shows right now.
#[derive(Debug, Clone, Default)]
pub struct ViewState {
    /// The chat open in the window, if any (any JID the app holds for it, an alias included).
    pub focused_chat: Option<String>,
    /// Whether the window is on screen; a hidden or background window sees nothing.
    pub window_visible: bool,
    /// The messages already notified, so a republished one is not notified again.
    pub notified: Notified,
}

/// A bounded set of `(chat, message id)` pairs already notified: the newest
/// [`NOTIFIED_CAPACITY`], so a day of messages keeps the memory flat.
#[derive(Debug, Clone, Default)]
pub struct Notified {
    order: VecDeque<(String, String)>,
    seen: HashSet<(String, String)>,
}

impl Notified {
    /// Whether message `id` of chat `jid` was notified.
    pub fn contains(&self, jid: &str, id: &str) -> bool {
        self.seen.contains(&(jid.to_owned(), id.to_owned()))
    }

    /// Remember that `notification` was shown.
    pub fn record(&mut self, notification: &Notification) {
        let key = (
            notification.chat_jid.clone(),
            notification.message_id.clone(),
        );
        if !self.seen.insert(key.clone()) {
            return;
        }
        self.order.push_back(key);
        if self.order.len() > NOTIFIED_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// One desktop notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// The chat its `default` action opens: the LID chat when the message came under one of its
    /// phone-number aliases.
    pub chat_jid: String,
    /// The chat's name, else its masked number, else `hidden number` / `unnamed group`.
    pub title: String,
    /// The message text with `[notifications] show_preview = true`; otherwise only what kind of
    /// message it is (`New message`, `Photo`, `Sticker`, …).
    pub body: String,
    pub message_id: String,
}

/// The notification `event` deserves, if any (see the module docs for when there is none).
/// Anything but an `event_message` is `None`. The caller records a shown notification in
/// [`ViewState::notified`].
pub fn notification_for(
    event: &DaemonFrame,
    chats: &[Chat],
    state: &ViewState,
    config: &Config,
) -> Option<Notification> {
    let DaemonFrame::EventMessage { jid, message, .. } = event else {
        return None;
    };
    if !config.notifications.enabled || message.from_me {
        return None;
    }
    let chat = find_chat(chats, jid);
    let chat_jid = chat.map_or(jid.as_str(), |chat| chat.jid.as_str());
    if chat.is_some_and(|chat| chat.muted) {
        return None;
    }
    if state.notified.contains(chat_jid, &message.id) {
        return None;
    }
    if state.window_visible {
        let focused = state
            .focused_chat
            .as_deref()
            .map(|focused| find_chat(chats, focused).map_or(focused, |chat| chat.jid.as_str()));
        if focused == Some(chat_jid) {
            return None;
        }
    }
    let title = match chat {
        Some(chat) => chat_title(chat),
        None => title_from_jid(jid),
    };
    let body = if config.notifications.show_preview && !message.text.trim().is_empty() {
        message.text.clone()
    } else {
        kind_label(&message.kind).to_owned()
    };
    Some(Notification {
        chat_jid: chat_jid.to_owned(),
        title,
        body,
        message_id: message.id.clone(),
    })
}

/// The request a click on `notification` sends: `focus` on its chat, which the daemon passes to
/// every running UI as `event_focus`.
pub fn default_action(notification: &Notification) -> ClientFrame {
    ClientFrame::focus(notification.chat_jid.clone())
}

/// The listed chat `jid` names: the chat itself, else the LID chat that lists it in `aliases`.
/// A number listed as a chat of its own is never moved to another chat.
fn find_chat<'a>(chats: &'a [Chat], jid: &str) -> Option<&'a Chat> {
    chats.iter().find(|chat| chat.jid == jid).or_else(|| {
        chats
            .iter()
            .find(|chat| chat.aliases.iter().any(|alias| alias == jid))
    })
}

/// The name a chat shows, as nuncio-tui's chat list does (wire "Chat names", ADR 0002): its
/// `name`, else its `phone`, else a phone-number chat's own number masked to its last four digits,
/// else `unnamed group` for a group and `hidden number` for anyone else. A LID's digits are never
/// shown.
pub fn chat_title(chat: &Chat) -> String {
    if !chat.name.is_empty() {
        return chat.name.clone();
    }
    match &chat.phone {
        Some(phone) if !phone.is_empty() => phone.clone(),
        _ => title_from_jid(&chat.jid),
    }
}

fn title_from_jid(jid: &str) -> String {
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

/// What a notification without the text says.
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "Photo",
        "sticker" => "Sticker",
        "video" => "Video",
        "audio" => "Audio",
        "document" => "Document",
        _ => "New message",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Message, PROTOCOL_VERSION, Reaction};

    const ANA_PN: &str = "15551230021@s.whatsapp.net";
    const ANA_LID: &str = "99000000000021@lid";
    const BEN: &str = "15551230022@s.whatsapp.net";
    const GROUP: &str = "120363000000000001@g.us";

    fn chat(jid: &str, name: &str) -> Chat {
        Chat {
            jid: jid.to_owned(),
            name: name.to_owned(),
            unread: 1,
            archived: false,
            pinned: false,
            muted: false,
            aliases: Vec::new(),
            phone: None,
            last_message: None,
        }
    }

    fn message(id: &str, kind: &str, text: &str, from_me: bool) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": kind, "text": text, "from_me": from_me,
            "timestamp": "2026-10-20T10:00:00Z",
        }))
        .expect("a message")
    }

    fn event(jid: &str, message: Message) -> DaemonFrame {
        DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: jid.to_owned(),
            message,
        }
    }

    fn text_event(jid: &str, id: &str) -> DaemonFrame {
        event(jid, message(id, "text", "Running late", false))
    }

    fn chats() -> Vec<Chat> {
        let mut ana = chat(ANA_LID, "Ana");
        ana.aliases = vec![ANA_PN.to_owned()];
        vec![ana, chat(BEN, "Ben"), chat(GROUP, "Climbing")]
    }

    fn hidden() -> ViewState {
        ViewState::default()
    }

    fn visible_on(jid: &str) -> ViewState {
        ViewState {
            focused_chat: Some(jid.to_owned()),
            window_visible: true,
            ..ViewState::default()
        }
    }

    #[test]
    fn a_new_message_notifies_with_the_chat_name_and_no_text_by_default() {
        let n = notification_for(
            &text_event(BEN, "M1"),
            &chats(),
            &hidden(),
            &Config::default(),
        )
        .expect("notifies");
        assert_eq!(
            n,
            Notification {
                chat_jid: BEN.to_owned(),
                title: "Ben".to_owned(),
                body: "New message".to_owned(),
                message_id: "M1".to_owned(),
            }
        );
        assert!(
            !n.body.contains("Running late"),
            "privacy: no text by default"
        );
    }

    #[test]
    fn show_preview_puts_the_text_in_the_body() {
        let mut config = Config::default();
        config.notifications.show_preview = true;
        let n = notification_for(&text_event(BEN, "M1"), &chats(), &hidden(), &config)
            .expect("notifies");
        assert_eq!(n.body, "Running late");
    }

    #[test]
    fn the_body_without_preview_names_the_kind() {
        for (kind, text, label) in [
            ("text", "hi", "New message"),
            ("image", "a caption", "Photo"),
            ("sticker", "", "Sticker"),
            ("video", "", "Video"),
            ("audio", "", "Audio"),
            ("document", "report.pdf", "Document"),
            ("poll", "", "New message"),
        ] {
            let ev = event(BEN, message("M2", kind, text, false));
            let n =
                notification_for(&ev, &chats(), &hidden(), &Config::default()).expect("notifies");
            assert_eq!(n.body, label, "{kind}");
        }
    }

    #[test]
    fn with_preview_a_caption_is_shown_and_media_without_one_keeps_its_label() {
        let mut config = Config::default();
        config.notifications.show_preview = true;
        let with_caption = event(BEN, message("M3", "image", "Summit!", false));
        let without = event(BEN, message("M4", "sticker", "", false));
        let blank = event(BEN, message("M5", "image", "  ", false));
        let body = |ev| {
            notification_for(&ev, &chats(), &hidden(), &config)
                .expect("notifies")
                .body
        };
        assert_eq!(body(with_caption), "Summit!");
        assert_eq!(body(without), "Sticker");
        assert_eq!(body(blank), "Photo");
    }

    #[test]
    fn nothing_is_notified_for_my_own_messages_and_their_state_changes() {
        let mut queued = message("M6", "text", "on my way", true);
        queued.state = serde_json::from_value(serde_json::json!("queued")).expect("state");
        queued.client_id = Some("tui-1".to_owned());
        for msg in [message("M6", "text", "on my way", true), queued] {
            assert_eq!(
                notification_for(&event(BEN, msg), &chats(), &hidden(), &Config::default()),
                None
            );
        }
    }

    #[test]
    fn a_message_republished_for_a_reaction_is_not_notified_twice() {
        let mut state = hidden();
        let first = notification_for(&text_event(BEN, "M7"), &chats(), &state, &Config::default())
            .expect("first time notifies");
        state.notified.record(&first);
        let mut reacted = message("M7", "text", "Running late", false);
        reacted.reactions = vec![Reaction {
            emoji: "👍".to_owned(),
            from_me: true,
        }];
        assert_eq!(
            notification_for(&event(BEN, reacted), &chats(), &state, &Config::default()),
            None
        );
        assert!(
            notification_for(&text_event(BEN, "M8"), &chats(), &state, &Config::default())
                .is_some(),
            "another message still notifies"
        );
    }

    #[test]
    fn the_notified_set_is_bounded() {
        let mut notified = Notified::default();
        for i in 0..NOTIFIED_CAPACITY + 10 {
            notified.record(&Notification {
                chat_jid: BEN.to_owned(),
                title: String::new(),
                body: String::new(),
                message_id: format!("M{i}"),
            });
        }
        assert_eq!(notified.len(), NOTIFIED_CAPACITY);
        assert!(!notified.contains(BEN, "M0"), "the oldest is forgotten");
        assert!(notified.contains(BEN, &format!("M{}", NOTIFIED_CAPACITY + 9)));
    }

    #[test]
    fn a_muted_chat_never_notifies_and_an_archived_one_does() {
        let mut list = chats();
        list[1].muted = true;
        assert_eq!(
            notification_for(&text_event(BEN, "M9"), &list, &hidden(), &Config::default()),
            None
        );
        let mut list = chats();
        list[1].archived = true;
        assert!(
            notification_for(&text_event(BEN, "M9"), &list, &hidden(), &Config::default())
                .is_some(),
            "archived but not muted notifies, as WhatsApp does"
        );
        list[1].muted = true;
        assert_eq!(
            notification_for(&text_event(BEN, "M9"), &list, &hidden(), &Config::default()),
            None
        );
    }

    #[test]
    fn disabled_notifications_show_nothing() {
        let mut config = Config::default();
        config.notifications.enabled = false;
        assert_eq!(
            notification_for(&text_event(BEN, "M10"), &chats(), &hidden(), &config),
            None
        );
    }

    #[test]
    fn the_chat_on_screen_does_not_notify_but_a_hidden_window_does() {
        let config = Config::default();
        let ev = text_event(BEN, "M11");
        assert_eq!(
            notification_for(&ev, &chats(), &visible_on(BEN), &config),
            None
        );
        let mut hidden_on_ben = visible_on(BEN);
        hidden_on_ben.window_visible = false;
        assert!(
            notification_for(&ev, &chats(), &hidden_on_ben, &config).is_some(),
            "focused chat in a hidden window notifies"
        );
        assert!(
            notification_for(&ev, &chats(), &visible_on(GROUP), &config).is_some(),
            "another chat on screen notifies"
        );
        let no_chat_open = ViewState {
            window_visible: true,
            ..ViewState::default()
        };
        assert!(notification_for(&ev, &chats(), &no_chat_open, &config).is_some());
    }

    #[test]
    fn a_message_under_an_alias_targets_the_lid_chat() {
        let n = notification_for(
            &text_event(ANA_PN, "M12"),
            &chats(),
            &hidden(),
            &Config::default(),
        )
        .expect("notifies");
        assert_eq!(n.chat_jid, ANA_LID);
        assert_eq!(n.title, "Ana");
        assert_eq!(
            default_action(&n),
            ClientFrame::Focus {
                v: PROTOCOL_VERSION,
                jid: ANA_LID.to_owned(),
            }
        );
    }

    #[test]
    fn the_alias_chats_mute_and_focus_apply_to_its_lid_chat() {
        let config = Config::default();
        let ev = text_event(ANA_PN, "M13");
        assert_eq!(
            notification_for(&ev, &chats(), &visible_on(ANA_LID), &config),
            None
        );
        let lid_event = text_event(ANA_LID, "M14");
        assert_eq!(
            notification_for(&lid_event, &chats(), &visible_on(ANA_PN), &config),
            None,
            "the app still holding the number has the LID chat open"
        );
        let mut list = chats();
        list[0].muted = true;
        assert_eq!(notification_for(&ev, &list, &hidden(), &config), None);
    }

    #[test]
    fn a_number_listed_as_its_own_chat_is_never_moved_to_the_lid_chat() {
        let mut list = chats();
        let mut recycled = chat(ANA_PN, "New owner");
        recycled.muted = false;
        list.push(recycled);
        let n = notification_for(
            &text_event(ANA_PN, "M15"),
            &list,
            &hidden(),
            &Config::default(),
        )
        .expect("notifies");
        assert_eq!(n.chat_jid, ANA_PN);
        assert_eq!(n.title, "New owner");
    }

    #[test]
    fn the_title_follows_the_chat_name_rule() {
        let mut phone = chat("99000000000031@lid", "");
        phone.phone = Some("…0031".to_owned());
        let cases = [
            (chat(BEN, "Ben"), "Ben"),
            (phone, "…0031"),
            (chat("15551230032@s.whatsapp.net", ""), "…0032"),
            (chat("99000000000033@lid", ""), "hidden number"),
            (chat(GROUP, ""), "unnamed group"),
        ];
        for (c, want) in cases {
            let jid = c.jid.clone();
            let n = notification_for(
                &text_event(&jid, "M16"),
                &[c],
                &hidden(),
                &Config::default(),
            )
            .expect("notifies");
            assert_eq!(n.title, want, "{jid}");
        }
    }

    #[test]
    fn a_chat_not_in_the_list_still_notifies_under_a_masked_title() {
        for (jid, want) in [
            ("15551230041@s.whatsapp.net", "…0041"),
            ("99000000000042@lid", "hidden number"),
            ("120363000000000043@g.us", "unnamed group"),
        ] {
            let n = notification_for(&text_event(jid, "M17"), &[], &hidden(), &Config::default())
                .expect("notifies");
            assert_eq!((n.chat_jid.as_str(), n.title.as_str()), (jid, want));
            assert!(!n.title.contains("1555123"), "never the full number");
        }
    }

    #[test]
    fn only_event_message_notifies() {
        let focus = DaemonFrame::EventFocus {
            v: PROTOCOL_VERSION,
            jid: BEN.to_owned(),
        };
        assert_eq!(
            notification_for(&focus, &chats(), &hidden(), &Config::default()),
            None
        );
    }

    #[test]
    fn the_default_action_focuses_the_chat() {
        let n = notification_for(
            &text_event(GROUP, "M18"),
            &chats(),
            &hidden(),
            &Config::default(),
        )
        .expect("notifies");
        assert_eq!(default_action(&n), ClientFrame::focus(GROUP));
        assert_eq!(n.title, "Climbing");
    }
}
