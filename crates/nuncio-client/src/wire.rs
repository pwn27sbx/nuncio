//! The nuncio wire types, mirroring `proto/wire.md` exactly.
//!
//! Keys are `snake_case` and every frame carries `"v"`. The two directions are separate
//! enums because `status`, `chats` and `messages` exist in both, with different fields.

use serde::{Deserialize, Serialize};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u8 = 1;

/// The largest frame accepted, in bytes, excluding the `\n` terminator.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// A frame a client sends to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientFrame {
    /// Must be the first frame on every connection.
    Hello {
        v: u8,
        client: String,
    },
    Status {
        v: u8,
    },
    /// `limit` is documented as 1..200; the daemon owns validating it.
    Chats {
        v: u8,
        limit: u16,
    },
    /// Up to `limit` messages of one chat, newest first; `limit` is 1..200 like `chats`.
    Messages {
        v: u8,
        jid: String,
        limit: u16,
    },
    /// One text message to one chat. `client_id` is chosen by the client and identifies the
    /// send in the reply, so a retry can never become a duplicate.
    Send {
        v: u8,
        jid: String,
        text: String,
        client_id: String,
        /// The id of a message of that chat this send quotes (a reply); absent for a plain send,
        /// and it must stay absent (added in M2).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
    },
    /// Up to `limit` stored messages of one chat whose text contains every word of `query`,
    /// newest first; answered with `messages`. `limit` is 1..200; an empty query is `bad_frame`.
    Search {
        v: u8,
        jid: String,
        query: String,
        limit: u16,
    },
    /// Set my reaction on message `id` of chat `jid`; an empty `emoji` removes it. Answered with
    /// `reacted`, or an error: reactions are never queued, so one asked for offline is refused.
    React {
        v: u8,
        jid: String,
        id: String,
        emoji: String,
    },
    /// Mark every unread message of chat `jid` read, here and on WhatsApp. A UI sends it when the
    /// user opens the chat: that is the explicit view after which the daemon sends read receipts
    /// (read-self when the user turned read receipts off). Answered with `marked_read`, or an
    /// error: receipts are never queued, so a chat opened offline stays unread (added in M2).
    MarkRead {
        v: u8,
        jid: String,
    },
    /// Send again, once, a send of this daemon that ended not sent, by its `client_id` and under
    /// its first WhatsApp message id, so it can never become a second message. A send that is
    /// queued or sent is only reported. Nothing retries on its own.
    Retry {
        v: u8,
        client_id: String,
    },
    /// Ask the daemon to push event frames on this connection.
    Subscribe {
        v: u8,
    },
    /// Ask the running UIs to open one chat.
    Focus {
        v: u8,
        jid: String,
    },
    /// Ask the daemon to delete everything it keeps for the device and exit. With `unlink` it
    /// first tells WhatsApp to unlink the device (`nuncio logout`); without it WhatsApp is never
    /// contacted (`nuncio purge`).
    Wipe {
        v: u8,
        unlink: bool,
    },
    /// Ask the daemon to download the attachment of message `id` in chat `jid` into its media
    /// cache, unless it is cached already. Nothing is downloaded without this request.
    DownloadMedia {
        v: u8,
        jid: String,
        id: String,
    },
}

impl ClientFrame {
    /// The `hello` frame announcing this client, e.g. `nuncio-tui/0.1.0`.
    pub fn hello(client: impl Into<String>) -> Self {
        Self::Hello {
            v: PROTOCOL_VERSION,
            client: client.into(),
        }
    }

    pub fn status() -> Self {
        Self::Status {
            v: PROTOCOL_VERSION,
        }
    }

    pub fn chats(limit: u16) -> Self {
        Self::Chats {
            v: PROTOCOL_VERSION,
            limit,
        }
    }

    pub fn messages(jid: impl Into<String>, limit: u16) -> Self {
        Self::Messages {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            limit,
        }
    }

    pub fn send(
        jid: impl Into<String>,
        text: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self::Send {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            text: text.into(),
            client_id: client_id.into(),
            reply_to: None,
        }
    }

    /// A `send` that quotes message `reply_to` of the same chat.
    pub fn send_reply(
        jid: impl Into<String>,
        text: impl Into<String>,
        client_id: impl Into<String>,
        reply_to: impl Into<String>,
    ) -> Self {
        Self::Send {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            text: text.into(),
            client_id: client_id.into(),
            reply_to: Some(reply_to.into()),
        }
    }

    pub fn search(jid: impl Into<String>, query: impl Into<String>, limit: u16) -> Self {
        Self::Search {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            query: query.into(),
            limit,
        }
    }

    /// `emoji` empty removes my reaction.
    pub fn react(jid: impl Into<String>, id: impl Into<String>, emoji: impl Into<String>) -> Self {
        Self::React {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            id: id.into(),
            emoji: emoji.into(),
        }
    }

    /// The `mark_read` frame a UI sends when the user opens chat `jid`.
    pub fn mark_read(jid: impl Into<String>) -> Self {
        Self::MarkRead {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
        }
    }

    pub fn retry(client_id: impl Into<String>) -> Self {
        Self::Retry {
            v: PROTOCOL_VERSION,
            client_id: client_id.into(),
        }
    }

    pub fn subscribe() -> Self {
        Self::Subscribe {
            v: PROTOCOL_VERSION,
        }
    }

    pub fn focus(jid: impl Into<String>) -> Self {
        Self::Focus {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
        }
    }

    pub fn wipe(unlink: bool) -> Self {
        Self::Wipe {
            v: PROTOCOL_VERSION,
            unlink,
        }
    }

    pub fn download_media(jid: impl Into<String>, id: impl Into<String>) -> Self {
        Self::DownloadMedia {
            v: PROTOCOL_VERSION,
            jid: jid.into(),
            id: id.into(),
        }
    }
}

/// A frame the daemon sends to a client.
///
/// An `op` this build does not know parses as [`DaemonFrame::Unknown`] instead of failing, so a
/// newer daemon's events and replies never break an older client (a known op with a broken shape
/// is still an error). The derived serde code is reached through `remote = "Self"`, wrapped by
/// the hand-written `Serialize`/`Deserialize` impls below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self", tag = "op", rename_all = "snake_case")]
pub enum DaemonFrame {
    HelloOk {
        v: u8,
        daemon: String,
        session: Session,
        unread: u32,
    },
    Status {
        v: u8,
        daemon: String,
        linked: bool,
        session: Session,
        unread: u32,
    },
    Chats {
        v: u8,
        chats: Vec<Chat>,
    },
    Error {
        v: u8,
        code: ErrorCode,
        message: String,
    },
    Messages {
        v: u8,
        jid: String,
        messages: Vec<Message>,
    },
    /// An accepted send: `Sent` with the WhatsApp id and timestamp, or `Queued` while
    /// offline, with neither.
    Sent {
        v: u8,
        client_id: String,
        state: SendState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
    /// A send that did not happen; the message stays visible as not sent.
    SendFailed {
        v: u8,
        client_id: String,
        message: String,
    },
    /// Event frames follow on this connection.
    Subscribed {
        v: u8,
    },
    /// `delivered` is how many subscribed UIs got the focus event; 0 means none is running.
    Focused {
        v: u8,
        jid: String,
        delivered: u32,
    },
    /// Everything is deleted and the socket is gone; the daemon closes the connection and exits.
    /// `unlinked` says whether WhatsApp confirmed the unlink.
    Wiped {
        v: u8,
        unlinked: bool,
    },
    /// The attachment of message `id` in chat `jid` is in the daemon's private media cache at
    /// `path`; the bytes never travel over the socket.
    MediaReady {
        v: u8,
        jid: String,
        id: String,
        path: String,
        mime: String,
    },
    /// WhatsApp accepted my reaction `emoji` (empty: removed) on message `id` of chat `jid`.
    Reacted {
        v: u8,
        jid: String,
        id: String,
        emoji: String,
    },
    /// Chat `jid` is read here and on WhatsApp; `unread` is the new total (added in M2).
    MarkedRead {
        v: u8,
        jid: String,
        unread: u32,
    },
    EventMessage {
        v: u8,
        jid: String,
        message: Message,
    },
    EventConnection {
        v: u8,
        session: Session,
    },
    /// `unread` is the total; `chat_unread` the count of chat `jid`.
    EventUnread {
        v: u8,
        unread: u32,
        jid: String,
        chat_unread: u32,
    },
    EventFocus {
        v: u8,
        jid: String,
    },
    /// App-state sync from another device changed a chat's pin, archive or mute: `chat` is the
    /// chat as it now is, as `chats` would list it (added in M2).
    EventChat {
        v: u8,
        chat: Chat,
    },
    /// A frame whose `op` this build does not know: an event or a reply a newer daemon added.
    /// Only its `op` is kept; it serialises as `{"op": op}`.
    #[serde(skip)]
    Unknown {
        op: String,
    },
}

/// Every `op` a [`DaemonFrame`] variant other than `Unknown` stands for.
const KNOWN_DAEMON_OPS: &[&str] = &[
    "hello_ok",
    "status",
    "chats",
    "error",
    "messages",
    "sent",
    "send_failed",
    "subscribed",
    "focused",
    "wiped",
    "media_ready",
    "reacted",
    "marked_read",
    "event_message",
    "event_connection",
    "event_unread",
    "event_focus",
    "event_chat",
];

impl Serialize for DaemonFrame {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Unknown { op } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("op", op)?;
                map.end()
            }
            // The inherent function `remote = "Self"` derived, not this trait method.
            known => DaemonFrame::serialize(known, serializer),
        }
    }
}

impl<'de> Deserialize<'de> for DaemonFrame {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("op").and_then(serde_json::Value::as_str) {
            Some(op) if !KNOWN_DAEMON_OPS.contains(&op) => Ok(Self::Unknown { op: op.to_owned() }),
            // A known op, or no string op at all: the derived code reports what is wrong.
            _ => DaemonFrame::deserialize(value).map_err(serde::de::Error::custom),
        }
    }
}

impl DaemonFrame {
    /// The `op` value, for error messages and logging.
    pub fn op(&self) -> &str {
        match self {
            Self::HelloOk { .. } => "hello_ok",
            Self::Status { .. } => "status",
            Self::Chats { .. } => "chats",
            Self::Error { .. } => "error",
            Self::Messages { .. } => "messages",
            Self::Sent { .. } => "sent",
            Self::SendFailed { .. } => "send_failed",
            Self::Subscribed { .. } => "subscribed",
            Self::Focused { .. } => "focused",
            Self::Wiped { .. } => "wiped",
            Self::MediaReady { .. } => "media_ready",
            Self::Reacted { .. } => "reacted",
            Self::MarkedRead { .. } => "marked_read",
            Self::EventMessage { .. } => "event_message",
            Self::EventConnection { .. } => "event_connection",
            Self::EventUnread { .. } => "event_unread",
            Self::EventFocus { .. } => "event_focus",
            Self::EventChat { .. } => "event_chat",
            Self::Unknown { op } => op,
        }
    }
}

/// Whether a WhatsApp device is paired.
///
/// This is a string on the wire, not a boolean, so the daemon can add states without a
/// protocol break: any state this build does not know becomes [`Session::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Session {
    /// No WhatsApp device is paired. Everything else still works.
    Unlinked,
    /// Paired and connected.
    Linked,
    /// Paired but not currently connected.
    Offline,
    /// A state this build does not know yet.
    #[serde(other)]
    Unknown,
}

impl Session {
    /// The wire spelling, as used in `proto/wire.md`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unlinked => "unlinked",
            Self::Linked => "linked",
            Self::Offline => "offline",
            Self::Unknown => "unknown",
        }
    }
}

/// The state of an accepted send. Unknown states become [`SendState::Unknown`] for the same
/// reason [`Session::Unknown`] exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendState {
    /// Stored while offline; sent once on reconnect.
    Queued,
    /// Acknowledged by the server.
    Sent,
    #[serde(other)]
    Unknown,
}

/// A protocol error code. Unknown codes become [`ErrorCode::Unknown`] for the same reason
/// [`Session::Unknown`] exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    VersionMismatch,
    BadFrame,
    NotLinked,
    Internal,
    /// A send or retry whose acknowledgement from WhatsApp timed out: it may have been delivered,
    /// so it is neither sent nor not sent; a `retry` reuses its WhatsApp id (added in M2).
    NotConfirmed,
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// The wire spelling, as used in `proto/wire.md`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VersionMismatch => "version_mismatch",
            Self::BadFrame => "bad_frame",
            Self::NotLinked => "not_linked",
            Self::Internal => "internal",
            Self::NotConfirmed => "not_confirmed",
            Self::Unknown => "unknown",
        }
    }
}

/// A chat, in the M0 shape plus `muted`, `aliases` and `phone`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    /// WhatsApp JID, e.g. `15551230001@s.whatsapp.net`.
    pub jid: String,
    /// Display name; may be empty.
    pub name: String,
    pub unread: u32,
    pub archived: bool,
    pub pinned: bool,
    /// Muted right now (from the phone's app-state sync); absent from an older daemon, which
    /// reads as not muted.
    #[serde(default)]
    pub muted: bool,
    /// The phone-number chat JIDs the daemon merged into this chat (ADR 0001, one chat per
    /// contact), listed only while WhatsApp's mapping still points them here; absent when there
    /// are none, and absent from an older daemon. A client holding one of them adopts this chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// The contact's phone number masked to its last four digits (`…0001`), what a client shows
    /// when `name` is empty (ADR 0002); never the chat's LID digits. Absent for a group, when the
    /// number is not known, and from an older daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Absent for a chat with no messages, and it must stay absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<Message>,
}

/// A chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// WhatsApp message id.
    pub id: String,
    /// `text`, `image`, `audio`, `video`, `document` or `sticker`; a kind a client does not know
    /// is treated like any other media kind.
    pub kind: String,
    /// May be empty for non-text kinds (a media caption).
    pub text: String,
    pub from_me: bool,
    /// RFC 3339 in UTC with a `Z` suffix.
    pub timestamp: String,
    /// Set for the media kinds; absent for text, and it must stay absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Media>,
    /// Set only on a send of mine that is not sent yet; absent once it is sent (and on every
    /// received message), and it must stay absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<MessageState>,
    /// The `client_id` of a send that is not sent yet: what a `retry` names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The id of the message this one quotes; absent when it quotes none (added in M2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The current reactions on this message, mine first; absent when there are none (M2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    /// Base64 of the thumbnail an image (JPEG) or sticker (PNG) message embeds in itself, never
    /// downloaded; absent when there is none (T15 Part 2.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    /// The mime type of `thumb`: `image/jpeg` or `image/png`; set with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_mime: Option<String>,
    /// The picture's width in pixels (images and stickers); absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// The picture's height in pixels (images and stickers); absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// An animated sticker; absent (false) otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub animated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One sender's current reaction on a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub from_me: bool,
}

/// The state of a send of mine that is not sent yet. Unknown states become
/// [`MessageState::Unknown`] for the same reason [`Session::Unknown`] exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// Waiting for the session to reconnect; sent once when it does.
    Queued,
    /// One attempt failed (or was not confirmed); it stays like this until a manual `retry`.
    NotSent,
    #[serde(other)]
    Unknown,
}

/// A message's attachment. The bytes are never on the wire: the daemon downloads them on
/// demand into its media cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub mime: String,
    /// Bytes.
    pub size: u64,
    /// May be empty.
    pub file_name: String,
}

impl Message {
    /// The `HH:MM` the daemon wrote, or `None` if `timestamp` is not RFC 3339.
    ///
    /// The timestamp is UTC and is *not* converted to local time: M0 has no time zone
    /// database (and a local-time render would make the rendering tests depend on `TZ`).
    pub fn time_hhmm(&self) -> Option<&str> {
        let after_t = self.timestamp.split_once('T')?.1;
        let bytes = after_t.as_bytes();
        if bytes.len() < 5
            || bytes[2] != b':'
            || !bytes[..2].iter().all(u8::is_ascii_digit)
            || !bytes[3..5].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        // The five bytes checked above are ASCII, so index 5 is a character boundary.
        Some(&after_t[..5])
    }
}
