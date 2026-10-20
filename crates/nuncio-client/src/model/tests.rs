//! The rules that span the chat list, the transcript and the composer. The terminal client's
//! integration tests (`crates/nuncio-tui/tests`) cover them end to end through keys and a daemon;
//! these pin each one where it lives.

use super::*;
use crate::wire::{PROTOCOL_VERSION, SendState};

const ANA_PN: &str = "15551230001@s.whatsapp.net";
const ANA_LID: &str = "99000000000001@lid";
const BEN: &str = "15551230002@s.whatsapp.net";

fn chat(jid: &str, name: &str, unread: u32) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: name.to_owned(),
        unread,
        archived: false,
        pinned: false,
        muted: false,
        aliases: Vec::new(),
        phone: None,
        last_message: None,
    }
}

fn message(id: &str, from_me: bool) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "kind": "text", "text": "hello", "from_me": from_me,
        "timestamp": "2026-10-20T10:00:00Z",
    }))
    .expect("a message")
}

fn model(chats: Vec<Chat>) -> Model {
    Model::new(
        Snapshot {
            daemon: "test".to_owned(),
            linked: true,
            session: Session::Linked,
            unread: 0,
            chats,
        },
        "t",
    )
}

fn chats_frame(chats: Vec<Chat>) -> DaemonFrame {
    DaemonFrame::Chats {
        v: PROTOCOL_VERSION,
        chats,
    }
}

fn messages_frame(jid: &str, messages: Vec<Message>) -> DaemonFrame {
    DaemonFrame::Messages {
        v: PROTOCOL_VERSION,
        jid: jid.to_owned(),
        messages,
    }
}

fn type_draft(model: &mut Model, text: &str) {
    text.chars().for_each(|c| model.composer_mut().insert(c));
}

#[test]
fn a_listed_alias_moves_the_conversation_and_the_selection_to_the_lid_chat() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    m.set_open(true);
    m.apply(&messages_frame(ANA_PN, vec![message("M1", false)]));
    let mut ana = chat(ANA_LID, "Ana", 0);
    ana.aliases = vec![ANA_PN.to_owned()];
    m.apply(&chats_frame(vec![ana, chat(BEN, "Ben", 0)]));
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    assert_eq!(
        m.loaded().len(),
        1,
        "the PN chat's messages are the LID chat's"
    );
    assert_eq!(m.chat_list().canonical(ANA_PN), Some(ANA_LID));
    assert!(m.chat_list().orphan().is_none());
}

#[test]
fn a_vanished_open_chat_keeps_the_draft_and_blocks_the_send() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    m.set_open(true);
    type_draft(&mut m, "for Ana only");
    // Ana's chat is gone with no alias naming where it went.
    m.apply(&chats_frame(vec![chat(BEN, "Ben", 0)]));
    assert_eq!(m.selected_jid(), Some(ANA_PN));
    assert_eq!(m.chat_list().highlighted(), None);
    assert_eq!(m.send(), None, "no Send to another contact");
    assert_eq!(m.composer().text(), "for Ana only");
    assert_eq!(m.notice(), Some(MERGED_NOTICE));
}

#[test]
fn a_reply_naming_another_chat_is_adopted_only_when_paired_with_its_request() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0)]);
    let request = m.open_selected().expect("opens");
    // An unpaired messages frame for the LID chat adopts nothing.
    m.apply(&messages_frame(ANA_LID, vec![message("M0", false)]));
    assert_eq!(m.selected_jid(), Some(ANA_PN));
    m.apply_reply(
        &request,
        &messages_frame(ANA_LID, vec![message("M1", false)]),
    );
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    assert_eq!(m.chat_list().chats()[0].jid, ANA_LID);
}

#[test]
fn only_one_mark_read_is_in_flight_per_chat() {
    let mut m = model(vec![chat(BEN, "Ben", 2)]);
    let request = m.open_selected().expect("opens");
    let first = m.apply_reply(&request, &messages_frame(BEN, Vec::new()));
    assert_eq!(
        first,
        Some(Effect::Request(ClientFrame::mark_read(BEN))),
        "an unread chat opened is marked read"
    );
    let event = DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        message: message("M2", false),
    };
    assert_eq!(m.apply(&event), None, "one is already in flight");
    m.apply(&DaemonFrame::MarkedRead {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        unread: 0,
    });
    let event = DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        message: message("M3", false),
    };
    assert!(
        m.apply(&event).is_some(),
        "the open chat reads a new message"
    );
}

#[test]
fn a_send_is_shown_at_once_and_settled_by_its_reply() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    m.open_selected();
    type_draft(&mut m, "On my way");
    let frame = m.send().expect("sends");
    assert_eq!(frame, ClientFrame::send(BEN, "On my way", "t-1"));
    assert!(m.composer().is_empty());
    assert!(m.take_view_reset(), "a send goes back to the newest");
    assert!(m.transcript().is_sending(&m.loaded()[0]));
    m.apply(&DaemonFrame::Sent {
        v: PROTOCOL_VERSION,
        client_id: "t-1".to_owned(),
        state: SendState::Sent,
        id: Some("3EB0".to_owned()),
        timestamp: Some("2026-10-20T10:01:00Z".to_owned()),
    });
    assert_eq!(m.loaded()[0].id, "3EB0");
    assert!(!m.transcript().is_sending(&m.loaded()[0]));
}

#[test]
fn search_results_fill_the_search_not_the_conversation() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    assert!(m.open_search());
    "late".chars().for_each(|c| m.search_type(c));
    let request = m.submit_search().expect("searches");
    assert_eq!(request, ClientFrame::search(BEN, "late", SEARCH_LIMIT));
    m.apply_reply(&request, &messages_frame(BEN, vec![message("M9", false)]));
    let search = m.transcript().search().expect("open");
    assert_eq!(search.results().map(<[Message]>::len), Some(1));
    assert_eq!(search.selected(), Some(0));
    assert!(m.transcript().conversation(BEN).is_none());
}

#[test]
fn a_notice_taken_is_shown_once() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    m.set_notice("daemon: internal: oops");
    assert_eq!(m.take_notice().as_deref(), Some("daemon: internal: oops"));
    assert_eq!(m.notice(), None);
    assert_eq!(m.take_notice(), None);
}

#[test]
fn a_retry_waiting_for_its_answer_is_not_asked_again() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    m.open_selected();
    type_draft(&mut m, "hi");
    m.send().expect("sends");
    m.apply(&DaemonFrame::SendFailed {
        v: PROTOCOL_VERSION,
        client_id: "t-1".to_owned(),
        message: "no".to_owned(),
    });
    assert!(m.select_message(0));
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("t-1")));
    assert_eq!(m.retry_selected(), None, "one attempt in flight at a time");
    // Its answer ends the attempt: not sent again, so one more retry may be asked for.
    m.apply(&DaemonFrame::SendFailed {
        v: PROTOCOL_VERSION,
        client_id: "t-1".to_owned(),
        message: "no".to_owned(),
    });
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("t-1")));
}

#[test]
fn a_new_message_in_the_open_chat_is_read_only_while_the_view_is_seen() {
    let incoming = |id: &str| DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        message: message(id, false),
    };
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let request = m.open_selected().expect("opens");
    m.apply_reply(&request, &messages_frame(BEN, Vec::new()));
    assert!(m.is_seen(), "a view is seen unless the UI says otherwise");
    m.set_seen(false);
    assert_eq!(
        m.apply(&incoming("M1")),
        None,
        "a hidden window marks nothing read"
    );
    m.apply(&DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 1,
        jid: BEN.to_owned(),
        chat_unread: 1,
    });
    // Seen again: the open chat's unread messages are read now.
    assert_eq!(
        m.set_seen(true),
        Some(Effect::Request(ClientFrame::mark_read(BEN)))
    );
    m.apply(&DaemonFrame::MarkedRead {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        unread: 0,
    });
    assert_eq!(
        m.apply(&incoming("M2")),
        Some(Effect::Request(ClientFrame::mark_read(BEN)))
    );
    assert_eq!(m.set_seen(true), None, "nothing unread, nothing to mark");
}
