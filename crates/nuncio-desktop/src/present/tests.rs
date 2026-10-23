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
        sender: None,
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
            stop_reason: None,
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
        None,
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
    let rows = chat_rows(&model(vec![old, chat(BEN, "Ben")]), 0, None);
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
    let shown = bubbles(&m, -180, None);
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
    let marks = |m: &Model| -> Vec<Option<SendMark>> {
        bubbles(m, 0, None).iter().map(|b| b.mark).collect()
    };
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
    let shown = bubbles(&m, 0, None);
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
    let waiting = bubbles(&m, 0, None);
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
    // nunciod publishes the send it refused under the WhatsApp id it stored it with.
    let mut refused = message("3EB0D1", "text", "fail", true);
    refused.state = Some(MessageState::NotSent);
    refused.client_id = Some("d-1".to_owned());
    m.apply(&DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: ANA.to_owned(),
        message: refused,
    });
    let failed = &bubbles(&m, 0, None)[2];
    assert!(failed.can_retry && !failed.can_target);
    // The retry the bubble offers is the model's; while it waits, none is offered.
    assert!(m.select_message(2));
    assert_eq!(m.retry_selected(), Some(ClientFrame::retry("d-1")));
    let retrying = &bubbles(&m, 0, None)[2];
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
    let shown = bubbles(&m, 0, None);
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
    let shown = bubbles(&m, 0, None);
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
    let shown = bubbles(&m, 0, None);
    assert_eq!(shown[0].media.as_deref(), Some("[document] downloading…"));
    assert!(shown[0].selected);
}

#[test]
fn the_rail_says_the_connection_in_one_word() {
    let mut m = model(vec![chat(ANA, "Ana")]);
    assert_eq!(
        rail_status(&m),
        RailStatus {
            word: "linked".to_owned(),
            linked: true
        }
    );
    m.set_connected(false);
    assert_eq!(
        rail_status(&m),
        RailStatus {
            word: "reconnecting…".to_owned(),
            linked: false
        }
    );
}

#[test]
fn the_transcript_header_is_only_the_chat_name() {
    let mut m = model(vec![chat(ANA, "Ana")]);
    assert_eq!(conversation_title(&m), "Ana");
    m.set_connected(false);
    assert_eq!(
        conversation_title(&m),
        "Ana",
        "the connection is the rail's"
    );
    assert_eq!(conversation_title(&model(Vec::new())), "No chat");
}

#[test]
fn the_filters_list_all_unread_or_archived_chats() {
    let mut ana = chat(ANA, "Ana");
    ana.unread = 2;
    let mut cara = chat("15551230003@s.whatsapp.net", "Cara");
    cara.archived = true;
    cara.unread = 1;
    let m = model(vec![cara, ana, chat(BEN, "Ben")]);
    let names = |filter| -> Vec<String> {
        listed(&m, filter, None)
            .into_iter()
            .map(|i| chat_title(&m.chat_list().chats()[i]))
            .collect()
    };
    assert_eq!(names(Filter::All), ["Ana", "Ben", "Cara"], "archived last");
    assert_eq!(names(Filter::Unread), ["Ana", "Cara"]);
    assert_eq!(names(Filter::Archived), ["Cara"]);
    let filters: Vec<&str> = Filter::EACH.iter().map(|f| f.name()).collect();
    assert_eq!(filters, ["All", "Unread", "Archived"]);
}

/// Ana, Ben (read now, opened from Unread) and Cara (archived), with Ben's conversation open.
fn ben_open() -> Model {
    let mut cara = chat("15551230003@s.whatsapp.net", "Cara");
    cara.archived = true;
    let mut m = model(vec![chat(ANA, "Ana"), chat(BEN, "Ben"), cara]);
    m.select_chat(1);
    m.open_selected().expect("opens");
    assert_eq!(m.selected_jid(), Some(BEN));
    m
}

#[test]
fn the_open_chat_stays_only_in_the_filter_it_was_opened_from() {
    let m = ben_open();
    assert_eq!(
        listed(&m, Filter::Unread, Some(Filter::Unread)),
        [1],
        "read now, but opened from Unread: its row stays"
    );
    assert_eq!(
        listed(&m, Filter::Archived, Some(Filter::Unread)),
        [2],
        "Archived lists only the archived chat"
    );
    assert!(
        listed(&m, Filter::Unread, None).is_empty(),
        "opened elsewhere"
    );
    assert!(
        listed(&m, Filter::Unread, Some(Filter::Archived)).is_empty(),
        "opened from another filter"
    );
    assert_eq!(listed(&m, Filter::All, Some(Filter::Unread)), [0, 1, 2]);
}

#[test]
fn the_counts_never_include_the_open_chat() {
    let m = ben_open();
    assert_eq!(count(&m, Filter::All), 3);
    assert_eq!(count(&m, Filter::Unread), 0, "nothing is unread");
    assert_eq!(count(&m, Filter::Archived), 1);
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

#[test]
fn a_group_bubble_and_its_label_name_the_sender_and_fall_back_to_the_group() {
    const GROUP: &str = "120363000000000001@g.us";
    let mut m = model(vec![chat(GROUP, "Climbing")]);
    let request = m.open_selected().expect("opens");
    let mut from_ana = message("G1", "text", "rope?", false);
    from_ana.sender = Some("Ana".to_owned());
    m.apply_reply(
        &request,
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: GROUP.to_owned(),
            messages: vec![message("G2", "text", "an old one", false), from_ana],
        },
    );
    let shown = bubbles(&m, 0, None);
    assert_eq!(shown[0].sender, "Ana");
    assert_eq!(shown[0].label, "Ana, 10:05: rope?");
    assert_eq!(
        shown[1].sender, "Climbing",
        "no sender: the group, as before"
    );
    assert_eq!(shown[1].label, "Climbing, 10:05: an old one");
}

/// Q6: the strip under the transcript header says why WhatsApp stopped the session, in fixed
/// text mapped from `stop_reason`; the rail keeps the short word; without one there is no strip.
#[test]
fn the_strip_names_the_stop_reason() {
    use nuncio_client::wire::StopReason;
    let mut m = model(vec![chat(ANA, "Ana")]);
    assert_eq!(stop_strip(&m), None);
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert_eq!(stop_strip(&m), None);
    assert_eq!(rail_status(&m).word, "offline");
    assert!(!rail_status(&m).linked);
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::StreamReplaced),
    });
    assert_eq!(
        stop_strip(&m).as_deref(),
        Some("another connection of this device took over")
    );
    assert_eq!(
        rail_status(&m).word,
        "offline",
        "the rail keeps the short word"
    );
    // Lost daemon connection wins: the stop is the daemon's news, reconnecting is ours.
    m.set_connected(false);
    assert_eq!(stop_strip(&m), None);
    assert_eq!(rail_status(&m).word, "reconnecting…");
}

/// D4: a stored device nunciod keeps closed shows its fixed text in the strip, and a later frame
/// without a stop_reason (the device opened) clears it.
#[test]
fn the_strip_names_a_closed_device_and_clears() {
    use nuncio_client::wire::StopReason;
    let mut m = model(vec![chat(ANA, "Ana")]);
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::KeyringLocked),
    });
    assert_eq!(
        stop_strip(&m).as_deref(),
        Some("keyring locked: unlock it to connect")
    );
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::DeviceUnreadable),
    });
    assert_eq!(
        stop_strip(&m).as_deref(),
        Some("the stored key does not open this device: see SECURITY.md")
    );
    m.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert_eq!(stop_strip(&m), None);
}

/// A text message of Ana's (or mine) at `timestamp`.
fn at(id: &str, timestamp: &str, from_me: bool) -> Message {
    let mut m = message(id, "text", id, from_me);
    m.timestamp = timestamp.to_owned();
    m
}

/// Which bubbles start a group and which carry the meta line.
fn groups(shown: &[Bubble]) -> Vec<(bool, bool)> {
    shown
        .iter()
        .map(|b| (b.starts_group, b.shows_meta))
        .collect()
}

#[test]
fn one_author_within_five_minutes_is_one_group_with_the_meta_under_its_last_bubble() {
    let m = open_with(vec![
        at("A1", "2026-10-20T10:00:00Z", false),
        at("A2", "2026-10-20T10:02:00Z", false),
        // Exactly five minutes after the one before: still the same group.
        at("A3", "2026-10-20T10:07:00Z", false),
    ]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(
        groups(&shown),
        [(true, false), (false, false), (false, true)]
    );
}

#[test]
fn a_new_author_or_a_gap_over_five_minutes_starts_a_new_group() {
    let m = open_with(vec![
        at("A1", "2026-10-20T10:00:00Z", false),
        at("M1", "2026-10-20T10:01:00Z", true),
        at("M2", "2026-10-20T10:02:00Z", true),
        // Five minutes and one second after M2.
        at("M3", "2026-10-20T10:07:01Z", true),
        at("A2", "2026-10-20T10:07:30Z", false),
    ]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(
        groups(&shown),
        [
            (true, true),
            (true, false),
            (false, true),
            (true, true),
            (true, true)
        ]
    );
}

#[test]
fn group_members_are_separate_authors() {
    const GROUP: &str = "120363000000000001@g.us";
    let mut m = model(vec![chat(GROUP, "Climbing")]);
    let request = m.open_selected().expect("opens");
    let mut ana = at("G1", "2026-10-20T10:00:00Z", false);
    ana.sender = Some("Ana".to_owned());
    let mut ben = at("G2", "2026-10-20T10:01:00Z", false);
    ben.sender = Some("Ben".to_owned());
    m.apply_reply(
        &request,
        &DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: GROUP.to_owned(),
            messages: vec![ben, ana],
        },
    );
    assert_eq!(groups(&bubbles(&m, 0, None)), [(true, true), (true, true)]);
}

#[test]
fn a_failed_send_inside_a_group_keeps_its_own_meta_line() {
    let mut failed = at("M2", "2026-10-20T10:01:00Z", true);
    failed.state = Some(MessageState::NotSent);
    failed.client_id = Some("d-1".to_owned());
    let m = open_with(vec![
        at("M1", "2026-10-20T10:00:00Z", true),
        failed,
        at("M3", "2026-10-20T10:02:00Z", true),
    ]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(
        groups(&shown),
        [(true, false), (false, true), (false, true)]
    );
    assert_eq!(meta_line(&shown[1]), "10:01 · not sent");
    assert_eq!(meta_line(&shown[2]), "10:02 · sent");
}

#[test]
fn the_meta_line_is_the_time_and_the_state_of_a_send() {
    let m = open_with(vec![
        at("A1", "2026-10-20T10:00:00Z", false),
        at("M1", "2026-10-20T10:01:00Z", true),
    ]);
    let shown = bubbles(&m, 60, None);
    assert_eq!(
        meta_line(&shown[0]),
        "11:00",
        "a received message: the time"
    );
    assert_eq!(meta_line(&shown[1]), "11:01 · sent");
}

#[test]
fn a_day_label_goes_above_the_first_bubble_of_each_local_day() {
    let m = open_with(vec![
        at("A1", "2026-10-19T09:00:00Z", false),
        at("A2", "2026-10-19T12:00:00Z", false),
        // 22:58 and 23:01 UTC are 23:58 and 00:01 at +60: a new day, so a new group too.
        at("A3", "2026-10-20T22:58:00Z", false),
        at("A4", "2026-10-20T23:01:00Z", false),
    ]);
    let today = Some(Day::from_civil(2026, 10, 21));
    let shown = bubbles(&m, 60, today);
    let days: Vec<Option<&str>> = shown.iter().map(|b| b.day.as_deref()).collect();
    assert_eq!(
        days,
        [Some("Mon 19 Oct"), None, Some("Yesterday"), Some("Today")]
    );
    assert!(shown[3].starts_group, "the day changed within five minutes");
    assert!(shown[2].shows_meta);
}

#[test]
fn a_day_reads_today_yesterday_or_its_date() {
    let day = Day::from_civil(2026, 10, 20);
    assert_eq!(day_label(day, Some(day)), "Today");
    assert_eq!(
        day_label(day, Some(Day::from_civil(2026, 10, 21))),
        "Yesterday"
    );
    assert_eq!(
        day_label(day, Some(Day::from_civil(2026, 11, 2))),
        "Tue 20 Oct"
    );
    assert_eq!(
        day_label(day, Some(Day::from_civil(2027, 1, 2))),
        "Tue 20 Oct 2026"
    );
    assert_eq!(
        day_label(day, None),
        "Tue 20 Oct 2026",
        "no clock: the full date"
    );
    assert_eq!(
        day_label(Day::from_civil(2024, 2, 29), None),
        "Thu 29 Feb 2024",
        "a leap day"
    );
}

#[test]
fn a_message_with_no_timestamp_has_no_day_and_starts_its_own_group() {
    let m = open_with(vec![
        at("A1", "2026-10-20T10:00:00Z", false),
        at("A2", "", false),
    ]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(shown[1].day, None);
    assert_eq!(groups(&shown), [(true, true), (true, true)]);
}

#[test]
fn the_count_chip_shows_only_more_than_one_unread() {
    let mut one = chat(ANA, "Ana");
    one.unread = 1;
    let mut two = chat(BEN, "Ben");
    two.unread = 2;
    let rows = chat_rows(
        &model(vec![one, two, chat("15551230003@s.whatsapp.net", "Cara")]),
        0,
        None,
    );
    assert_eq!(
        rows[0].unread.as_deref(),
        Some("1"),
        "unread: the marker and name tone"
    );
    assert_eq!(rows[0].chip, None, "no chip for a single unread message");
    assert_eq!(rows[1].chip.as_deref(), Some("2"));
    assert_eq!(rows[2].chip, None);
}

#[test]
fn a_row_shows_today_as_a_time_and_yesterday_as_a_word() {
    let mut ana = chat(ANA, "Ana");
    ana.last_message = Some(at("A1", "2026-10-20T10:05:00Z", false));
    let rows = |today| chat_rows(&model(vec![ana.clone()]), 0, today);
    assert_eq!(rows(Some(Day::from_civil(2026, 10, 20)))[0].when, "10:05");
    assert_eq!(
        rows(Some(Day::from_civil(2026, 10, 21)))[0].when,
        "Yesterday"
    );
    assert_eq!(rows(Some(Day::from_civil(2026, 10, 25)))[0].when, "20 Oct");
    assert_eq!(rows(None)[0].when, "10:05", "no clock: the time");
    assert_eq!(
        rows(Some(Day::from_civil(2026, 10, 21)))[0].label,
        "Ana. Last message at 10:05: A1",
        "the label is unchanged"
    );
}

#[test]
fn a_group_row_has_its_glyph_and_the_author_before_the_preview_unless_muted() {
    const GROUP: &str = "120363000000000001@g.us";
    let mut climbing = chat(GROUP, "Climbing");
    let mut from_ana = message("G1", "text", "rope?", false);
    from_ana.sender = Some("Ana".to_owned());
    climbing.last_message = Some(from_ana);
    let mut mine = chat("120363000000000002@g.us", "Runs");
    mine.last_message = Some(message("G2", "text", "at six", true));
    let mut muted = chat("120363000000000003@g.us", "Noise");
    muted.muted = true;
    let mut from_ben = message("G3", "text", "spam", false);
    from_ben.sender = Some("Ben".to_owned());
    muted.last_message = Some(from_ben);
    let mut ana = chat(ANA, "Ana");
    ana.last_message = Some(message("A1", "text", "hi", false));
    let rows = chat_rows(&model(vec![climbing, mine, muted, ana]), 0, None);
    let by = |title: &str| rows.iter().find(|r| r.title == title).expect(title);
    assert!(by("Climbing").group && by("Runs").group && !by("Ana").group);
    assert_eq!(by("Climbing").author.as_deref(), Some("Ana"));
    assert_eq!(by("Runs").author.as_deref(), Some("You"));
    assert_eq!(by("Noise").author, None, "a muted group: no author");
    assert_eq!(by("Ana").author, None, "a direct chat: no author");
    assert_eq!(
        by("Climbing").label,
        "Climbing. Last message at 10:05: Ana: rope?"
    );
}

#[test]
fn ticks_go_before_the_preview_of_my_last_message_only() {
    let mut ana = chat(ANA, "Ana");
    ana.last_message = Some(message("M1", "text", "hello", true));
    let mut ben = chat(BEN, "Ben");
    ben.last_message = Some(message("M2", "text", "hi", false));
    let mut cara = chat("15551230003@s.whatsapp.net", "Cara");
    let mut queued = message("", "text", "later", true);
    queued.state = Some(MessageState::Queued);
    cara.last_message = Some(queued);
    let rows = chat_rows(&model(vec![ana, ben, cara]), 0, None);
    let by = |title: &str| rows.iter().find(|r| r.title == title).expect(title);
    assert_eq!(by("Ana").ticks, Some("✓"));
    assert_eq!(by("Ben").ticks, None);
    assert_eq!(by("Cara").ticks, Some("◷"));
}

#[test]
fn the_transcript_says_why_it_is_empty() {
    assert_eq!(
        transcript_empty(&model(vec![])),
        Some(EmptyState {
            title: "No messages yet",
            hint: "Pick a chat to read it here"
        })
    );
    let mut opening = model(vec![chat(ANA, "Ana")]);
    opening.open_selected().expect("opens");
    assert_eq!(
        transcript_empty(&opening),
        None,
        "loading: nothing to say yet"
    );
    assert_eq!(
        transcript_empty(&open_with(vec![])),
        Some(EmptyState {
            title: "No messages yet",
            hint: "This chat has no messages"
        })
    );
    assert_eq!(
        transcript_empty(&open_with(vec![message("M1", "text", "hi", false)])),
        None
    );
}

#[test]
fn the_list_says_why_it_is_empty_per_filter() {
    assert_eq!(list_empty(Filter::All).title, "No chats");
    assert_eq!(list_empty(Filter::Unread).title, "No unread chats");
    assert_eq!(list_empty(Filter::Archived).title, "No archived chats");
    for filter in Filter::EACH {
        assert!(!list_empty(filter).hint.is_empty(), "{filter:?}");
    }
}

#[test]
fn a_reply_names_the_quoted_author() {
    let mut quoting = message("M2", "text", "yes", true);
    quoting.reply_to = Some("M1".to_owned());
    let mut back = message("M3", "text", "good", false);
    back.reply_to = Some("M2".to_owned());
    let mut lost = message("M4", "text", "and?", false);
    lost.reply_to = Some("OLD".to_owned());
    let m = open_with(vec![
        message("M1", "text", "coffee?", false),
        quoting,
        back,
        lost,
    ]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(shown[1].quote_author.as_deref(), Some("Ana"));
    assert_eq!(shown[2].quote_author.as_deref(), Some("You"));
    assert_eq!(shown[3].quote_author, None);
}

#[test]
fn a_document_bubble_names_its_file_and_size() {
    use nuncio_client::wire::Media;
    let mut report = message("D1", "document", "", false);
    report.media = Some(Media {
        mime: "application/pdf".to_owned(),
        size: 1_258_291,
        file_name: "report.pdf".to_owned(),
    });
    let mut unnamed = message("D2", "document", "", false);
    unnamed.media = Some(Media {
        mime: "application/pdf".to_owned(),
        size: 900,
        file_name: String::new(),
    });
    let m = open_with(vec![report, unnamed, message("M1", "image", "", false)]);
    let shown = bubbles(&m, 0, None);
    assert_eq!(
        shown[0].document,
        Some(DocumentView {
            name: "report.pdf".to_owned(),
            size: "1.2 MB".to_owned()
        })
    );
    assert_eq!(
        shown[1].document,
        Some(DocumentView {
            name: "Document".to_owned(),
            size: "900 B".to_owned()
        })
    );
    assert_eq!(shown[2].document, None);
    assert_eq!(
        shown[0].label, "Ana, 10:05: [document]",
        "the label is unchanged"
    );
}

#[test]
fn reactions_are_chips_counted_by_emoji() {
    let reactions =
        [("👍", false), ("😂", false), ("👍", true)].map(|(emoji, mine)| ReactionView {
            emoji: emoji.to_owned(),
            mine,
        });
    assert_eq!(
        reaction_chips(&reactions),
        [
            ReactionChip {
                emoji: "👍".to_owned(),
                count: 2,
                mine: true
            },
            ReactionChip {
                emoji: "😂".to_owned(),
                count: 1,
                mine: false
            }
        ]
    );
}
