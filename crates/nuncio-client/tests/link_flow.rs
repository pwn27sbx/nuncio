//! The device-linking flow as a client folds it (`proto/wire.md`, "The link flow"), checked
//! against the shared fixtures in `proto/fixtures/`: the code to draw and where the flow stands,
//! and nothing else.

use nuncio_client::{DaemonFrame, LinkFlow, LinkState, LinkUpdate};

const LINK_QR: &str = include_str!("../../../proto/fixtures/event_link_qr.json");
const LINK_WAITING: &str = include_str!("../../../proto/fixtures/event_link_state_waiting.json");
const LINK_LINKED: &str = include_str!("../../../proto/fixtures/event_link_state_linked.json");
const LINK_FAILED: &str = include_str!("../../../proto/fixtures/event_link_state_failed.json");
const EVENT_MESSAGE: &str = include_str!("../../../proto/fixtures/event_message.json");

fn frame(fixture: &str) -> DaemonFrame {
    serde_json::from_str(fixture).expect("the fixture is a daemon frame")
}

/// A flow holding `LINK_QR`'s code, as a client has it right after the daemon published one.
fn flow_with_a_code() -> LinkFlow {
    let mut flow = LinkFlow::new();
    assert_eq!(flow.apply(&frame(LINK_QR)), Some(LinkUpdate::Qr));
    flow
}

#[test]
fn a_link_qr_fixture_is_the_code_the_flow_holds() {
    let flow = flow_with_a_code();
    let qr = flow.qr().expect("the flow holds the code");
    assert_eq!(qr.size(), 21);
    assert_eq!(qr.rows().len(), 21, "one row per module row");
    assert!(qr.rows().iter().all(|row| row.len() == 21));
    // A `link_qr` alone says nothing about the flow's state: `link_state` does.
    assert_eq!(flow.phase(), None);
    assert_eq!(flow.reason(), None);
}

#[test]
fn the_modules_are_indexed_row_first_and_outside_the_matrix_is_light() {
    // Row 0 of the fixture: `111111101011101111111`, so module 7 is light.
    let flow = flow_with_a_code();
    let qr = flow.qr().expect("a code");
    assert!(qr.module(0, 0), "the first module of row 0 is dark");
    assert!(!qr.module(0, 7), "module 7 of row 0 is light");
    assert!(qr.module(0, 20));
    assert!(!qr.module(21, 0), "past the last row");
    assert!(!qr.module(0, 21), "past the last module of a row");
    assert!(!qr.module(u32::MAX, 0));
}

#[test]
fn a_refreshed_code_replaces_the_one_before() {
    let mut flow = flow_with_a_code();
    let refreshed = frame(r#"{"v":1,"op":"link_qr","size":2,"rows":["10","01"]}"#);
    assert_eq!(flow.apply(&refreshed), Some(LinkUpdate::Qr));
    let qr = flow.qr().expect("the newer code");
    assert_eq!(qr.size(), 2);
    assert_eq!(qr.rows(), ["10", "01"]);
}

#[test]
fn the_state_frames_say_where_the_flow_stands() {
    let mut flow = flow_with_a_code();
    assert_eq!(
        flow.apply(&frame(LINK_WAITING)),
        Some(LinkUpdate::State(LinkState::Waiting))
    );
    assert_eq!(flow.phase(), Some(LinkState::Waiting));
    assert!(flow.qr().is_some(), "waiting keeps the code to draw");

    let mut linked = flow_with_a_code();
    assert_eq!(
        linked.apply(&frame(LINK_LINKED)),
        Some(LinkUpdate::State(LinkState::Linked))
    );
    assert_eq!(linked.phase(), Some(LinkState::Linked));
    assert!(linked.qr().is_none(), "a linked flow is over");

    let mut failed = flow_with_a_code();
    assert_eq!(
        failed.apply(&frame(LINK_FAILED)),
        Some(LinkUpdate::State(LinkState::Failed))
    );
    assert_eq!(failed.phase(), Some(LinkState::Failed));
    assert!(failed.qr().is_none(), "a failed flow is over");
}

#[test]
fn a_failed_flow_carries_the_daemons_sentence_verbatim() {
    let fixture: serde_json::Value =
        serde_json::from_str(LINK_FAILED).expect("the fixture is JSON");
    let sentence = fixture["reason"]
        .as_str()
        .expect("the failed fixture carries the sentence");

    let mut flow = flow_with_a_code();
    flow.apply(&frame(LINK_FAILED));
    assert_eq!(
        flow.reason(),
        Some(sentence),
        "the client shows the daemon's sentence, it does not write its own"
    );

    // A state frame without a reason clears the one before, so a later failure cannot show a
    // stale sentence.
    flow.apply(&frame(LINK_WAITING));
    assert_eq!(flow.reason(), None);
}

#[test]
fn a_state_this_client_does_not_know_keeps_the_code() {
    let mut flow = flow_with_a_code();
    let unknown = frame(r#"{"v":1,"op":"link_state","state":"expired"}"#);
    assert_eq!(
        flow.apply(&unknown),
        Some(LinkUpdate::State(LinkState::Unknown))
    );
    assert_eq!(flow.phase(), Some(LinkState::Unknown));
    assert!(
        flow.qr().is_some(),
        "a state this build does not know ends nothing"
    );
}

#[test]
fn a_frame_outside_the_flow_is_left_to_the_client() {
    let mut flow = flow_with_a_code();
    assert_eq!(flow.apply(&frame(EVENT_MESSAGE)), None);
    assert_eq!(
        flow.apply(&frame(LINK_WAITING)),
        Some(LinkUpdate::State(LinkState::Waiting))
    );
    // The state of the flow it folded is untouched by the frame it skipped.
    assert_eq!(flow.phase(), Some(LinkState::Waiting));
    assert!(flow.qr().is_some());
}

#[test]
fn a_flow_never_debugs_its_matrix() {
    // The code is pairing material: a `{:?}` of the flow must not put it in a log line.
    let flow = flow_with_a_code();
    let printed = format!("{flow:?}");
    let qr = flow.qr().expect("a code");
    for row in qr.rows() {
        assert!(!printed.contains(row.as_str()), "{printed}");
    }
    assert!(printed.contains("21"), "the size stays: {printed}");
}
