use super::*;
use crate::worker::Incoming;
use nuncio_client::wire::{MessageState, PROTOCOL_VERSION};
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message};

const ANA: &str = "15551230001@s.whatsapp.net";
const BEN: &str = "15551230002@s.whatsapp.net";
const NO_NAME: &str = "15551239876@s.whatsapp.net";

fn chat(jid: &str, name: &str) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: name.to_owned(),
        unread: 0,
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

/// A send of mine nunciod lists as not sent: under the WhatsApp id it stored it with.
fn not_sent(client_id: &str) -> Message {
    Message {
        id: format!("3EB0{client_id}"),
        from_me: true,
        state: Some(MessageState::NotSent),
        client_id: Some(client_id.to_owned()),
        ..message("", "text")
    }
}

fn reply(request: &ClientFrame, frame: DaemonFrame) -> Incoming {
    Incoming::Reply {
        request: request.clone(),
        result: Ok(frame),
    }
}

/// Connected, with Ana, Ben and a chat with no name listed.
fn listed() -> Controller {
    let mut c = Controller::new("p");
    c.incoming(Incoming::Connected);
    c.incoming(reply(
        &ClientFrame::chats(200),
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana"), chat(BEN, "Ben"), chat(NO_NAME, "")],
        },
    ));
    c
}

/// Ana's chat open with `messages` (newest first) loaded.
fn opened(messages: Vec<Message>) -> Controller {
    let mut c = listed();
    c.open_chat(0);
    c.incoming(reply(
        &ClientFrame::messages(ANA, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages,
        },
    ));
    c
}

fn titles(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.title.as_str()).collect()
}

#[test]
fn a_query_matches_its_characters_in_order_ignoring_case_and_spaces() {
    assert!(score("set", "Settings").is_some());
    assert!(score("SET", "settings").is_some());
    assert!(score("chat ana", "Chat: Ana").is_some());
    assert!(score("ñan", "Chat: Ñandú").is_some(), "not only ASCII");
    assert!(score("stngs", "Settings").is_some(), "a subsequence");
    assert_eq!(score("ste", "Set"), None, "the order counts");
    assert_eq!(score("x", "Settings"), None);
    assert_eq!(score("settingss", "Settings"), None, "every character once");
    assert_eq!(score("", "anything"), Some(0));
    assert_eq!(score("   ", "anything"), Some(0));
}

#[test]
fn a_word_start_and_a_run_of_characters_rank_first() {
    let entries = vec![
        Entry::new("Chat: Banana", None, Action::Search),
        Entry::new("Chat: Ana", None, Action::Quit),
        Entry::new("Chat: Diana", None, Action::Settings),
    ];
    assert_eq!(filter(&entries, "ana"), [1, 0, 2]);
    assert!(score("q", "Quit") > score("q", "Square"));
    assert!(score("se", "Search in this chat") > score("se", "Chat: Rose"));
}

#[test]
fn an_empty_query_keeps_every_entry_in_order_and_no_match_keeps_none() {
    let c = listed();
    let entries = entries(c.model(), None);
    assert_eq!(filter(&entries, ""), (0..entries.len()).collect::<Vec<_>>());
    assert!(filter(&entries, "zzzz").is_empty());
}

#[test]
fn with_no_chat_open_it_lists_settings_quit_and_every_chat_by_its_title() {
    let c = listed();
    let entries = entries(c.model(), Some(0));
    assert_eq!(
        titles(&entries),
        [
            "Settings",
            "Quit",
            "Chat: Ana",
            "Chat: Ben",
            // The list's own title for a chat with no name: its masked number.
            &format!("Chat: {}", nuncio_client::chat_title(&chat(NO_NAME, "")))
        ]
    );
    assert_eq!(entries[2].action, Action::OpenChat(ANA.to_owned()));
    assert_eq!(entries[0].shortcut, Some("Ctrl+,"));
    assert_eq!(entries[1].label(), "Quit, shortcut Ctrl+Q");
    assert_eq!(entries[2].label(), "Chat: Ana");
}

#[test]
fn an_open_chat_adds_its_search_and_the_selected_message_its_actions() {
    let c = opened(vec![message("M2", "image"), message("M1", "text")]);
    // Oldest first: 0 is the text, 1 the picture.
    let text = entries(c.model(), Some(0));
    assert_eq!(
        titles(&text)[..4],
        [
            "Reply to the selected message",
            "React to the selected message…",
            "Search in this chat",
            "Settings",
        ]
    );
    assert!(matches!(&text[0].action, Action::Reply(t) if t.id == "M1" && t.jid == ANA));
    assert!(matches!(&text[1].action, Action::React(t) if t.id == "M1"));
    let picture = entries(c.model(), Some(1));
    assert!(
        picture.iter().any(|e| e.title == "Open the selected media"
            && e.shortcut == Some("o")
            && matches!(&e.action, Action::OpenMedia(t) if t.id == "M2")),
        "{:?}",
        titles(&picture)
    );
    assert!(!titles(&text).contains(&"Open the selected media"));
    assert!(
        !titles(&text).contains(&"Retry the selected message"),
        "a received message is not retried"
    );
    let none = entries(c.model(), None);
    assert_eq!(titles(&none)[0], "Search in this chat");
    let past_the_end = entries(c.model(), Some(9));
    assert_eq!(titles(&past_the_end)[0], "Search in this chat");
}

#[test]
fn a_send_that_ended_not_sent_offers_retry_and_nothing_that_needs_an_id() {
    let c = opened(vec![not_sent("p-7")]);
    let entries = entries(c.model(), Some(0));
    assert!(
        matches!(&entries[0].action, Action::Retry(t) if t.client_id.as_deref() == Some("p-7"))
    );
    assert_eq!(entries[0].shortcut, Some("Shift+R"));
    for missing in [
        "Reply to the selected message",
        "React to the selected message…",
    ] {
        assert!(!titles(&entries).contains(&missing), "{missing}");
    }
}

#[test]
fn message_actions_need_the_chat_open() {
    let mut c = opened(vec![message("M1", "image")]);
    c.back_to_list();
    let entries = entries(c.model(), Some(0));
    assert!(
        entries
            .iter()
            .all(|e| !matches!(e.action, Action::Reply(_) | Action::OpenMedia(_))),
        "{:?}",
        titles(&entries)
    );
}

#[test]
fn no_entry_sends_a_message_whatever_is_chosen() {
    let base = opened(vec![
        message("M2", "image"),
        message("M1", "text"),
        not_sent("p-7"),
    ]);
    let mut every = Vec::new();
    for message in [None, Some(0), Some(1), Some(2)] {
        every.extend(entries(base.model(), message));
    }
    assert!(every.iter().all(|e| !e.title.starts_with("Send")));
    for entry in every {
        let mut c = base.clone();
        c.set_draft("a draft nobody sent");
        let commands = match run(&mut c, &entry.action) {
            Step::Opened(commands) | Step::Commands(commands) => commands,
            _ => Vec::new(),
        };
        assert!(
            commands
                .iter()
                .all(|cmd| !matches!(cmd, Command::Send(ClientFrame::Send { .. }))),
            "{} sent: {commands:?}",
            entry.title
        );
        assert_eq!(
            c.model().composer().text(),
            "a draft nobody sent",
            "{} kept the draft",
            entry.title
        );
    }
}

#[test]
fn choosing_a_chat_opens_it_as_a_click_does() {
    let mut c = listed();
    let entries = entries(c.model(), None);
    let ben = filter(&entries, "ben")[0];
    assert_eq!(entries[ben].action, Action::OpenChat(BEN.to_owned()));
    assert_eq!(
        run(&mut c, &entries[ben].action),
        Step::Opened(vec![Command::Send(ClientFrame::messages(BEN, 200))])
    );
    assert_eq!(c.model().selected_jid(), Some(BEN));
    assert!(c.model().is_open());
}

#[test]
fn choosing_retry_or_open_asks_the_daemon_and_the_rest_go_to_the_window() {
    let mut c = opened(vec![message("M2", "image"), not_sent("p-7")]);
    // Oldest first: 0 is the send that ended not sent, 1 the picture.
    let action = |c: &Controller, index: usize, title: &str| {
        entries(c.model(), Some(index))
            .into_iter()
            .find(|e| e.title == title)
            .map(|e| e.action)
            .unwrap_or_else(|| panic!("{title} for {index}"))
    };
    let retry = action(&c, 0, "Retry the selected message");
    let open = action(&c, 1, "Open the selected media");
    let reply_to = action(&c, 1, "Reply to the selected message");
    let react = action(&c, 1, "React to the selected message…");
    assert_eq!(
        run(&mut c, &retry),
        Step::Commands(vec![Command::Send(ClientFrame::retry("p-7"))])
    );
    assert!(matches!(
        run(&mut c, &open),
        Step::Commands(commands) if matches!(commands.as_slice(), [Command::Send(ClientFrame::DownloadMedia { .. })])
    ));
    assert_eq!(run(&mut c, &reply_to), Step::Reply(1));
    assert_eq!(run(&mut c, &react), Step::React(1));
    assert_eq!(run(&mut c, &Action::Search), Step::Search);
    assert_eq!(run(&mut c, &Action::Settings), Step::Settings);
    assert_eq!(run(&mut c, &Action::Quit), Step::Quit);
}

/// Ben's chat opened on `c` with `messages` (newest first) loaded.
fn open_ben(c: &mut Controller, messages: Vec<Message>) {
    c.open_chat(1);
    c.incoming(reply(
        &ClientFrame::messages(BEN, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: BEN.to_owned(),
            messages,
        },
    ));
}

fn commands_of(step: Step) -> Vec<Command> {
    match step {
        Step::Opened(commands) | Step::Commands(commands) => commands,
        _ => Vec::new(),
    }
}

#[test]
fn a_message_command_chosen_after_another_chat_opened_does_nothing_there() {
    // Ana: the third message is my send p-7 that ended not sent.
    let mut c = opened(vec![
        not_sent("p-7"),
        message("A2", "text"),
        message("A1", "text"),
    ]);
    let retry = entries(c.model(), Some(2))
        .into_iter()
        .find(|e| e.title == "Retry the selected message")
        .expect("Ana's send can be retried");
    let reply_entry = entries(c.model(), Some(1))
        .into_iter()
        .find(|e| e.title == "Reply to the selected message")
        .expect("A2 can be replied to");
    // A notification click or a link opens Ben while the palette is open; his third message is
    // another send that ended not sent.
    open_ben(
        &mut c,
        vec![
            not_sent("p-9"),
            message("B2", "image"),
            message("B1", "text"),
        ],
    );
    let step = run(&mut c, &retry.action);
    assert!(
        commands_of(step.clone()).is_empty(),
        "Ben's p-9 must not be retried: {step:?}"
    );
    assert_eq!(step, Step::Gone);
    assert_eq!(run(&mut c, &reply_entry.action), Step::Gone);
    // Even a message with the same id in the other chat is not the one chosen.
    let mut c = opened(vec![message("S1", "image")]);
    let open = entries(c.model(), Some(0))
        .into_iter()
        .find(|e| e.title == "Open the selected media")
        .expect("S1 is a picture");
    open_ben(&mut c, vec![message("S1", "image")]);
    assert_eq!(run(&mut c, &open.action), Step::Gone);
    assert!(c.model().composer().reply_to().is_none());
}

#[test]
fn a_message_command_follows_its_message_when_the_conversation_grows() {
    let mut c = opened(vec![message("A2", "image"), message("A1", "text")]);
    let open = entries(c.model(), Some(1))
        .into_iter()
        .find(|e| e.title == "Open the selected media")
        .expect("A2 is a picture");
    // A reload with one older message moves it one place later (the conversation is oldest
    // first).
    c.incoming(reply(
        &ClientFrame::messages(ANA, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![
                message("A2", "image"),
                message("A1", "text"),
                message("A0", "text"),
            ],
        },
    ));
    let commands = commands_of(run(&mut c, &open.action));
    assert_eq!(
        commands,
        [Command::Send(ClientFrame::download_media(ANA, "A2"))],
        "the picture chosen, wherever it is now"
    );
}
