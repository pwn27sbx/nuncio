use super::*;
use nuncio_client::wire::{PROTOCOL_VERSION, Reaction, SendState};
use nuncio_client::{ClientFrame, DaemonFrame, Session, Snapshot};

const ANA: &str = "15551230001@s.whatsapp.net";
const BEN: &str = "15551230002@s.whatsapp.net";

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

fn message(id: &str, kind: &str, text: &str, from_me: bool) -> Message {
    Message {
        id: id.to_owned(),
        kind: kind.to_owned(),
        text: text.to_owned(),
        from_me,
        timestamp: "2026-10-20T10:05:00Z".to_owned(),
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

fn model(chats: Vec<Chat>) -> Model {
    Model::new(
        Snapshot {
            daemon: "test".to_owned(),
            linked: true,
            session: Session::Linked,
            unread: 0,
            chats,
        },
        "d",
    )
}

/// A model with Ana's chat open and `messages` (oldest first) loaded.
fn open_with(messages: Vec<Message>) -> Model {
    let mut m = model(vec![chat(ANA, "Ana")]);
    let request = m.open_selected().expect("opens");
    let newest_first = messages.into_iter().rev().collect();
    m.apply_reply(
        &request,
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: newest_first,
        },
    );
    m
}

fn type_draft(m: &mut Model, text: &str) {
    text.chars().for_each(|c| m.composer_mut().insert(c));
}

#[test]
fn a_chat_row_has_its_title_badge_markers_and_last_message() {
    let mut ana = chat(ANA, "");
    ana.phone = Some("…0001".to_owned());
    ana.unread = 3;
    ana.pinned = true;
    ana.muted = true;
    ana.last_message = Some(message("M1", "text", "see you\nat ten", false));
    let mut ben = chat(BEN, "Ben");
    ben.last_message = Some(message("M2", "image", "", true));
    let rows = chat_rows(
        &model(vec![ana, ben, chat("120363000000000001@g.us", "")]),
        60,
    );

    assert_eq!(rows[0].title, "…0001", "chat_title: the masked phone");
    assert_eq!(rows[0].unread.as_deref(), Some("3"));
    assert!(rows[0].pinned && rows[0].muted && !rows[0].archived);
    assert_eq!(rows[0].preview, "see you", "the first line only");
    assert_eq!(rows[0].time, "11:05", "local time");
    assert_eq!(
        rows[0].label,
        "…0001, 3 unread, pinned, muted. Last message at 11:05: see you"
    );
    assert_eq!(rows[1].unread, None, "no badge without unread");
    assert_eq!(rows[1].preview, "[image]");
    assert_eq!(rows[2].title, "unnamed group");
    assert_eq!(rows[2].preview, "(no messages)");
    assert_eq!(rows[2].label, "unnamed group. No messages");
}

#[test]
fn archived_chats_are_listed_last() {
    let mut old = chat(ANA, "Ana");
    old.archived = true;
    let rows = chat_rows(&model(vec![old, chat(BEN, "Ben")]), 0);
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["Ben", "Ana"]);
    assert!(rows[1].archived);
    assert!(rows[1].label.contains(", archived"), "{}", rows[1].label);
}

#[test]
fn bubbles_are_mine_or_theirs_with_the_sender_and_time() {
    let m = open_with(vec![
        message("M1", "text", "hi", false),
        message("M2", "text", "hello", true),
    ]);
    let shown = bubbles(&m, -180);
    assert!(!shown[0].mine);
    assert_eq!(shown[0].sender, "Ana");
    assert_eq!(shown[0].time, "07:05");
    assert_eq!(shown[0].mark, None, "a received message has no send mark");
    assert!(shown[1].mine);
    assert_eq!(shown[1].sender, "You");
    assert_eq!(shown[1].mark, Some(SendMark::Sent));
    assert_eq!(shown[1].label, "You, 07:05, sent: hello");
}

#[test]
fn a_send_is_marked_sending_then_sent_queued_not_sent_or_not_confirmed() {
    let mut m = open_with(Vec::new());
    let marks =
        |m: &Model| -> Vec<Option<SendMark>> { bubbles(m, 0).iter().map(|b| b.mark).collect() };
    for text in ["one", "two", "three", "four"] {
        type_draft(&mut m, text);
        m.send().expect("sends");
    }
    assert_eq!(marks(&m), [Some(SendMark::Sending); 4]);
    m.apply(&DaemonFrame::Sent {
        v: PROTOCOL_VERSION,
        client_id: "d-1".to_owned(),
        state: SendState::Sent,
        id: Some("3EB0".to_owned()),
        timestamp: Some("2026-10-20T10:06:00Z".to_owned()),
    });
    m.apply(&DaemonFrame::Sent {
        v: PROTOCOL_VERSION,
        client_id: "d-2".to_owned(),
        state: SendState::Queued,
        id: None,
        timestamp: None,
    });
    m.apply(&DaemonFrame::SendFailed {
        v: PROTOCOL_VERSION,
        client_id: "d-3".to_owned(),
        message: "no".to_owned(),
    });
    m.send_not_confirmed("d-4");
    assert_eq!(
        marks(&m),
        [
            Some(SendMark::Sent),
            Some(SendMark::Queued),
            Some(SendMark::NotSent),
            Some(SendMark::NotConfirmed)
        ]
    );
    let shown = bubbles(&m, 0);
    assert!(
        shown[2].label.contains(", not sent: three"),
        "{}",
        shown[2].label
    );
    assert!(SendMark::NotSent.is_failure() && SendMark::NotConfirmed.is_failure());
    assert!(!SendMark::Queued.is_failure());
}

#[test]
fn what_a_bubble_offers_follows_what_whatsapp_has() {
    let mut m = open_with(vec![
        message("M1", "text", "hi", false),
        message("M2", "document", "report.pdf", false),
    ]);
    type_draft(&mut m, "fail");
    m.send().expect("sends");
    let waiting = bubbles(&m, 0);
    assert!(waiting[0].can_target && !waiting[0].can_open && !waiting[0].can_retry);
    assert!(waiting[1].can_open, "media with an id opens");
    assert!(
        !waiting[2].can_target && !waiting[2].can_retry,
        "still sending"
    );
    m.apply(&DaemonFrame::SendFailed {
        v: PROTOCOL_VERSION,
        client_id: "d-1".to_owned(),
        message: "no".to_owned(),
    });
    let failed = &bubbles(&m, 0)[2];
    assert!(failed.can_retry && !failed.can_target);
    // The retry the bubble offers is the model's; while it waits, none is offered.
    assert!(m.select_message(2));
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("d-1")));
    let retrying = &bubbles(&m, 0)[2];
    assert_eq!(retrying.mark, Some(SendMark::Sending));
    assert!(!retrying.can_retry, "one attempt at a time");
}

#[test]
fn a_reply_shows_the_quoted_line_and_one_not_loaded_says_so() {
    let mut quoting = message("M2", "text", "yes", true);
    quoting.reply_to = Some("M1".to_owned());
    let mut lost = message("M3", "text", "and?", false);
    lost.reply_to = Some("OLD".to_owned());
    let m = open_with(vec![message("M1", "text", "coffee?", false), quoting, lost]);
    let shown = bubbles(&m, 0);
    assert_eq!(shown[0].quote, None);
    assert_eq!(shown[1].quote.as_deref(), Some("coffee?"));
    assert_eq!(
        shown[1].label,
        "You, 10:05, sent, replying to: coffee?: yes"
    );
    assert_eq!(shown[2].quote.as_deref(), Some("(a message not loaded)"));
}

#[test]
fn media_shows_its_label_and_reactions_are_listed_mine_marked() {
    let mut sticker = message("M1", "sticker", "", false);
    sticker.animated = true;
    sticker.reactions = vec![
        Reaction {
            emoji: "👍".to_owned(),
            from_me: true,
        },
        Reaction {
            emoji: "😂".to_owned(),
            from_me: false,
        },
    ];
    let m = open_with(vec![sticker, message("M2", "image", "the beach", false)]);
    let shown = bubbles(&m, 0);
    assert_eq!(shown[0].media.as_deref(), Some("[sticker] ▶"));
    assert_eq!(
        shown[0].reactions,
        [
            ReactionView {
                emoji: "👍".to_owned(),
                mine: true
            },
            ReactionView {
                emoji: "😂".to_owned(),
                mine: false
            }
        ]
    );
    assert_eq!(shown[0].label, "Ana, 10:05: Sticker. Reactions: 👍 😂");
    assert_eq!(shown[1].media.as_deref(), Some("[image]"));
    assert_eq!(shown[1].text, "the beach");
    assert_eq!(shown[1].label, "Ana, 10:05: Photo, the beach");
}

#[test]
fn a_media_file_being_fetched_to_open_says_downloading() {
    let mut m = open_with(vec![message("M1", "document", "", false)]);
    assert!(m.select_message(0));
    m.download_selected(false, true).expect("asks for the file");
    let shown = bubbles(&m, 0);
    assert_eq!(shown[0].media.as_deref(), Some("[document] downloading…"));
    assert!(shown[0].selected);
}

#[test]
fn the_headers_say_reconnecting_while_the_connection_is_lost() {
    let mut m = model(vec![chat(ANA, "Ana")]);
    assert_eq!(
        conversation_header(&m),
        Header {
            title: "Ana".to_owned(),
            subtitle: "linked".to_owned()
        }
    );
    m.set_connected(false);
    assert_eq!(conversation_header(&m).subtitle, "reconnecting…");
    assert_eq!(sidebar_header(&m).subtitle, "reconnecting…");
    m.set_connected(true);
    m.apply(&DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 4,
        jid: ANA.to_owned(),
        chat_unread: 4,
    });
    assert_eq!(sidebar_header(&m).subtitle, "linked · 4 unread");
    assert_eq!(conversation_header(&model(Vec::new())).title, "No chat");
}

#[test]
fn the_composer_shows_the_reply_banner_until_it_is_cancelled() {
    let mut m = open_with(vec![message("M1", "text", "coffee?", false)]);
    assert_eq!(composer_view(&m).replying, None);
    assert!(m.select_message(0));
    assert!(m.reply_to_selected());
    assert_eq!(
        composer_view(&m).replying.as_deref(),
        Some("Replying to: coffee?")
    );
    m.composer_mut().cancel_reply();
    assert_eq!(composer_view(&m).replying, None);
}

#[test]
fn a_vanished_open_chat_blocks_the_send_with_its_notice() {
    let mut m = open_with(Vec::new());
    type_draft(&mut m, "for Ana");
    // The daemon merged Ana's number chat away: the list no longer has it.
    m.apply(&DaemonFrame::Chats {
        v: PROTOCOL_VERSION,
        chats: vec![chat(BEN, "Ben")],
    });
    assert_eq!(composer_view(&m).blocked.as_deref(), Some(MERGED_NOTICE));
    assert_eq!(m.send(), None, "the draft never changes recipient");
    assert_eq!(m.composer().text(), "for Ana");
    m.select_chat(0);
    assert_eq!(composer_view(&m).blocked, None);
}

#[test]
fn the_search_lists_its_results_with_their_time() {
    let mut m = open_with(Vec::new());
    assert_eq!(search_view(&m, 0), None);
    assert!(m.open_search());
    "late".chars().for_each(|c| m.search_type(c));
    let request = m.submit_search().expect("searches");
    assert_eq!(
        search_view(&m, 0),
        Some(SearchView {
            awaiting: true,
            results: None,
            selected: None
        })
    );
    m.apply_reply(
        &request,
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("M9", "text", "running late", false)],
        },
    );
    assert_eq!(
        search_view(&m, 120),
        Some(SearchView {
            awaiting: false,
            results: Some(vec!["12:05  running late".to_owned()]),
            selected: Some(0)
        })
    );
}

#[test]
fn the_title_bar_is_hidden_on_hyprland_unless_chosen() {
    let signature = Some(OsStr::new("abc_123"));
    assert!(shows_title_bar(TitleBar::Auto, None, Some("GNOME")));
    assert!(!shows_title_bar(TitleBar::Auto, signature, None));
    assert!(!shows_title_bar(TitleBar::Auto, None, Some("Hyprland")));
    assert!(shows_title_bar(
        TitleBar::Shown,
        signature,
        Some("Hyprland")
    ));
    assert!(!shows_title_bar(TitleBar::Hidden, None, Some("GNOME")));
}

#[test]
fn a_time_with_no_timestamp_shows_dashes_and_wraps_past_midnight() {
    let mut late = message("M1", "text", "", false);
    late.timestamp = "2026-10-20T23:30:00Z".to_owned();
    assert_eq!(local_hhmm(&late, 60), "00:30");
    late.timestamp = String::new();
    assert_eq!(local_hhmm(&late, 60), "--:--");
}
