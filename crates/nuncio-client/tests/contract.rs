//! Contract tests for the nuncio NDJSON wire protocol.
//!
//! Both languages read the *same* files in `proto/fixtures/` (see `proto/wire.md`), so a
//! change to the contract fails one side until both agree. Two things are checked here:
//!
//! 1. every fixture deserialises into the wire types, re-serialises, and compares
//!    **semantically** (key order is not part of the contract);
//! 2. the daemon→client fixtures are *constructed* in Rust and compared against the
//!    fixture files, which is what proves the two languages agree on the shape rather
//!    than each agreeing with itself.

use std::fs;
use std::path::{Path, PathBuf};

use nuncio_client::wire::{MessageState, Reaction};
use nuncio_client::{
    Chat, ClientFrame, DaemonFrame, ErrorCode, MAX_FRAME_BYTES, Media, Message, PROTOCOL_VERSION,
    SendState, Session,
};
use serde::Serialize;
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto/fixtures")
}

/// Every `.json` file in `proto/fixtures/`, sorted so failures are reproducible.
fn fixture_files() -> Vec<PathBuf> {
    let dir = fixtures_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", dir.display()))
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "{} must hold the shared fixtures",
        dir.display()
    );
    files
}

fn read_json(path: &Path) -> Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is JSON: {e}", path.display()))
}

/// `Some(serialised)` when the fixture deserialises into `F` and the re-serialised value
/// is semantically equal to the file — which is exactly the contract test `proto/wire.md`
/// describes.
fn round_trips<F>(path: &Path, want: &Value) -> Option<Value>
where
    F: serde::de::DeserializeOwned + Serialize,
{
    let text = fs::read_to_string(path).expect("fixture is readable");
    let frame: F = serde_json::from_str(&text).ok()?;
    let got = serde_json::to_value(&frame).expect("frame serialises");
    (got == *want).then_some(got)
}

#[test]
fn every_fixture_round_trips_through_exactly_one_direction() {
    for path in fixture_files() {
        let want = read_json(&path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let as_client = round_trips::<ClientFrame>(&path, &want);
        let as_daemon = round_trips::<DaemonFrame>(&path, &want);

        match (as_client.as_ref(), as_daemon.as_ref()) {
            (Some(got), None) | (None, Some(got)) => assert_eq!(
                *got, want,
                "{name}: re-serialised fixture is not semantically equal"
            ),
            (Some(c), Some(d)) => panic!(
                "{name}: fixture round-trips as both a client frame ({c}) and a daemon frame ({d}); \
                 the wire shape is ambiguous"
            ),
            (None, None) => panic!(
                "{name}: fixture round-trips through neither ClientFrame nor DaemonFrame; \
                 the wire types and the fixture disagree. Fixture: {want}"
            ),
        }
    }
}

#[test]
fn fixture_directory_holds_the_forty_four_documented_files() {
    let names: Vec<String> = fixture_files()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 44, "unexpected fixture count: {names:?}");
}

/// The client frames the Rust client actually sends must byte-for-byte agree with the
/// fixtures the Go daemon is tested against (semantically).
#[test]
fn constructed_client_frames_match_their_fixtures() {
    let dir = fixtures_dir();
    assert_frame(
        &dir.join("hello_client.json"),
        &ClientFrame::hello("nuncio-tui/0.1.0"),
    );
    assert_frame(&dir.join("status_request.json"), &ClientFrame::status());
    assert_frame(&dir.join("chats_request.json"), &ClientFrame::chats(40));
    assert_frame(
        &dir.join("messages_request.json"),
        &ClientFrame::messages("15551230001@s.whatsapp.net", 50),
    );
    assert_frame(
        &dir.join("send_request.json"),
        &ClientFrame::send("15551230001@s.whatsapp.net", "On my way", "c-7f3a9e21"),
    );
    assert_frame(
        &dir.join("subscribe_request.json"),
        &ClientFrame::subscribe(),
    );
    assert_frame(
        &dir.join("focus_request.json"),
        &ClientFrame::focus("15551230002@s.whatsapp.net"),
    );
    assert_frame(&dir.join("wipe_request.json"), &ClientFrame::wipe(true));
    assert_frame(
        &dir.join("download_media_request.json"),
        &ClientFrame::download_media("15551230001@s.whatsapp.net", "3EB0AAA0"),
    );
    assert_frame(
        &dir.join("retry_request.json"),
        &ClientFrame::retry("c-7f3a9e23"),
    );
}

/// The M1 daemon frames, constructed in Rust.
#[test]
fn constructed_m1_daemon_frames_match_their_fixtures() {
    let dir = fixtures_dir();
    let ada = "15551230001@s.whatsapp.net".to_owned();
    let grace = "15551230002@s.whatsapp.net".to_owned();

    assert_frame(
        &dir.join("error_not_linked.json"),
        &DaemonFrame::Error {
            v: PROTOCOL_VERSION,
            code: ErrorCode::NotLinked,
            message: "no WhatsApp device is linked".to_owned(),
        },
    );
    assert_frame(
        &dir.join("messages_response.json"),
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ada.clone(),
            messages: vec![
                Message {
                    id: "3EB0AAA1".to_owned(),
                    kind: "text".to_owned(),
                    text: "See you at 18:00".to_owned(),
                    from_me: false,
                    timestamp: "2026-10-18T17:55:04Z".to_owned(),
                    media: None,
                    state: None,
                    client_id: None,
                    reply_to: None,
                    reactions: Vec::new(),
                    thumb: None,
                    thumb_mime: None,
                    width: None,
                    height: None,
                    animated: false,
                    sender: None,
                },
                Message {
                    id: "3EB0AAA0".to_owned(),
                    kind: "image".to_owned(),
                    text: "the venue".to_owned(),
                    from_me: false,
                    timestamp: "2026-10-18T17:50:30Z".to_owned(),
                    media: Some(Media {
                        mime: "image/jpeg".to_owned(),
                        size: 48213,
                        file_name: String::new(),
                    }),
                    state: None,
                    client_id: None,
                    reply_to: None,
                    reactions: Vec::new(),
                    thumb: None,
                    thumb_mime: None,
                    width: None,
                    height: None,
                    animated: false,
                    sender: None,
                },
            ],
        },
    );
    assert_frame(
        &dir.join("sent_response.json"),
        &DaemonFrame::Sent {
            v: PROTOCOL_VERSION,
            client_id: "c-7f3a9e21".to_owned(),
            state: SendState::Sent,
            id: Some("3EB0BBB1".to_owned()),
            timestamp: Some("2026-10-18T17:56:10Z".to_owned()),
        },
    );
    assert_frame(
        &dir.join("sent_queued_response.json"),
        &DaemonFrame::Sent {
            v: PROTOCOL_VERSION,
            client_id: "c-7f3a9e22".to_owned(),
            state: SendState::Queued,
            id: None,
            timestamp: None,
        },
    );
    assert_frame(
        &dir.join("send_failed_response.json"),
        &DaemonFrame::SendFailed {
            v: PROTOCOL_VERSION,
            client_id: "c-7f3a9e23".to_owned(),
            message: "the server did not acknowledge the message".to_owned(),
        },
    );
    assert_frame(
        &dir.join("subscribed_response.json"),
        &DaemonFrame::Subscribed {
            v: PROTOCOL_VERSION,
        },
    );
    assert_frame(
        &dir.join("focused_response.json"),
        &DaemonFrame::Focused {
            v: PROTOCOL_VERSION,
            jid: grace.clone(),
            delivered: 1,
        },
    );
    assert_frame(
        &dir.join("event_message.json"),
        &DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: ada.clone(),
            message: Message {
                id: "3EB0AAA3".to_owned(),
                kind: "text".to_owned(),
                text: "Running late".to_owned(),
                from_me: false,
                timestamp: "2026-10-18T17:58:00Z".to_owned(),
                media: None,
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
                thumb: None,
                thumb_mime: None,
                width: None,
                height: None,
                animated: false,
                sender: None,
            },
        },
    );
    assert_frame(
        &dir.join("event_message_not_sent.json"),
        &DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: ada.clone(),
            message: Message {
                id: "3EB0BBB3".to_owned(),
                kind: "text".to_owned(),
                text: "On my way".to_owned(),
                from_me: true,
                timestamp: "2026-10-18T17:56:12Z".to_owned(),
                media: None,
                state: Some(MessageState::NotSent),
                client_id: Some("c-7f3a9e23".to_owned()),
                reply_to: None,
                reactions: Vec::new(),
                thumb: None,
                thumb_mime: None,
                width: None,
                height: None,
                animated: false,
                sender: None,
            },
        },
    );
    assert_frame(
        &dir.join("event_connection.json"),
        &DaemonFrame::EventConnection {
            v: PROTOCOL_VERSION,
            session: Session::Offline,
        },
    );
    assert_frame(
        &dir.join("event_unread.json"),
        &DaemonFrame::EventUnread {
            v: PROTOCOL_VERSION,
            unread: 20,
            jid: ada,
            chat_unread: 3,
        },
    );
    assert_frame(
        &dir.join("event_focus.json"),
        &DaemonFrame::EventFocus {
            v: PROTOCOL_VERSION,
            jid: grace,
        },
    );
    assert_frame(
        &dir.join("wiped_response.json"),
        &DaemonFrame::Wiped {
            v: PROTOCOL_VERSION,
            unlinked: true,
        },
    );
    assert_frame(
        &dir.join("media_ready_response.json"),
        &DaemonFrame::MediaReady {
            v: PROTOCOL_VERSION,
            jid: "15551230001@s.whatsapp.net".to_owned(),
            id: "3EB0AAA0".to_owned(),
            path: "/home/ada/.local/share/nuncio/media/\
                   e9603b3a3952bcc1847052b2b73de4ac145b06a2c5a615735b7440e41d65ad12.jpeg"
                .to_owned(),
            mime: "image/jpeg".to_owned(),
        },
    );
}

#[test]
fn a_send_state_this_client_does_not_know_yet_still_parses() {
    let frame: DaemonFrame =
        serde_json::from_str(r#"{"v":1,"op":"sent","client_id":"c","state":"delivered"}"#)
            .expect("an unknown send state parses");
    let DaemonFrame::Sent { state, .. } = frame else {
        panic!("a sent frame stays a sent frame");
    };
    assert_eq!(state, SendState::Unknown);
}

/// Constructing the daemon→client fixtures in Rust and comparing them to the files is what
/// proves the two languages agree on the shape rather than each agreeing with itself.
#[test]
fn constructed_daemon_frames_match_their_fixtures() {
    let dir = fixtures_dir();

    assert_frame(
        &dir.join("hello_ok_daemon.json"),
        &DaemonFrame::HelloOk {
            v: PROTOCOL_VERSION,
            daemon: "nunciod/0.1.0".to_owned(),
            session: Session::Unlinked,
            unread: 0,
        },
    );

    assert_frame(
        &dir.join("status_response.json"),
        &DaemonFrame::Status {
            v: PROTOCOL_VERSION,
            daemon: "nunciod/0.1.0".to_owned(),
            linked: false,
            session: Session::Unlinked,
            unread: 0,
        },
    );

    assert_frame(
        &dir.join("error_version_mismatch.json"),
        &DaemonFrame::Error {
            v: PROTOCOL_VERSION,
            code: ErrorCode::VersionMismatch,
            message: "unsupported protocol version".to_owned(),
        },
    );

    assert_frame(
        &dir.join("chats_response.json"),
        &DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![
                Chat {
                    jid: "15551230001@s.whatsapp.net".to_owned(),
                    name: "Ada Lovelace".to_owned(),
                    unread: 2,
                    archived: false,
                    pinned: true,
                    muted: false,
                    aliases: Vec::new(),
                    phone: None,
                    last_message: Some(Message {
                        id: "3EB0AAA1".to_owned(),
                        kind: "text".to_owned(),
                        text: "See you at 18:00".to_owned(),
                        from_me: false,
                        timestamp: "2026-10-18T17:55:04Z".to_owned(),
                        media: None,
                        state: None,
                        client_id: None,
                        reply_to: None,
                        reactions: Vec::new(),
                        thumb: None,
                        thumb_mime: None,
                        width: None,
                        height: None,
                        animated: false,
                        sender: None,
                    }),
                },
                Chat {
                    jid: "15551230002@s.whatsapp.net".to_owned(),
                    name: "Grace Hopper".to_owned(),
                    unread: 0,
                    archived: false,
                    pinned: false,
                    muted: false,
                    aliases: Vec::new(),
                    phone: None,
                    last_message: Some(Message {
                        id: "3EB0AAA2".to_owned(),
                        kind: "text".to_owned(),
                        text: "The compiler is done".to_owned(),
                        from_me: true,
                        timestamp: "2026-10-18T16:02:11Z".to_owned(),
                        media: None,
                        state: None,
                        client_id: None,
                        reply_to: None,
                        reactions: Vec::new(),
                        thumb: None,
                        thumb_mime: None,
                        width: None,
                        height: None,
                        animated: false,
                        sender: None,
                    }),
                },
                Chat {
                    jid: "120363000000000000@g.us".to_owned(),
                    name: "Omarchy devs".to_owned(),
                    unread: 17,
                    archived: true,
                    pinned: false,
                    muted: true,
                    aliases: Vec::new(),
                    phone: None,
                    last_message: None,
                },
            ],
        },
    );
}

fn assert_frame<T: Serialize>(path: &Path, frame: &T) {
    let got = serde_json::to_value(frame).expect("frame serialises");
    assert_eq!(got, read_json(path), "{}", path.display());
}

#[test]
fn a_chat_with_no_messages_omits_last_message() {
    let text = fs::read_to_string(fixtures_dir().join("chats_response.json")).unwrap();
    let frame: DaemonFrame = serde_json::from_str(&text).unwrap();
    let DaemonFrame::Chats { chats, .. } = frame else {
        panic!("chats_response.json is a chats frame");
    };
    assert!(chats[2].last_message.is_none());

    let last = serde_json::to_value(&chats[2]).unwrap();
    assert!(
        last.get("last_message").is_none(),
        "an absent last_message must stay absent: {last}"
    );
}

#[test]
fn a_chat_from_a_daemon_without_muted_parses_as_not_muted() {
    let chat: Chat = serde_json::from_str(
        r#"{"jid":"15551230001@s.whatsapp.net","name":"Ada","unread":0,"archived":false,"pinned":true}"#,
    )
    .expect("a chat without muted parses");
    assert!(!chat.muted);
    assert!(chat.pinned);
}

#[test]
fn sessions_and_error_codes_accept_states_this_client_does_not_know_yet() {
    // `session` is a string (not a boolean) precisely so the daemon can add states without
    // a protocol break; an unknown state must not fail the parse.
    let frame: DaemonFrame = serde_json::from_str(
        r#"{"v":1,"op":"status","daemon":"nunciod/9.9.9","linked":true,"session":"connecting","unread":3}"#,
    )
    .expect("an unknown session parses");
    assert_eq!(
        frame,
        DaemonFrame::Status {
            v: PROTOCOL_VERSION,
            daemon: "nunciod/9.9.9".to_owned(),
            linked: true,
            session: Session::Unknown,
            unread: 3,
        }
    );

    let frame: DaemonFrame =
        serde_json::from_str(r#"{"v":1,"op":"error","code":"teapot","message":"nope"}"#)
            .expect("an unknown error code parses");
    let DaemonFrame::Error { code, .. } = frame else {
        panic!("an error frame stays an error frame");
    };
    assert_eq!(code, ErrorCode::Unknown);
}

#[test]
fn the_frame_cap_is_one_mebibyte() {
    assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
}

#[test]
fn timestamps_expose_their_utc_time_of_day() {
    let text = fs::read_to_string(fixtures_dir().join("chats_response.json")).unwrap();
    let frame: DaemonFrame = serde_json::from_str(&text).unwrap();
    let DaemonFrame::Chats { chats, .. } = frame else {
        panic!("chats_response.json is a chats frame");
    };
    let message = chats[0].last_message.as_ref().unwrap();
    assert_eq!(message.time_hhmm(), Some("17:55"));

    let odd = Message {
        timestamp: "not a timestamp".to_owned(),
        ..message.clone()
    };
    assert_eq!(odd.time_hhmm(), None);
}

#[test]
fn a_message_without_an_outbox_state_parses_as_sent_and_an_unknown_state_still_parses() {
    let sent: Message = serde_json::from_str(
        r#"{"id":"x","kind":"text","text":"hi","from_me":true,"timestamp":"2026-10-18T17:55:04Z"}"#,
    )
    .unwrap();
    assert_eq!((sent.state, sent.client_id), (None, None));
    let later: Message = serde_json::from_str(
        r#"{"id":"x","kind":"text","text":"hi","from_me":true,"timestamp":"2026-10-18T17:55:04Z","state":"sending","client_id":"c"}"#,
    )
    .unwrap();
    assert_eq!(later.state, Some(MessageState::Unknown));
    let queued: Message = serde_json::from_str(
        r#"{"id":"x","kind":"text","text":"hi","from_me":true,"timestamp":"2026-10-18T17:55:04Z","state":"queued","client_id":"c"}"#,
    )
    .unwrap();
    assert_eq!(queued.state, Some(MessageState::Queued));
}

/// The M2 frames (search, reply, react), constructed in Rust.
#[test]
fn constructed_m2_frames_match_their_fixtures() {
    let dir = fixtures_dir();
    let ada = "15551230001@s.whatsapp.net";
    assert_frame(
        &dir.join("search_request.json"),
        &ClientFrame::search(ada, "venue", 50),
    );
    assert_frame(
        &dir.join("send_reply_request.json"),
        &ClientFrame::send_reply(ada, "Which venue?", "c-7f3a9e22", "3EB0AAA0"),
    );
    assert_frame(
        &dir.join("react_request.json"),
        &ClientFrame::react(ada, "3EB0AAA1", "\u{1F44D}"),
    );
    assert_frame(
        &dir.join("react_remove_request.json"),
        &ClientFrame::react(ada, "3EB0AAA1", ""),
    );
    assert_frame(
        &dir.join("reacted_response.json"),
        &DaemonFrame::Reacted {
            v: PROTOCOL_VERSION,
            jid: ada.to_owned(),
            id: "3EB0AAA1".to_owned(),
            emoji: "\u{1F44D}".to_owned(),
        },
    );
    assert_frame(
        &dir.join("search_response.json"),
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ada.to_owned(),
            messages: vec![
                Message {
                    id: "3EB0AAA2".to_owned(),
                    kind: "text".to_owned(),
                    text: "Which venue?".to_owned(),
                    from_me: true,
                    timestamp: "2026-10-18T17:56:40Z".to_owned(),
                    media: None,
                    state: None,
                    client_id: None,
                    reply_to: Some("3EB0AAA0".to_owned()),
                    reactions: vec![Reaction {
                        emoji: "\u{1F44D}".to_owned(),
                        from_me: false,
                    }],
                    thumb: None,
                    thumb_mime: None,
                    width: None,
                    height: None,
                    animated: false,
                    sender: None,
                },
                Message {
                    id: "3EB0AAA0".to_owned(),
                    kind: "image".to_owned(),
                    text: "the venue".to_owned(),
                    from_me: false,
                    timestamp: "2026-10-18T17:50:30Z".to_owned(),
                    media: Some(Media {
                        mime: "image/jpeg".to_owned(),
                        size: 48213,
                        file_name: String::new(),
                    }),
                    state: None,
                    client_id: None,
                    reply_to: None,
                    reactions: Vec::new(),
                    thumb: None,
                    thumb_mime: None,
                    width: None,
                    height: None,
                    animated: false,
                    sender: None,
                },
            ],
        },
    );
}

/// `mark_read` (a UI opened a chat) and its `marked_read` answer, constructed in Rust.
#[test]
fn constructed_mark_read_frames_match_their_fixtures() {
    let dir = fixtures_dir();
    let ada = "15551230001@s.whatsapp.net";
    assert_frame(
        &dir.join("mark_read_request.json"),
        &ClientFrame::mark_read(ada),
    );
    let marked = DaemonFrame::MarkedRead {
        v: PROTOCOL_VERSION,
        jid: ada.to_owned(),
        unread: 17,
    };
    assert_eq!(marked.op(), "marked_read");
    assert_frame(&dir.join("marked_read_response.json"), &marked);
}

/// A message from an M1 daemon (no `reply_to`, no `reactions`) still parses, as quoting nothing
/// and with no reactions, and a plain send still has no `reply_to` key.
#[test]
fn an_m1_message_parses_without_reply_to_or_reactions() {
    let m: Message = serde_json::from_str(
        r#"{"id":"A","kind":"text","text":"x","from_me":false,"timestamp":"2026-10-18T17:55:04Z"}"#,
    )
    .unwrap();
    assert_eq!(m.reply_to, None);
    assert!(m.reactions.is_empty());
    let plain = serde_json::to_value(ClientFrame::send("j", "t", "c")).unwrap();
    assert!(plain.get("reply_to").is_none(), "plain send: {plain}");
}

/// `event_chat` (app-state sync changed a chat's pin, archive or mute) and the `not_confirmed`
/// error (a send whose acknowledgement timed out), constructed in Rust (added in M2).
#[test]
fn constructed_event_chat_and_not_confirmed_frames_match_their_fixtures() {
    let dir = fixtures_dir();
    let event = DaemonFrame::EventChat {
        v: PROTOCOL_VERSION,
        chat: Chat {
            jid: "15551230001@s.whatsapp.net".to_owned(),
            name: "Ada Lovelace".to_owned(),
            unread: 2,
            archived: false,
            pinned: true,
            muted: true,
            aliases: Vec::new(),
            phone: None,
            last_message: Some(Message {
                id: "3EB0AAA1".to_owned(),
                kind: "text".to_owned(),
                text: "See you at 18:00".to_owned(),
                from_me: false,
                timestamp: "2026-10-18T17:55:04Z".to_owned(),
                media: None,
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
                thumb: None,
                thumb_mime: None,
                width: None,
                height: None,
                animated: false,
                sender: None,
            }),
        },
    };
    assert_eq!(event.op(), "event_chat");
    assert_frame(&dir.join("event_chat.json"), &event);

    assert_eq!(ErrorCode::NotConfirmed.as_str(), "not_confirmed");
    assert_frame(
        &dir.join("error_not_confirmed.json"),
        &DaemonFrame::Error {
            v: PROTOCOL_VERSION,
            code: ErrorCode::NotConfirmed,
            message:
                "not confirmed: WhatsApp did not acknowledge in time; it may have been delivered"
                    .to_owned(),
        },
    );
}

/// A newer daemon may send an op this client does not know (an event or a reply added later):
/// it parses into [`DaemonFrame::Unknown`] carrying its op instead of failing the connection.
#[test]
fn a_daemon_op_this_client_does_not_know_parses_as_unknown() {
    let text = r#"{"v":1,"op":"event_typing","jid":"15551230001@s.whatsapp.net","typing":true}"#;
    let frame: DaemonFrame = serde_json::from_str(text).expect("an unknown op still parses");
    assert_eq!(
        frame,
        DaemonFrame::Unknown {
            op: "event_typing".to_owned()
        }
    );
    assert_eq!(frame.op(), "event_typing");
}

/// The catch-all never swallows a known op: a known op with a broken shape is still an error,
/// and a frame with no op at all too.
#[test]
fn a_known_op_with_a_broken_shape_is_still_an_error() {
    for text in [
        r#"{"v":1,"op":"sent"}"#,
        r#"{"v":1,"op":"event_chat","chat":7}"#,
        r#"{"v":1}"#,
        r#"{"v":1,"op":7}"#,
    ] {
        assert!(
            serde_json::from_str::<DaemonFrame>(text).is_err(),
            "{text} must not parse"
        );
    }
}

/// Every daemon fixture still parses to its own variant (never to the catch-all), and only the
/// client fixtures, whose ops the daemon never sends, read as unknown daemon frames.
#[test]
fn every_daemon_fixture_parses_to_its_own_variant_not_the_catch_all() {
    let mut daemon_fixtures = 0;
    for path in fixture_files() {
        let want = read_json(&path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let op = want["op"].as_str().expect("every fixture has an op");
        let is_client = round_trips::<ClientFrame>(&path, &want).is_some();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        match serde_json::from_str::<DaemonFrame>(&text) {
            Ok(DaemonFrame::Unknown { op: got }) => {
                assert!(is_client, "{name}: a daemon fixture parsed as unknown");
                assert_eq!(got, op, "{name}");
            }
            Ok(frame) => {
                assert!(!is_client, "{name}: a client fixture parsed as {frame:?}");
                assert_eq!(frame.op(), op, "{name}: parsed to another variant");
                daemon_fixtures += 1;
            }
            // A client op the daemon also uses (`status`, `chats`, `messages`) with the client's
            // fields: that is the daemon variant's shape failing, as before.
            Err(_) => assert!(is_client, "{name}: a daemon fixture does not parse"),
        }
    }
    assert_eq!(daemon_fixtures, 29, "every daemon fixture was checked");
}

/// ADR 0001: `event_chat` (and `chats`) name the phone-number chats merged into a chat in
/// `aliases`, constructed in Rust.
#[test]
fn a_constructed_event_chat_with_aliases_matches_its_fixture() {
    let event = DaemonFrame::EventChat {
        v: PROTOCOL_VERSION,
        chat: Chat {
            jid: "100000000000001@lid".to_owned(),
            name: "Ada Lovelace".to_owned(),
            unread: 1,
            archived: false,
            pinned: false,
            muted: false,
            aliases: vec!["15551230001@s.whatsapp.net".to_owned()],
            phone: None,
            last_message: Some(Message {
                id: "3EB0AAA3".to_owned(),
                kind: "text".to_owned(),
                text: "reply from ada".to_owned(),
                from_me: false,
                timestamp: "2026-10-18T18:01:00Z".to_owned(),
                media: None,
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
                thumb: None,
                thumb_mime: None,
                width: None,
                height: None,
                animated: false,
                sender: None,
            }),
        },
    };
    assert_frame(&fixtures_dir().join("event_chat_aliases.json"), &event);
}

/// ADR 0002: a contact with no name carries her masked phone number in `phone`, the fallback a
/// client shows instead of the chat's LID digits; constructed in Rust.
#[test]
fn a_constructed_event_chat_with_a_phone_matches_its_fixture() {
    let event = DaemonFrame::EventChat {
        v: PROTOCOL_VERSION,
        chat: Chat {
            jid: "100000000000002@lid".to_owned(),
            name: String::new(),
            unread: 0,
            archived: false,
            pinned: false,
            muted: false,
            aliases: Vec::new(),
            phone: Some("…0002".to_owned()),
            last_message: None,
        },
    };
    assert_frame(&fixtures_dir().join("event_chat_phone.json"), &event);
}

/// A chat with no aliases and no phone serialises exactly as before `aliases` and `phone` existed:
/// the chats of the `chats` and `event_chat` fixtures, byte for byte.
#[test]
fn chats_without_aliases_are_byte_identical_to_before() {
    let text = fs::read_to_string(fixtures_dir().join("chats_response.json")).unwrap();
    let DaemonFrame::Chats { chats, .. } = serde_json::from_str(&text).unwrap() else {
        panic!("chats_response.json is a chats frame");
    };
    let want = text
        .trim_end()
        .strip_prefix(r#"{"v":1,"op":"chats","chats":"#)
        .and_then(|rest| rest.strip_suffix('}'))
        .expect("the fixture's shape");
    assert_eq!(serde_json::to_string(&chats).unwrap(), want);

    let text = fs::read_to_string(fixtures_dir().join("event_chat.json")).unwrap();
    let DaemonFrame::EventChat { chat, .. } = serde_json::from_str(&text).unwrap() else {
        panic!("event_chat.json is an event_chat frame");
    };
    let want = text
        .trim_end()
        .strip_prefix(r#"{"v":1,"op":"event_chat","chat":"#)
        .and_then(|rest| rest.strip_suffix('}'))
        .expect("the fixture's shape");
    assert_eq!(serde_json::to_string(&chat).unwrap(), want);
}

/// An image carries the thumbnail embedded in the message itself and its size; an animated sticker
/// is a media kind of its own with its PNG thumbnail (T15 Part 2.5).
#[test]
fn constructed_image_and_sticker_previews_match_their_fixtures() {
    let dir = fixtures_dir();
    let ada = "15551230001@s.whatsapp.net".to_owned();
    assert_frame(
        &dir.join("event_message_image.json"),
        &DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: ada.clone(),
            message: Message {
                id: "3EB0AAA4".to_owned(),
                kind: "image".to_owned(),
                text: "the venue".to_owned(),
                from_me: false,
                timestamp: "2026-10-18T17:59:00Z".to_owned(),
                media: Some(Media {
                    mime: "image/jpeg".to_owned(),
                    size: 48213,
                    file_name: String::new(),
                }),
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
                thumb: Some("/9j/4GZha2UganBlZyB0aHVtYm5haWw=".to_owned()),
                thumb_mime: Some("image/jpeg".to_owned()),
                width: Some(1280),
                height: Some(960),
                animated: false,
                sender: None,
            },
        },
    );
    assert_frame(
        &dir.join("event_message_sticker.json"),
        &DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: ada,
            message: Message {
                id: "3EB0AAA5".to_owned(),
                kind: "sticker".to_owned(),
                text: String::new(),
                from_me: false,
                timestamp: "2026-10-18T17:59:30Z".to_owned(),
                media: Some(Media {
                    mime: "image/webp".to_owned(),
                    size: 18000,
                    file_name: String::new(),
                }),
                state: None,
                client_id: None,
                reply_to: None,
                reactions: Vec::new(),
                thumb: Some("iVBORw0KGgpmYWtlIHBuZyB0aHVtYm5haWw=".to_owned()),
                thumb_mime: Some("image/png".to_owned()),
                width: Some(512),
                height: Some(512),
                animated: true,
                sender: None,
            },
        },
    );
}

/// Messages without a preview serialise exactly as before `thumb`, `thumb_mime`, `width`,
/// `height` and `animated` existed: the messages of the `messages` and `search` fixtures, byte
/// for byte.
#[test]
fn messages_without_a_preview_are_byte_identical_to_before() {
    for file in ["messages_response.json", "search_response.json"] {
        let text = fs::read_to_string(fixtures_dir().join(file)).unwrap();
        let DaemonFrame::Messages { messages, .. } = serde_json::from_str(&text).unwrap() else {
            panic!("{file} is a messages frame");
        };
        assert!(messages.iter().all(|m| m.thumb.is_none() && !m.animated));
        let want = text
            .trim_end()
            .strip_prefix(
                r#"{"v":1,"op":"messages","jid":"15551230001@s.whatsapp.net","messages":"#,
            )
            .and_then(|rest| rest.strip_suffix('}'))
            .expect("the fixture's shape");
        assert_eq!(serde_json::to_string(&messages).unwrap(), want, "{file}");
    }
}

/// A group message names its author in `sender` (M4): constructed in Rust, an incoming one carries
/// it, mine does not, and both group fixtures agree.
#[test]
fn constructed_group_messages_with_their_sender_match_their_fixtures() {
    let dir = fixtures_dir();
    let group = "120363000000000001@g.us".to_owned();
    let text = |id: &str, body: &str, from_me: bool, at: &str, sender: Option<&str>| Message {
        id: id.to_owned(),
        kind: "text".to_owned(),
        text: body.to_owned(),
        from_me,
        timestamp: at.to_owned(),
        media: None,
        state: None,
        client_id: None,
        reply_to: None,
        reactions: Vec::new(),
        sender: sender.map(str::to_owned),
        thumb: None,
        thumb_mime: None,
        width: None,
        height: None,
        animated: false,
    };
    assert_frame(
        &dir.join("messages_group_response.json"),
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: group.clone(),
            messages: vec![
                text("3EB0CCC3", "On my way", true, "2026-10-18T18:02:00Z", None),
                text(
                    "3EB0CCC2",
                    "Who brings the rope?",
                    false,
                    "2026-10-18T18:01:00Z",
                    Some("…0002"),
                ),
                text(
                    "3EB0CCC1",
                    "Meet at the crag",
                    false,
                    "2026-10-18T18:00:00Z",
                    Some("Ana"),
                ),
            ],
        },
    );
    assert_frame(
        &dir.join("event_message_group.json"),
        &DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: group,
            message: text(
                "3EB0CCC4",
                "Running late",
                false,
                "2026-10-18T18:03:00Z",
                Some("Ana"),
            ),
        },
    );
}
