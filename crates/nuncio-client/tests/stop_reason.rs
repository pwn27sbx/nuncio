//! Q6: why WhatsApp stopped the session (`stop_reason`), as `proto/wire.md` defines it: an
//! optional fixed value on `status` and `event_connection`, mapped to fixed English text.
use std::fs;
use std::path::PathBuf;

use nuncio_client::wire::{PROTOCOL_VERSION, StopReason};
use nuncio_client::{DaemonFrame, Model, Session, Snapshot};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../proto/fixtures")
        .join(name);
    fs::read_to_string(path)
        .expect("fixture")
        .trim_end()
        .to_owned()
}

const ALL: [(&str, StopReason); 6] = [
    ("logged_out", StopReason::LoggedOut),
    ("temporary_ban", StopReason::TemporaryBan),
    ("stream_replaced", StopReason::StreamReplaced),
    ("client_outdated", StopReason::ClientOutdated),
    ("connect_rejected", StopReason::ConnectRejected),
    ("other", StopReason::Other),
];

#[test]
fn event_connection_reads_each_stop_reason() {
    for (wire, want) in ALL {
        let text = format!(
            r#"{{"v":1,"op":"event_connection","session":"offline","stop_reason":"{wire}"}}"#
        );
        let frame: DaemonFrame = serde_json::from_str(&text).expect("parses");
        assert_eq!(
            frame,
            DaemonFrame::EventConnection {
                v: PROTOCOL_VERSION,
                session: Session::Offline,
                stop_reason: Some(want),
            },
            "{text}"
        );
        assert_eq!(want.as_str(), wire);
    }
}

#[test]
fn an_unknown_stop_reason_is_other_and_a_missing_one_is_none() {
    let frame: DaemonFrame = serde_json::from_str(
        r#"{"v":1,"op":"event_connection","session":"offline","stop_reason":"something_new"}"#,
    )
    .expect("a newer daemon's value still parses");
    assert!(matches!(
        frame,
        DaemonFrame::EventConnection {
            stop_reason: Some(StopReason::Other),
            ..
        }
    ));
    let frame: DaemonFrame =
        serde_json::from_str(&fixture("event_connection.json")).expect("parses");
    assert!(matches!(
        frame,
        DaemonFrame::EventConnection {
            stop_reason: None,
            ..
        }
    ));
}

/// The same JSON as the fixture (serde orders `op` first, so compared as values) and no
/// `stop_reason` key at all.
fn assert_fixture(frame: &DaemonFrame, name: &str) {
    let got = serde_json::to_string(frame).expect("serializes");
    assert!(!got.contains("stop_reason"), "{got}");
    let got: serde_json::Value = serde_json::from_str(&got).expect("JSON");
    let want: serde_json::Value = serde_json::from_str(&fixture(name)).expect("JSON");
    assert_eq!(got, want);
}

#[test]
fn frames_without_a_stop_serialize_as_before() {
    let event = DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    };
    assert_fixture(&event, "event_connection.json");
    let status = DaemonFrame::Status {
        v: PROTOCOL_VERSION,
        daemon: "nunciod/0.1.0".to_owned(),
        linked: false,
        session: Session::Unlinked,
        unread: 0,
        stop_reason: None,
    };
    assert_fixture(&status, "status_response.json");
}

#[test]
fn each_stop_reason_has_fixed_text_without_numbers() {
    let want = [
        "WhatsApp logged this device out",
        "WhatsApp temporarily banned this account",
        "another connection of this device took over",
        "WhatsApp says this client is out of date",
        "WhatsApp rejected the connection",
        "WhatsApp stopped the session",
    ];
    for ((_, reason), text) in ALL.into_iter().zip(want) {
        assert_eq!(reason.describe(), text);
        assert!(!text.chars().any(|c| c.is_ascii_digit()), "{text}");
    }
}

fn model() -> Model {
    Model::new(
        Snapshot {
            daemon: "test".to_owned(),
            linked: true,
            session: Session::Linked,
            unread: 0,
            chats: Vec::new(),
            stop_reason: None,
        },
        "t",
    )
}

#[test]
fn the_model_follows_the_stop_reason_of_events_status_and_snapshot() {
    let mut m = model();
    assert_eq!(m.status().stop_reason, None);
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::TemporaryBan),
    });
    assert_eq!(m.status().session, Session::Offline);
    assert_eq!(m.status().stop_reason, Some(StopReason::TemporaryBan));

    // A later frame without one (a new daemon run reconnected) clears it.
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Linked,
        stop_reason: None,
    });
    assert_eq!(m.status().stop_reason, None);

    m.apply(&DaemonFrame::Status {
        v: PROTOCOL_VERSION,
        daemon: "test".to_owned(),
        linked: false,
        session: Session::Unlinked,
        unread: 0,
        stop_reason: Some(StopReason::LoggedOut),
    });
    assert_eq!(m.status().stop_reason, Some(StopReason::LoggedOut));

    let started = Model::new(
        Snapshot {
            daemon: "test".to_owned(),
            linked: true,
            session: Session::Offline,
            unread: 0,
            chats: Vec::new(),
            stop_reason: Some(StopReason::StreamReplaced),
        },
        "t",
    );
    assert_eq!(
        started.status().stop_reason,
        Some(StopReason::StreamReplaced)
    );
}
