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
            stop_reason: None,
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
    // nunciod publishes the send it refused under the WhatsApp id it stored it with.
    let mut refused = message("3EB0D1", true);
    refused.state = Some(MessageState::NotSent);
    refused.client_id = Some("t-1".to_owned());
    m.apply(&DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        message: refused,
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

const CAL_PN: &str = "15551230003@s.whatsapp.net";

#[test]
fn opening_a_listed_jid_selects_and_opens_it() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 2)]);
    assert_eq!(
        m.open_jid(BEN),
        Some(ClientFrame::messages(BEN, MESSAGE_LIMIT))
    );
    assert_eq!(m.selected_jid(), Some(BEN));
    assert!(m.is_open());
    assert_eq!(m.chat_list().chats().len(), 2, "nothing is added");
    assert_eq!(
        m.apply_reply(
            &ClientFrame::messages(BEN, MESSAGE_LIMIT),
            &messages_frame(BEN, Vec::new())
        ),
        Some(Effect::Request(ClientFrame::mark_read(BEN))),
        "opened as a click opens it: read once shown"
    );
}

#[test]
fn opening_a_number_merged_into_a_lid_chat_opens_the_lid_chat() {
    let mut ana = chat(ANA_LID, "Ana", 0);
    ana.aliases = vec![ANA_PN.to_owned()];
    let mut m = model(vec![chat(BEN, "Ben", 0), ana]);
    assert_eq!(
        m.open_jid(ANA_PN),
        Some(ClientFrame::messages(ANA_LID, MESSAGE_LIMIT))
    );
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    assert_eq!(m.chat_list().chats().len(), 2);
}

#[test]
fn opening_an_unknown_number_adds_an_empty_chat_that_a_new_list_keeps() {
    let mut archived = chat(BEN, "Ben", 0);
    archived.archived = true;
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), archived.clone()]);
    assert_eq!(
        m.open_jid(CAL_PN),
        Some(ClientFrame::messages(CAL_PN, MESSAGE_LIMIT))
    );
    assert_eq!(m.selected_jid(), Some(CAL_PN));
    let order: Vec<&str> = m
        .chat_list()
        .chats()
        .iter()
        .map(|c| c.jid.as_str())
        .collect();
    assert_eq!(order, [ANA_PN, CAL_PN, BEN], "archived chats stay last");
    type_draft(&mut m, "hola");
    // A resync before the first send: the daemon does not list the new chat yet.
    m.apply(&chats_frame(vec![chat(ANA_PN, "Ana", 0), archived.clone()]));
    assert_eq!(
        m.selected_jid(),
        Some(CAL_PN),
        "the new chat stays selected"
    );
    assert!(m.chat_list().orphan().is_none(), "it is not a merged chat");
    assert_eq!(m.notice(), None);
    let frame = m.send().expect("the first send goes to the new chat");
    assert_eq!(frame, ClientFrame::send(CAL_PN, "hola", "t-1"));
    // Once the daemon lists it, the listed entry is the one kept.
    let mut listed = chat(CAL_PN, "Cal", 0);
    listed.unread = 0;
    m.apply(&chats_frame(vec![listed, chat(ANA_PN, "Ana", 0), archived]));
    assert_eq!(m.chat_list().chats().len(), 3);
    assert_eq!(m.selected_chat().map(|c| c.name.as_str()), Some("Cal"));
}

#[test]
fn a_new_chat_answered_as_a_lid_chat_is_adopted() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let request = m.open_jid(ANA_PN).expect("opens");
    m.apply_reply(&request, &messages_frame(ANA_LID, Vec::new()));
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    let mut ana = chat(ANA_LID, "Ana", 0);
    ana.aliases = vec![ANA_PN.to_owned()];
    m.apply(&chats_frame(vec![chat(BEN, "Ben", 0), ana]));
    let jids: Vec<&str> = m
        .chat_list()
        .chats()
        .iter()
        .map(|c| c.jid.as_str())
        .collect();
    assert_eq!(jids, [BEN, ANA_LID], "no leftover entry for the number");
    assert_eq!(m.selected_jid(), Some(ANA_LID));
}

#[test]
fn a_new_chat_put_where_the_selected_chat_was_still_resets_the_view() {
    let mut ben = chat(BEN, "Ben", 0);
    ben.archived = true;
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), ben]);
    m.select_chat(1);
    m.open_selected();
    m.apply(&messages_frame(BEN, vec![message("M1", false)]));
    assert!(m.select_message(0));
    m.take_view_reset();
    // The new chat goes before the archived ones: at Ben's own index.
    m.open_jid(CAL_PN);
    assert_eq!(m.chat_list().selected(), 1);
    assert!(m.take_view_reset(), "another chat: back to the newest");
    assert_eq!(m.transcript().cursor(), None, "no message carried over");
}

#[test]
fn a_new_chat_answered_as_a_lid_chat_stays_listed_until_the_daemon_lists_it() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let request = m.open_jid(ANA_PN).expect("opens");
    m.apply_reply(&request, &messages_frame(ANA_LID, Vec::new()));
    type_draft(&mut m, "hola");
    m.apply(&chats_frame(vec![chat(BEN, "Ben", 0)]));
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    assert!(m.chat_list().orphan().is_none(), "not a merged chat");
    assert_eq!(m.send(), Some(ClientFrame::send(ANA_LID, "hola", "t-1")));
}

/// The home report (B3): a send made while nunciod was down never reached it, so nunciod has no
/// send under that `client_id` and a `retry` of it is refused ("no send with that client_id").
/// The explicit retry sends it instead, once, under the same `client_id`.
#[test]
fn a_send_that_never_reached_the_daemon_is_sent_again_under_its_client_id() {
    use crate::driver::{self, Client};
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let open = m.open_selected().expect("opens");
    m.apply_reply(&open, &messages_frame(BEN, vec![message("M1", false)]));
    type_draft(&mut m, "while it is down");
    let frame = m.send().expect("sends");
    let client = Client {
        program: "test",
        retry_hint: "R retries it once reconnected",
    };
    driver::not_connected(&mut m, &frame, client);
    // The reconnect's resync lists only what nunciod stored, and the send stays shown.
    for resync in driver::reconnected(&mut m) {
        if let ClientFrame::Messages { .. } = resync {
            m.apply_reply(&resync, &messages_frame(BEN, vec![message("M1", false)]));
        }
    }
    assert!(m.select_message(1));
    assert_eq!(
        m.selected_message().map(|s| s.state.clone()),
        Some(Some(MessageState::NotSent))
    );
    assert_eq!(
        m.retry_selected(),
        Some(ClientFrame::send(BEN, "while it is down", "t-1"))
    );
    assert_eq!(m.retry_selected(), None, "one attempt in flight at a time");
}

/// A reply that never reached the daemon is sent again still quoting what it quoted.
#[test]
fn a_reply_that_never_reached_the_daemon_is_sent_again_as_the_same_reply() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let open = m.open_selected().expect("opens");
    m.apply_reply(&open, &messages_frame(BEN, vec![message("M1", false)]));
    assert!(m.select_message(0));
    assert!(m.reply_to_selected());
    type_draft(&mut m, "yes");
    m.send().expect("sends");
    m.apply(&DaemonFrame::SendFailed {
        v: PROTOCOL_VERSION,
        client_id: "t-1".to_owned(),
        message: crate::driver::NOT_CONNECTED.to_owned(),
    });
    assert!(m.select_message(1));
    assert_eq!(
        m.retry_selected(),
        Some(ClientFrame::send_reply(BEN, "yes", "t-1", "M1"))
    );
}

/// A send nunciod answered `not_confirmed` is stored there: the explicit retry is a `retry`, one
/// at a time, never a new send.
#[test]
fn a_send_shown_as_not_confirmed_is_retried_once_under_its_client_id() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    m.open_selected();
    type_draft(&mut m, "hi");
    m.send().expect("sends");
    m.send_not_confirmed("t-1");
    assert!(m.select_message(0));
    assert!(
        m.transcript()
            .is_unconfirmed(m.selected_message().expect("selected"))
    );
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("t-1")));
    assert_eq!(m.retry_selected(), None, "one attempt in flight at a time");
}

/// A send nunciod stored and published as not sent (its WhatsApp id with `state: "not_sent"`, as
/// it lists a `not_confirmed` one too, after a restart) is retried, and a sent or received
/// message offers no retry.
#[test]
fn a_stored_not_sent_send_is_retried_and_a_sent_or_received_one_is_not() {
    let mut m = model(vec![chat(BEN, "Ben", 0)]);
    let open = m.open_selected().expect("opens");
    let mut stored: Message = message("3EB0BBB3", true);
    stored.state = Some(MessageState::NotSent);
    stored.client_id = Some("tui-9".to_owned());
    m.apply_reply(
        &open,
        &messages_frame(BEN, vec![stored, message("M2", true), message("M1", false)]),
    );
    assert!(m.select_message(0), "the received one");
    assert_eq!(m.retry_selected(), None);
    assert!(m.select_message(1), "my sent one");
    assert_eq!(m.retry_selected(), None);
    assert!(m.select_message(2));
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("tui-9")));
    assert_eq!(m.retry_selected(), None, "one attempt in flight at a time");
}

// R9 (S8): each chat keeps its own draft and its own reply in progress.
#[test]
fn each_chat_keeps_its_own_draft_and_sending_empties_only_the_open_one() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    type_draft(&mut m, "meant for ana");
    m.select_chat(1);
    assert_eq!(m.selected_jid(), Some(BEN));
    assert_eq!(
        m.composer().text(),
        "",
        "Ana's draft never shows in Ben's chat"
    );
    type_draft(&mut m, "for ben");
    m.select_chat(0);
    assert_eq!(m.composer().text(), "meant for ana");
    assert_eq!(m.composer().caret(), "meant for ana".len());
    m.select_chat(1);
    assert_eq!(m.composer().text(), "for ben");
    assert_eq!(
        m.send(),
        Some(ClientFrame::send(BEN, "for ben", "t-1")),
        "only the open chat's draft is sent"
    );
    assert!(m.composer().is_empty());
    m.select_chat(0);
    assert_eq!(
        m.composer().text(),
        "meant for ana",
        "Ana's draft is untouched"
    );
}

#[test]
fn opening_a_chat_from_outside_the_list_shows_its_own_draft() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    type_draft(&mut m, "meant for ana");
    m.open_jid("15550009999@s.whatsapp.net");
    assert_eq!(m.composer().text(), "", "a new chat starts empty");
    m.open_jid(ANA_PN);
    assert_eq!(m.composer().text(), "meant for ana");
}

#[test]
fn each_chat_keeps_its_own_reply_in_progress() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    m.open_selected();
    m.apply(&messages_frame(ANA_PN, vec![message("M1", false)]));
    assert!(m.select_message(0));
    assert!(m.reply_to_selected());
    type_draft(&mut m, "yes");
    m.select_chat(1);
    assert_eq!(m.composer().reply_to(), None, "Ben's draft quotes nothing");
    m.select_chat(0);
    assert_eq!(m.composer().reply_to(), Some("M1"));
    assert_eq!(
        m.send(),
        Some(ClientFrame::send_reply(ANA_PN, "yes", "t-1", "M1"))
    );
}

#[test]
fn a_draft_follows_its_chat_when_the_chat_is_renamed_to_its_lid() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    type_draft(&mut m, "meant for ana");
    m.select_chat(1);
    let mut ana = chat(ANA_LID, "Ana", 0);
    ana.aliases = vec![ANA_PN.to_owned()];
    m.apply(&chats_frame(vec![ana, chat(BEN, "Ben", 0)]));
    assert_eq!(m.selected_jid(), Some(BEN));
    assert_eq!(m.composer().text(), "");
    m.open_jid(ANA_LID);
    assert_eq!(
        m.composer().text(),
        "meant for ana",
        "the draft moved with Ana"
    );
}

#[test]
fn a_draft_merged_into_a_chat_with_its_own_draft_keeps_both_texts() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(ANA_LID, "Ana", 0)]);
    type_draft(&mut m, "one");
    m.select_chat(1);
    type_draft(&mut m, "two");
    m.adopt(ANA_PN, ANA_LID);
    assert_eq!(m.selected_jid(), Some(ANA_LID));
    assert_eq!(
        m.composer().text(),
        "two\none",
        "no draft is lost in a merge"
    );
}

#[test]
fn choosing_another_chat_after_a_merge_never_carries_the_vanished_draft_there() {
    let mut m = model(vec![chat(ANA_PN, "Ana", 0), chat(BEN, "Ben", 0)]);
    m.set_open(true);
    type_draft(&mut m, "for Ana only");
    m.apply(&chats_frame(vec![chat(BEN, "Ben", 0)]));
    assert_eq!(
        m.composer().text(),
        "for Ana only",
        "the vanished chat keeps it"
    );
    m.select_chat(0);
    assert_eq!(m.selected_jid(), Some(BEN));
    assert_eq!(m.composer().text(), "");
    assert_eq!(m.send(), None, "nothing is sent to Ben");
    // The daemon later names the chat Ana's number became: her draft is there.
    let mut ana = chat(ANA_LID, "Ana", 0);
    ana.aliases = vec![ANA_PN.to_owned()];
    m.apply(&chats_frame(vec![ana, chat(BEN, "Ben", 0)]));
    m.open_jid(ANA_LID);
    assert_eq!(m.composer().text(), "for Ana only");
}
