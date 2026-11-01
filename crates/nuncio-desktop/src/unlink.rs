//! The Settings row **Unlink this device** and its typed confirmation (S11): the words the
//! dialog shows, the count of unsent work a wipe would destroy, and the rule that only the exact
//! word sends anything.
//!
//! There is no new frame: the confirmation sends the daemon's own `wipe` with `unlink: true`
//! (`proto/wire.md`, the wipe frame). The daemon tells WhatsApp to unlink the device through its
//! one session, deletes its keyring items, database, media cache and exports, removes its socket
//! and exits; the app lands on the link screen (S4) when the daemon that replaces it reports
//! `unlinked`, with no restart.
//!
//! The typed word is read from the field when the button is pressed, and nowhere else: it is
//! never kept, logged or shown.

/// The word the confirmation asks for, typed exactly.
pub const WORD: &str = "unlink";

/// The row's title in the settings window.
pub const ROW: &str = "Unlink this device";

/// The row's one line under the title: what the row does.
pub const ROW_DETAIL: &str = "Unlink it on WhatsApp and delete nuncio's data on this machine";

/// What the confirmation says will happen, above the field.
pub const BODY: &str = "WhatsApp will unlink this device, and nuncio's data on this machine — the \
     database, its keys and the media it downloaded — will be deleted. This cannot be undone.";

/// The one line the app shows once the daemon confirmed the unlink and deleted its data.
pub const UNLINKED: &str = "the device was unlinked and nuncio's data on this machine is deleted";

/// What the app says when the daemon answered `wiped` without WhatsApp's confirmation: it never
/// claims the device was unlinked.
pub const UNCONFIRMED: &str = "WhatsApp did not confirm the unlink";

/// The line above the field: the dialog states the exact word it wants.
pub fn ask() -> String {
    format!("Type {WORD} to confirm")
}

/// What a wipe destroys that the user has not sent yet: the draft of every chat that has one,
/// and the sends the daemon still holds queued.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unsent {
    /// Chats with a draft written (a chat merged into another keeps both texts in one).
    pub drafts: usize,
    /// Sends of mine waiting for the session to reconnect (`state: queued`).
    pub queued: usize,
}

impl Unsent {
    /// Whether a wipe would destroy unsent work.
    pub fn any(self) -> bool {
        self.drafts > 0 || self.queued > 0
    }

    /// The one line the confirmation shows before the field, when there is work to lose.
    pub fn sentence(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.drafts > 0 {
            let noun = if self.drafts == 1 { "draft" } else { "drafts" };
            parts.push(format!("{} unsent {noun}", self.drafts));
        }
        if self.queued > 0 {
            let noun = if self.queued == 1 {
                "message"
            } else {
                "messages"
            };
            parts.push(format!("{} {noun} waiting to send", self.queued));
        }
        (!parts.is_empty()).then(|| format!("{} will be deleted with it.", parts.join(" and ")))
    }
}

#[cfg(test)]
mod tests {
    use super::{BODY, ROW, UNCONFIRMED, UNLINKED, Unsent, WORD, ask};

    #[test]
    fn the_confirmation_states_the_word_it_wants_and_what_the_wipe_deletes() {
        assert_eq!(WORD, "unlink");
        assert_eq!(ask(), "Type unlink to confirm");
        assert_eq!(ROW, "Unlink this device");
        for word in [
            "WhatsApp",
            "unlink",
            "database",
            "keys",
            "media",
            "cannot be undone",
        ] {
            assert!(BODY.contains(word), "`{word}` missing from: {BODY}");
        }
        assert!(
            UNLINKED.contains("unlinked"),
            "the app says the device was unlinked: {UNLINKED}"
        );
        assert!(
            !UNCONFIRMED.to_lowercase().contains("unlinked"),
            "an unconfirmed wipe never claims the device was unlinked: {UNCONFIRMED}"
        );
    }

    #[test]
    fn the_warning_counts_the_drafts_and_queued_sends_a_wipe_would_destroy() {
        assert_eq!(Unsent::default().sentence(), None);
        assert!(!Unsent::default().any());
        assert_eq!(
            Unsent {
                drafts: 1,
                queued: 0
            }
            .sentence()
            .as_deref(),
            Some("1 unsent draft will be deleted with it.")
        );
        assert_eq!(
            Unsent {
                drafts: 0,
                queued: 1
            }
            .sentence()
            .as_deref(),
            Some("1 message waiting to send will be deleted with it.")
        );
        assert_eq!(
            Unsent {
                drafts: 2,
                queued: 3
            }
            .sentence()
            .as_deref(),
            Some("2 unsent drafts and 3 messages waiting to send will be deleted with it.")
        );
    }
}
