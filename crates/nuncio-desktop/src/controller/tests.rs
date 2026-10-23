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
        sender: None,
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
    // nunciod never saw it, so it would refuse a `retry`: Retry sends it, once, under its id.
    assert_eq!(
        c.retry(0),
        [Command::Send(ClientFrame::send(ANA, "hi", "d-1"))]
    );
    assert_eq!(c.retry(0), [], "one attempt at a time");
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
    // Choosing Ben shows Ben's own draft: Ana's never goes to him (R9).
    c.open_chat(0);
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert_eq!(c.model().composer().text(), "");
    assert!(c.take_draft_changed(), "the composer widget shows Ben's");
    assert_eq!(c.send(), (false, Vec::new()), "nothing is sent to Ben");
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

/// D1: a send whose life ended while it waited is answered as a late reply: unanswered, it is not
/// confirmed, and the link that came back meanwhile stays connected; sent, it settles its send.
#[test]
fn a_late_reply_marks_its_own_send_and_is_no_loss_of_the_current_link() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    let frame = sent(c.send().1);
    c.set_draft("again");
    let second = sent(c.send().1);
    c.incoming(Incoming::LateReply {
        request: frame,
        result: Err(ClientError::SendUnanswered(std::time::Duration::from_secs(
            25,
        ))),
    });
    assert!(
        c.model().connected(),
        "the old life's loss was already shown"
    );
    let first = &c.model().loaded()[0];
    assert!(c.model().transcript().is_unconfirmed(first));
    assert!(c.model().transcript().is_sending(&c.model().loaded()[1]));
    let ClientFrame::Send { client_id, .. } = &second else {
        panic!("a send");
    };
    c.incoming(Incoming::LateReply {
        request: second.clone(),
        result: Ok(DaemonFrame::Sent {
            v: PROTOCOL_VERSION,
            client_id: client_id.clone(),
            state: nuncio_client::SendState::Sent,
            id: Some("M2".to_owned()),
            timestamp: Some("2026-10-22T10:00:00Z".to_owned()),
        }),
    });
    assert_eq!(c.model().loaded()[1].id, "M2");
    assert!(
        c.model()
            .transcript()
            .is_unconfirmed(&c.model().loaded()[0])
    );
    assert!(c.model().connected());
}

/// A send the outbox could not even connect for, answered after its life ended (the event
/// connection noticed the loss first): it never reached the daemon, so it is not sent, not "not
/// confirmed", and the loss already shown is not shown again.
#[test]
fn a_late_reply_that_never_connected_marks_the_send_not_sent() {
    let mut c = opened(Vec::new());
    c.set_draft("hi");
    let frame = sent(c.send().1);
    c.incoming(Incoming::Lost(ClientError::Closed));
    c.incoming(Incoming::Connected);
    c.incoming(Incoming::LateReply {
        request: frame,
        result: Err(ClientError::Connect {
            path: "/nonexistent/core.sock".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }),
    });
    let shown = &c.model().loaded()[0];
    assert_eq!(shown.state, Some(MessageState::NotSent));
    assert!(!c.model().transcript().is_unconfirmed(shown));
    assert!(!c.model().transcript().is_sending(shown));
    assert!(c.model().connected(), "the new link stays");
    assert_eq!(c.model().notice(), Some("reconnected to nunciod"));
}

// Notifications, the `open-chat` action and links (D3b).

use nuncio_client::Config;
use nuncio_client::notify::Notification;

const CAL: &str = "15551230003@s.whatsapp.net";

fn incoming_message(jid: &str, id: &str) -> Incoming {
    Incoming::Event(DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: jid.to_owned(),
        message: message(id, "text"),
    })
}

fn shown(alerts: Vec<Alert>) -> Vec<Notification> {
    alerts
        .into_iter()
        .map(|alert| match alert {
            Alert::Show(n) => n,
            other => panic!("expected only shown notifications, got {other:?}"),
        })
        .collect()
}

/// Every frame the commands would hand the worker.
fn frames(commands: &[Command]) -> Vec<ClientFrame> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Send(frame) => Some(frame.clone()),
            Command::Open(_) => None,
        })
        .collect()
}

fn is_a_send(frame: &ClientFrame) -> bool {
    matches!(
        frame,
        ClientFrame::Send { .. } | ClientFrame::Retry { .. } | ClientFrame::React { .. }
    )
}

#[test]
fn an_incoming_message_shows_the_notification_the_client_computed() {
    let mut c = connected(0);
    c.incoming(incoming_message(BEN, "M1"));
    assert_eq!(
        shown(c.take_alerts()),
        [Notification {
            chat_jid: BEN.to_owned(),
            title: "Ben".to_owned(),
            body: "New message".to_owned(),
            message_id: "M1".to_owned(),
        }],
        "no message text unless show_preview"
    );
    assert!(c.take_alerts().is_empty(), "alerts are handed out once");
    assert_eq!(notification_id(BEN), notification_id(BEN));
    assert_ne!(
        notification_id(BEN),
        notification_id(ANA),
        "one id per chat"
    );
    assert_eq!(OPEN_CHAT_ACTION, "app.open-chat");
}

#[test]
fn show_preview_and_disabled_notifications_follow_the_config() {
    let mut config = Config::default();
    config.notifications.show_preview = true;
    let mut c = connected(0);
    c.set_config(&config);
    c.incoming(incoming_message(BEN, "M1"));
    assert_eq!(shown(c.take_alerts())[0].body, "hello");
    config.notifications.enabled = false;
    c.set_config(&config);
    c.incoming(incoming_message(BEN, "M2"));
    assert!(c.take_alerts().is_empty());
}

#[test]
fn the_open_chat_notifies_only_while_the_window_is_not_seen() {
    let mut c = opened(Vec::new());
    c.set_seen(true);
    c.incoming(incoming_message(ANA, "M1"));
    assert!(c.take_alerts().is_empty(), "the chat on screen");
    c.incoming(incoming_message(BEN, "M2"));
    assert_eq!(shown(c.take_alerts())[0].chat_jid, BEN, "another chat");
    c.set_seen(false);
    c.incoming(incoming_message(ANA, "M3"));
    assert_eq!(
        shown(c.take_alerts())[0].chat_jid,
        ANA,
        "a hidden window sees nothing"
    );
}

#[test]
fn a_message_republished_for_a_reaction_does_not_notify_again() {
    let mut c = connected(0);
    c.incoming(incoming_message(BEN, "M1"));
    assert_eq!(shown(c.take_alerts()).len(), 1);
    c.incoming(incoming_message(BEN, "M1"));
    assert!(c.take_alerts().is_empty());
    c.incoming(incoming_message(BEN, "M2"));
    assert_eq!(shown(c.take_alerts()).len(), 1, "a newer one does");
}

#[test]
fn opening_a_chat_withdraws_its_notification_and_only_its_own() {
    let mut c = connected(0);
    c.incoming(incoming_message(BEN, "M1"));
    c.take_alerts();
    c.open_chat(0);
    assert!(c.take_alerts().is_empty(), "Ana had no notification");
    c.open_chat(1);
    assert_eq!(c.take_alerts(), [Alert::Withdraw(BEN.to_owned())]);
    c.open_chat(1);
    assert!(c.take_alerts().is_empty(), "withdrawn once");
}

#[test]
fn a_chat_read_here_or_elsewhere_withdraws_its_notification() {
    let mut c = connected(0);
    c.incoming(incoming_message(BEN, "M1"));
    c.incoming(incoming_message(ANA, "M2"));
    c.take_alerts();
    // Read on the phone: the daemon says the chat has nothing unread.
    c.incoming(Incoming::Event(DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 1,
        jid: BEN.to_owned(),
        chat_unread: 0,
    }));
    assert_eq!(c.take_alerts(), [Alert::Withdraw(BEN.to_owned())]);
    c.incoming(reply(
        &ClientFrame::mark_read(ANA),
        DaemonFrame::MarkedRead {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            unread: 0,
        },
    ));
    assert_eq!(c.take_alerts(), [Alert::Withdraw(ANA.to_owned())]);
}

#[test]
fn the_open_chat_action_opens_and_reads_the_chat_it_names() {
    let mut c = connected(0);
    c.incoming(Incoming::Event(DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 2,
        jid: BEN.to_owned(),
        chat_unread: 2,
    }));
    c.incoming(incoming_message(BEN, "M1"));
    c.take_alerts();
    let request = sent(c.open_jid(BEN));
    assert_eq!(request, ClientFrame::messages(BEN, 200));
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert!(c.model().is_open());
    assert_eq!(c.take_alerts(), [Alert::Withdraw(BEN.to_owned())]);
    let next = c.incoming(reply(
        &request,
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: BEN.to_owned(),
            messages: vec![message("M1", "text")],
        },
    ));
    assert_eq!(next, [Command::Send(ClientFrame::mark_read(BEN))]);
}

#[test]
fn from_a_cold_start_the_chat_opens_once_the_chat_list_is_loaded() {
    let mut c = Controller::new("d");
    assert!(c.open_jid(BEN).is_empty(), "nothing to open yet");
    let frames = frames(&c.incoming(Incoming::Connected));
    assert!(!frames.contains(&ClientFrame::messages(BEN, 200)));
    let loaded = c.incoming(reply(
        &ClientFrame::chats(200),
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana", 0), chat(BEN, "Ben", 3)],
        },
    ));
    assert_eq!(loaded, [Command::Send(ClientFrame::messages(BEN, 200))]);
    assert_eq!(c.model().selected_jid(), Some(BEN));
    let next = c.incoming(reply(
        &ClientFrame::messages(BEN, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: BEN.to_owned(),
            messages: vec![message("M1", "text")],
        },
    ));
    assert_eq!(
        next,
        [Command::Send(ClientFrame::mark_read(BEN))],
        "read once shown"
    );
}

#[test]
fn a_link_opens_the_chat_and_puts_its_text_in_the_composer_without_sending() {
    let mut c = connected(0);
    let commands = c.open_link("https://wa.me/15551230002?text=on%20my%20way");
    assert_eq!(frames(&commands), [ClientFrame::messages(BEN, 200)]);
    assert!(!frames(&commands).iter().any(is_a_send), "never a send");
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert_eq!(c.model().composer().text(), "on my way");
    assert!(c.take_draft_changed(), "the composer widget must show it");
    assert!(!c.take_draft_changed());
    // The messages arrive: still nothing is sent.
    let next = c.incoming(reply(
        &ClientFrame::messages(BEN, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: BEN.to_owned(),
            messages: Vec::new(),
        },
    ));
    assert!(!frames(&next).iter().any(is_a_send));
    assert_eq!(c.model().composer().text(), "on my way");
}

#[test]
fn a_link_without_text_shows_its_chats_own_draft_and_keeps_the_one_left() {
    let mut c = opened(vec![message("M1", "text")]);
    c.set_draft("typed");
    assert!(c.reply(0));
    c.open_link("whatsapp://send?phone=15551230002");
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert_eq!(c.model().composer().text(), "", "Ana's draft stays Ana's");
    assert_eq!(c.model().composer().reply_to(), None);
    assert!(c.take_draft_changed(), "the composer widget shows Ben's");
    c.open_chat(0);
    assert_eq!(c.model().composer().text(), "typed");
    assert_eq!(c.model().composer().reply_to(), Some("M1"));
    assert!(c.take_draft_changed());
}

#[test]
fn a_link_to_a_number_with_no_chat_opens_an_empty_conversation_for_it() {
    let mut c = connected(0);
    let commands = c.open_link("whatsapp://send?phone=%2B1%20555%20123%200003&text=hola");
    assert_eq!(frames(&commands), [ClientFrame::messages(CAL, 200)]);
    assert_eq!(c.model().selected_jid(), Some(CAL));
    assert_eq!(c.model().composer().text(), "hola");
    // Only the user's own send sends it, once.
    let (sent_it, commands) = c.send();
    assert!(sent_it);
    assert_eq!(
        commands,
        [Command::Send(ClientFrame::send(CAL, "hola", "d-1"))]
    );
}

#[test]
fn a_link_from_a_cold_start_waits_for_the_chat_list() {
    let mut c = Controller::new("d");
    assert!(
        c.open_link("https://api.whatsapp.com/send?phone=15551230002&text=hi")
            .is_empty()
    );
    c.incoming(Incoming::Connected);
    let loaded = c.incoming(reply(
        &ClientFrame::chats(200),
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana", 0), chat(BEN, "Ben", 0)],
        },
    ));
    assert_eq!(loaded, [Command::Send(ClientFrame::messages(BEN, 200))]);
    assert_eq!(c.model().composer().text(), "hi");
    assert!(c.take_draft_changed());
}

#[test]
fn a_group_invite_link_says_it_is_not_supported_yet() {
    let mut c = connected(0);
    assert!(
        c.open_link("https://chat.whatsapp.com/AbCdEf123")
            .is_empty()
    );
    assert_eq!(
        c.take_notice().as_deref(),
        Some("group invite links are not supported yet")
    );
    assert_eq!(c.model().selected_jid(), Some(ANA), "nothing opened");
    assert!(!c.model().is_open());
}

#[test]
fn an_invalid_link_says_why() {
    let mut c = connected(0);
    assert!(c.open_link("whatsapp://send?phone=12").is_empty());
    assert_eq!(
        c.take_notice().as_deref(),
        Some("cannot open the link: the link's phone number is not 7 to 15 digits")
    );
    assert!(c.open_link("https://example.com/").is_empty());
    assert_eq!(
        c.take_notice().as_deref(),
        Some("cannot open the link: not a WhatsApp chat link")
    );
}

#[test]
fn a_typed_number_stands_for_its_link_by_the_link_rules() {
    for (written, digits) in [
        ("15551230003", "15551230003"),
        ("+15551230003", "15551230003"),
        ("+1 555 123 0003", "15551230003"),
        ("1-555-123-0003", "15551230003"),
        ("  +1 (555) 123.0003  ", "15551230003"),
        ("1555123", "1555123"),
        ("155512300031234", "155512300031234"),
    ] {
        assert_eq!(
            number_link(written).as_deref(),
            Ok(format!("whatsapp://send?phone={digits}").as_str()),
            "{written:?}"
        );
    }
    for written in [
        "",
        "   ",
        "+",
        "155512",
        "1555123000312345",
        "1555abc0003",
        "+1 555 CALL ME",
        "15551230003&text=hi",
        "15551230003#",
        "1555%31230003",
        "١٥٥٥١٢٣٠٠٠٣",
    ] {
        assert_eq!(number_link(written), Err(BAD_NUMBER), "{written:?}");
    }
    assert_eq!(BAD_NUMBER, "the phone number is not 7 to 15 digits");
}

#[test]
fn a_new_message_opens_the_chat_as_its_link_with_no_text_does_and_sends_nothing() {
    for (written, link, jid) in [
        ("+1 555 123 0003", "whatsapp://send?phone=15551230003", CAL),
        ("1-555-123-0002", "whatsapp://send?phone=15551230002", BEN),
    ] {
        let mut typed = connected(0);
        let mut linked = connected(0);
        let commands = typed.new_message(written).expect("a valid number");
        assert_eq!(commands, linked.open_link(link), "{written:?}");
        assert_eq!(frames(&commands), [ClientFrame::messages(jid, 200)]);
        assert!(!frames(&commands).iter().any(is_a_send), "never a send");
        assert_eq!(typed.model().selected_jid(), Some(jid));
        assert!(typed.model().is_open());
        assert_eq!(typed.model().composer().text(), "", "the composer is empty");
        assert!(!typed.take_draft_changed(), "no draft was put there");
        assert_eq!(typed.take_notice(), None);
        // The messages arrive: still nothing is sent.
        let next = typed.incoming(reply(
            &ClientFrame::messages(jid, 200),
            DaemonFrame::Messages {
                v: PROTOCOL_VERSION,
                jid: jid.to_owned(),
                messages: Vec::new(),
            },
        ));
        assert!(!frames(&next).iter().any(is_a_send));
    }
}

#[test]
fn an_invalid_number_opens_nothing_and_says_why_without_a_notice() {
    let mut c = connected(0);
    for written in ["12", "1555abc0003", ""] {
        assert_eq!(c.new_message(written), Err(BAD_NUMBER), "{written:?}");
    }
    assert_eq!(c.model().selected_jid(), Some(ANA), "nothing opened");
    assert!(!c.model().is_open());
    assert_eq!(c.take_notice(), None, "the dialog says it, not a toast");
}

#[test]
fn a_new_message_opens_with_an_empty_composer_and_keeps_the_other_chats_draft() {
    let mut c = opened(vec![message("M1", "text")]);
    c.set_draft("meant for ana");
    assert_eq!(c.new_message("12"), Err(BAD_NUMBER));
    assert_eq!(
        c.model().composer().text(),
        "meant for ana",
        "an invalid number clears nothing"
    );
    let commands = c.new_message("+1 555 123 0003").expect("a valid number");
    assert!(!frames(&commands).iter().any(is_a_send));
    assert_eq!(c.model().selected_jid(), Some(CAL));
    assert_eq!(
        c.model().composer().text(),
        "",
        "never another chat's draft"
    );
    assert!(c.take_draft_changed(), "the composer widget must empty too");
    let (sent_it, commands) = c.send();
    assert!(!sent_it, "a second Enter sends nothing");
    assert!(commands.is_empty());
    // Ana's draft was never lost: it is back when her chat is.
    c.open_chat(0);
    assert_eq!(c.model().selected_jid(), Some(ANA));
    assert_eq!(c.model().composer().text(), "meant for ana");
    assert!(c.take_draft_changed());
}

#[test]
fn a_new_message_to_a_chat_with_a_saved_draft_shows_that_draft() {
    let mut c = connected(0);
    c.open_chat(1);
    c.set_draft("for ben");
    c.open_chat(0);
    c.set_draft("for ana");
    c.new_message("1-555-123-0002").expect("Ben's number");
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert_eq!(c.model().composer().text(), "for ben", "never destroyed");
    let (sent_it, commands) = c.send();
    assert!(sent_it);
    assert_eq!(
        commands,
        [Command::Send(ClientFrame::send(BEN, "for ben", "d-1"))]
    );
}

#[test]
fn switching_chats_shows_each_chats_own_draft_in_the_widget() {
    let mut c = connected(0);
    c.open_chat(0);
    c.set_draft("for ana");
    assert!(!c.take_draft_changed(), "a typed draft is already shown");
    c.open_chat(1);
    assert!(c.take_draft_changed());
    assert_eq!(c.model().composer().text(), "");
    c.set_draft("for ben");
    c.open_chat(0);
    assert!(c.take_draft_changed());
    assert_eq!(c.model().composer().text(), "for ana");
    assert!(!c.take_draft_changed(), "asked once");
}
