use super::*;
use nuncio_client::wire::{MessageState, PROTOCOL_VERSION};
use nuncio_client::{Chat, ClientError, DaemonFrame, Message};

const ANA: &str = "15551230001@s.whatsapp.net";
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

fn message(id: &str, kind: &str) -> Message {
    Message {
        id: id.to_owned(),
        kind: kind.to_owned(),
        text: "hello".to_owned(),
        from_me: false,
        timestamp: "2026-10-20T10:00:00Z".to_owned(),
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
    }
}

fn reply(request: &ClientFrame, frame: DaemonFrame) -> Incoming {
    Incoming::Reply {
        request: request.clone(),
        result: Ok(frame),
    }
}

fn sent(commands: Vec<Command>) -> ClientFrame {
    match commands.as_slice() {
        [Command::Send(frame)] => frame.clone(),
        other => panic!("expected one send, got {other:?}"),
    }
}

/// Connected, with Ana (unread `unread`) and Ben listed.
fn connected(unread: u32) -> Controller {
    let mut c = Controller::new("d");
    let frames = c.incoming(Incoming::Connected);
    let chats = frames
        .iter()
        .find_map(|cmd| match cmd {
            Command::Send(frame @ ClientFrame::Chats { .. }) => Some(frame.clone()),
            _ => None,
        })
        .expect("asks for the chats");
    c.incoming(reply(
        &chats,
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana", unread), chat(BEN, "Ben", 0)],
        },
    ));
    c
}

/// Ana's chat opened and its `messages` (newest first) shown.
fn opened(messages: Vec<Message>) -> Controller {
    let mut c = connected(0);
    let request = sent(c.open_chat(0));
    c.incoming(reply(
        &request,
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages,
        },
    ));
    c
}

#[test]
fn the_first_connect_asks_for_the_status_and_chats_and_says_nothing() {
    let mut c = Controller::new("d");
    assert!(
        !c.model().connected(),
        "nothing is connected before the worker says so"
    );
    let frames = c.incoming(Incoming::Connected);
    assert_eq!(
        frames,
        [
            Command::Send(ClientFrame::status()),
            Command::Send(ClientFrame::chats(200))
        ]
    );
    assert!(c.model().connected());
    assert_eq!(c.model().notice(), None);
}

#[test]
fn a_reconnect_says_so_and_resyncs_the_open_chat() {
    let mut c = opened(Vec::new());
    c.incoming(Incoming::Lost(ClientError::Closed));
    assert!(!c.model().connected());
    assert!(
        c.model()
            .notice()
            .is_some_and(|n| n.ends_with("; reconnecting…")),
        "{:?}",
        c.model().notice()
    );
    let frames = c.incoming(Incoming::Connected);
    assert_eq!(c.model().notice(), Some("reconnected to nunciod"));
    assert_eq!(
        frames.last(),
        Some(&Command::Send(ClientFrame::messages(ANA, 200))),
        "the selected chat's messages are asked again"
    );
}

#[test]
fn opening_a_chat_with_unread_messages_marks_it_read_once_they_are_shown() {
    let mut c = connected(2);
    let request = sent(c.open_chat(0));
    assert_eq!(request, ClientFrame::messages(ANA, 200));
    assert!(c.model().is_open());
    let next = c.incoming(reply(
        &request,
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("M1", "text")],
        },
    ));
    assert_eq!(next, [Command::Send(ClientFrame::mark_read(ANA))]);
    assert!(c.open_chat(9).is_empty(), "no such chat");
}

#[test]
fn a_new_message_in_the_open_chat_is_marked_read_but_not_in_another() {
    let mut c = opened(Vec::new());
    assert!(c.set_seen(true).is_empty(), "nothing unread yet");
    let event = |jid: &str| {
        Incoming::Event(DaemonFrame::EventMessage {
            v: PROTOCOL_VERSION,
            jid: jid.to_owned(),
            message: message("M7", "text"),
        })
    };
    assert_eq!(c.incoming(event(BEN)), []);
    assert_eq!(
        c.incoming(event(ANA)),
        [Command::Send(ClientFrame::mark_read(ANA))]
    );
}

#[test]
fn a_draft_is_sent_once_and_a_blank_one_is_not() {
    let mut c = opened(Vec::new());
    c.set_draft("   ");
    assert_eq!(c.send(), (false, Vec::new()));
    c.set_draft("on my way\nsoon");
    let (sent_it, commands) = c.send();
    assert!(sent_it);
    assert_eq!(
        commands,
        [Command::Send(ClientFrame::send(
            ANA,
            "on my way\nsoon",
            "d-1"
        ))]
    );
    assert!(c.model().composer().is_empty());
    assert!(c.take_view_reset());
}

#[test]
fn a_send_while_disconnected_is_not_sent_and_says_how_to_retry() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    let frame = sent(c.send().1);
    c.incoming(Incoming::NotConnected(frame));
    assert_eq!(c.model().loaded()[0].state, Some(MessageState::NotSent));
    assert_eq!(
        c.model().notice(),
        Some("not connected to nunciod: not sent (choose Retry on the message once reconnected)")
    );
    assert_eq!(c.retry(0), [Command::Send(ClientFrame::retry("d-1"))]);
}

#[test]
fn a_reply_that_breaks_the_connection_marks_the_send_not_sent_and_disconnects() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    let frame = sent(c.send().1);
    c.incoming(Incoming::Reply {
        request: frame,
        result: Err(ClientError::Closed),
    });
    assert!(!c.model().connected());
    assert_eq!(c.model().loaded()[0].state, Some(MessageState::NotSent));
}

#[test]
fn reply_react_and_open_media_act_on_the_message_they_name() {
    let mut c = opened(vec![message("M2", "document"), message("M1", "text")]);
    assert!(c.reply(0));
    assert_eq!(c.model().composer().reply_to(), Some("M1"));
    c.cancel_reply();
    assert_eq!(c.model().composer().reply_to(), None);
    assert_eq!(
        c.react(0, "👍"),
        [Command::Send(ClientFrame::react(ANA, "M1", "👍"))]
    );
    assert!(c.open_media(0).is_empty(), "a text message has no file");
    let download = sent(c.open_media(1));
    assert_eq!(download, ClientFrame::download_media(ANA, "M2"));
    let ready = c.incoming(reply(
        &download,
        DaemonFrame::MediaReady {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            id: "M2".to_owned(),
            path: "/tmp/nuncio-media/M2.pdf".to_owned(),
            mime: "application/pdf".to_owned(),
        },
    ));
    assert_eq!(
        ready,
        [Command::Open(PathBuf::from("/tmp/nuncio-media/M2.pdf"))]
    );
    assert!(c.react(7, "👍").is_empty() && c.retry(7).is_empty());
}

#[test]
fn a_send_still_sending_cannot_be_replied_to_and_the_notice_says_why() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    c.send();
    assert!(!c.reply(0));
    assert_eq!(
        c.take_notice().as_deref(),
        Some("that message was not sent, so it cannot be replied to")
    );
    assert_eq!(c.take_notice(), None, "a notice is shown once");
}

#[test]
fn opening_another_chat_drops_the_reply_and_the_same_one_keeps_it() {
    let mut c = opened(vec![message("M1", "text")]);
    assert!(c.reply(0));
    c.open_chat(0);
    assert_eq!(c.model().composer().reply_to(), Some("M1"));
    c.open_chat(1);
    assert_eq!(c.model().composer().reply_to(), None);
}

#[test]
fn a_search_asks_the_daemon_and_a_result_jumps_to_its_message() {
    let mut c = opened(vec![message("M2", "text"), message("M1", "text")]);
    assert!(c.search("   ").is_empty(), "a blank query asks nothing");
    let request = sent(c.search("hello"));
    assert_eq!(request, ClientFrame::search(ANA, "hello", 50));
    c.incoming(reply(
        &request,
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("M1", "text"), message("OLD", "text")],
        },
    ));
    assert_eq!(c.jump_to_result(0), Some(0));
    assert!(
        c.model().transcript().search().is_none(),
        "the search closes"
    );
    c.search("x").first().expect("searches again");
    assert_eq!(c.jump_to_result(5), None);
}

#[test]
fn a_new_message_is_not_marked_read_while_the_window_is_not_seen() {
    let mut c = opened(Vec::new());
    assert!(
        !c.model().is_seen(),
        "unseen until the window says it is active"
    );
    let event = Incoming::Event(DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: ANA.to_owned(),
        message: message("M7", "text"),
    });
    assert_eq!(c.incoming(event), []);
    c.incoming(Incoming::Event(DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 1,
        jid: ANA.to_owned(),
        chat_unread: 1,
    }));
    assert_eq!(
        c.set_seen(true),
        [Command::Send(ClientFrame::mark_read(ANA))],
        "seen again, the open chat is read"
    );
}

#[test]
fn an_overflow_resyncs_and_says_updates_were_missed() {
    let mut c = opened(Vec::new());
    let frames = c.incoming(Incoming::Overflow);
    assert_eq!(
        frames,
        [
            Command::Send(ClientFrame::status()),
            Command::Send(ClientFrame::chats(200)),
            Command::Send(ClientFrame::messages(ANA, 200))
        ]
    );
    assert!(c.model().connected());
    assert_eq!(
        c.model().notice(),
        Some("too many updates at once; reloading the chats")
    );
}

#[test]
fn a_draft_for_a_chat_that_was_merged_away_is_not_sent_until_a_chat_is_chosen() {
    let mut c = opened(Vec::new());
    c.set_draft("for Ana");
    // The daemon merged Ana's number chat away: the list no longer has it.
    c.incoming(reply(
        &ClientFrame::chats(200),
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(BEN, "Ben", 0)],
        },
    ));
    assert!(c.model().chat_list().orphan().is_some());
    assert_eq!(
        c.send(),
        (false, Vec::new()),
        "a draft never changes recipient"
    );
    assert_eq!(
        c.take_notice().as_deref(),
        Some(nuncio_client::model::MERGED_NOTICE)
    );
    assert_eq!(c.model().composer().text(), "for Ana", "the draft stays");
    c.open_chat(0);
    let (sent_it, commands) = c.send();
    assert!(sent_it);
    assert_eq!(
        commands,
        [Command::Send(ClientFrame::send(BEN, "for Ana", "d-1"))]
    );
}

#[test]
fn a_send_answered_not_confirmed_is_shown_so_and_can_be_retried() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    let frame = sent(c.send().1);
    c.incoming(Incoming::Reply {
        request: frame,
        result: Err(ClientError::Daemon {
            code: "not_confirmed".to_owned(),
            message: "the acknowledgement timed out".to_owned(),
        }),
    });
    let shown = &c.model().loaded()[0];
    assert_eq!(shown.state, Some(MessageState::NotSent));
    assert!(c.model().transcript().is_unconfirmed(shown));
    assert!(c.model().connected(), "an error frame is not a loss");
    assert_eq!(
        c.model().notice(),
        Some("daemon: not_confirmed: the acknowledgement timed out")
    );
    assert_eq!(c.retry(0), [Command::Send(ClientFrame::retry("d-1"))]);
}
