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

use nuncio_client::{
    Chat, ClientFrame, DaemonFrame, ErrorCode, MAX_FRAME_BYTES, Message, PROTOCOL_VERSION, Session,
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
fn fixture_directory_holds_the_seven_documented_files() {
    let names: Vec<String> = fixture_files()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 7, "unexpected fixture count: {names:?}");
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
                    last_message: Some(Message {
                        id: "3EB0AAA1".to_owned(),
                        kind: "text".to_owned(),
                        text: "See you at 18:00".to_owned(),
                        from_me: false,
                        timestamp: "2026-10-18T17:55:04Z".to_owned(),
                    }),
                },
                Chat {
                    jid: "15551230002@s.whatsapp.net".to_owned(),
                    name: "Grace Hopper".to_owned(),
                    unread: 0,
                    archived: false,
                    pinned: false,
                    last_message: Some(Message {
                        id: "3EB0AAA2".to_owned(),
                        kind: "text".to_owned(),
                        text: "The compiler is done".to_owned(),
                        from_me: true,
                        timestamp: "2026-10-18T16:02:11Z".to_owned(),
                    }),
                },
                Chat {
                    jid: "120363000000000000@g.us".to_owned(),
                    name: "Omarchy devs".to_owned(),
                    unread: 17,
                    archived: true,
                    pinned: false,
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
