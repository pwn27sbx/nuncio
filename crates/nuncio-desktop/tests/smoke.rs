//! The window built for real on GTK, without a daemon and without showing it: the chat list and
//! the transcript are drawn from the model, rows carry their accessible roles, opening a chat and
//! sending go through the controller, and Enter-to-send empties the composer.
//!
//! GTK must run on the process's main thread (macOS enforces it), so this test has its own `main`
//! (`harness = false`). Where GTK cannot start (no display), it says so and passes: the logic it
//! drives is covered by the unit tests.

use adw::prelude::*;
use gtk::gdk::{Key, ModifierType};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nuncio_client::config::App;
use nuncio_client::wire::{MessageState, PROTOCOL_VERSION};
use nuncio_client::{Backoff, Chat, ClientFrame, DaemonFrame, Message};
use nuncio_desktop::controller::{Alert, Command, Controller};
use nuncio_desktop::lifecycle::Lifecycle;
use nuncio_desktop::palette::{Action, Target};
use nuncio_desktop::present::Filter;
use nuncio_desktop::style::Layout;
use nuncio_desktop::window::{Ui, WeakUi};
use nuncio_desktop::worker::{Incoming, Settings};

const ANA: &str = "15551230001@s.whatsapp.net";

fn chat(jid: &str, name: &str) -> Chat {
    Chat {
        jid: jid.to_owned(),
        name: name.to_owned(),
        unread: 1,
        archived: false,
        pinned: false,
        muted: false,
        aliases: Vec::new(),
        phone: None,
        last_message: None,
    }
}

fn message(id: &str, text: &str, from_me: bool) -> Message {
    Message {
        id: id.to_owned(),
        kind: "text".to_owned(),
        text: text.to_owned(),
        from_me,
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

fn check(ok: bool, what: &str) {
    if !ok {
        eprintln!("smoke: FAIL: {what}");
        std::process::exit(1);
    }
}

fn main() {
    if let Err(error) = adw::init() {
        println!("smoke: skipped, GTK cannot start here: {error}");
        return;
    }
    let ui = Ui::new(None, Controller::new("smoke"), 0, true);
    check(
        ui.drawn() == (0, 0),
        "nothing is drawn before the daemon answers",
    );
    ui.render();
    let (list, transcript) = ui.empty_states();
    check(
        list.as_ref().is_some_and(|(title, _)| title == "No chats"),
        "an empty list says so",
    );
    check(
        transcript
            == Some((
                "No messages yet".to_owned(),
                "Pick a chat to read it here".to_owned(),
            )),
        "an empty transcript says why",
    );

    ui.incoming(Incoming::Connected);
    let asked = ui.take_undelivered();
    check(
        asked.contains(&Command::Send(ClientFrame::chats(200))),
        "the first connect asks for the chats",
    );
    ui.incoming(Incoming::Reply {
        request: ClientFrame::chats(200),
        result: Ok(DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana"), chat("15551230002@s.whatsapp.net", "Ben")],
        }),
    });
    check(ui.drawn() == (2, 0), "two chat rows");

    ui.open_chat(0);
    let open = ui.take_undelivered();
    check(
        open == [Command::Send(ClientFrame::messages(ANA, 200))],
        "opening asks for its messages",
    );
    ui.incoming(Incoming::Reply {
        request: ClientFrame::messages(ANA, 200),
        result: Ok(DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("M2", "hello", true), message("M1", "hi", false)],
        }),
    });
    check(
        ui.take_undelivered() == [Command::Send(ClientFrame::mark_read(ANA))],
        "an opened unread chat is marked read once shown",
    );
    check(ui.drawn() == (2, 2), "two bubbles");
    check(
        ui.empty_states() == (None, None),
        "no empty state once there are rows",
    );
    let theirs = ui.message_parts(0).expect("a first bubble");
    let mine = ui.message_parts(1).expect("a second bubble");
    check(
        theirs
            .day
            .as_deref()
            .is_some_and(|day| day.contains("20 OCT"))
            && mine.day.is_none(),
        "one day label, above the day's first bubble",
    );
    check(
        theirs.author.as_deref() == Some("Ana") && mine.author.is_none(),
        "an author line above their group only",
    );
    check(
        theirs.meta.as_deref() == Some("10:00") && mine.meta.as_deref() == Some("10:00 · sent"),
        "each group's meta line under its last bubble",
    );
    check(
        ui.conversation_title() == "Ana",
        "the header names the chat",
    );
    check(
        ui.roles(0, 1)
            == (
                Some(gtk::AccessibleRole::ListItem),
                Some(gtk::AccessibleRole::ListItem),
            ),
        "chat rows and bubbles are list items for screen readers",
    );

    ui.type_draft("on my way\nsoon");
    check(
        ui.controller().model().composer().text() == "on my way\nsoon",
        "the model holds the draft as typed",
    );
    ui.send();
    let sent = ui.take_undelivered();
    check(
        matches!(sent.as_slice(), [Command::Send(ClientFrame::Send { text, .. })] if text == "on my way\nsoon"),
        "one send of the whole draft",
    );
    check(ui.draft().is_empty(), "the composer empties after a send");
    check(ui.drawn() == (2, 3), "the send shows at once");

    ui.incoming(Incoming::Lost(nuncio_client::ClientError::Closed));
    check(
        !ui.controller().model().connected(),
        "a loss is shown as reconnecting",
    );

    // The real worker and its channel into the main loop: with no daemon on the socket, a send
    // comes back not sent through the main loop.
    ui.start_worker(Settings {
        path: std::path::PathBuf::from(format!("/tmp/nds-{}.sock", std::process::id())),
        client: "nuncio-desktop/smoke".to_owned(),
        timeout: Duration::from_secs(1),
        send_wait: nuncio_client::driver::SEND_WAIT,
        backoff: Backoff::default(),
    });
    ui.type_draft("are you there?");
    ui.send();
    let deadline = Instant::now() + Duration::from_secs(5);
    let not_sent = |ui: &Ui| {
        ui.controller()
            .model()
            .loaded()
            .last()
            .is_some_and(|m| m.state == Some(MessageState::NotSent))
    };
    while !not_sent(&ui) && Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
    check(not_sent(&ui), "the worker's answer reaches the main loop");
    let daemon = fake_daemon();
    connects_and_loads_the_chats_with_no_ui_handle_kept(&daemon, "a fake daemon");
    notifications_and_the_open_chat_action();
    a_link_fills_the_composer_and_sends_nothing();
    the_rail_button_asks_for_a_number_and_opens_its_chat();
    the_composer_keys_send_button_and_reply_strip();
    closing_in_background_mode_hides_and_keeps_the_state_and_worker(&daemon);
    the_palette_opens_on_f1_and_jumps_to_a_chat();
    the_settings_write_the_file_and_apply_at_once();
    the_search_is_inline_in_the_conversation();
    pictures_show_their_thumbnail_at_once_and_the_full_one_on_a_click();
    the_panes_the_filters_and_the_strip();
    a_filter_keeps_the_open_chat_and_its_selection();
    // The real demo daemon, when its binary is given (`go build -o DIR/nuncio-demo ./cmd/nuncio-demo`).
    if let Some(demo) = std::env::var_os("NUNCIO_DEMO_BIN") {
        let runtime = PathBuf::from(format!("/tmp/ndsd-{}", std::process::id()));
        std::fs::create_dir_all(&runtime).expect("runtime dir");
        let mut child = std::process::Command::new(demo)
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start nuncio-demo");
        let socket = runtime.join("nuncio/core.sock");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        connects_and_loads_the_chats_with_no_ui_handle_kept(&socket, "nuncio-demo");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&runtime);
    }
    println!("smoke: ok");
}

/// As `main.rs` does: build the window, start the worker on `socket`, and keep no `Ui`: the
/// window alone must keep its state and worker alive, connect, and load the chats.
fn connects_and_loads_the_chats_with_no_ui_handle_kept(socket: &Path, what: &str) {
    let ui = Ui::new(None, Controller::new("owner"), 0, true);
    let window = ui.window().clone();
    ui.start_worker(Settings {
        path: socket.to_owned(),
        client: "nuncio-desktop/smoke".to_owned(),
        timeout: Duration::from_secs(5),
        send_wait: nuncio_client::driver::SEND_WAIT,
        backoff: Backoff::default(),
    });
    let weak = ui.downgrade();
    drop(ui);
    let loaded = |weak: &WeakUi| {
        weak.upgrade()
            .is_some_and(|ui| ui.controller().model().connected() && ui.drawn().0 > 0)
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !loaded(&weak) && Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
    check(weak.upgrade().is_some(), "the window keeps its state alive");
    check(
        loaded(&weak),
        &format!("connected to {what} and the chats loaded"),
    );
    // Realized but never shown: `close` only acts on a realized window.
    gtk::prelude::WidgetExt::realize(&window);
    window.close();
    for _ in 0..50 {
        glib::MainContext::default().iteration(false);
    }
    check(
        weak.upgrade().is_none(),
        "closing the window lets its state go",
    );
}

const BEN: &str = "15551230002@s.whatsapp.net";
const CARA: &str = "15551230003@s.whatsapp.net";

/// A window with no worker and Ana and Ben listed (the commands stay undelivered).
fn listed() -> Ui {
    let ui = Ui::new(None, Controller::new("smoke"), 0, true);
    ui.incoming(Incoming::Connected);
    ui.incoming(Incoming::Reply {
        request: ClientFrame::chats(200),
        result: Ok(DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana"), chat(BEN, "Ben")],
        }),
    });
    ui.take_undelivered();
    ui
}

/// The rail's filters list their chats by the row's own chat, the connection is said once (in
/// the rail) with a stop reason as a strip under the conversation header, and one pane at a time
/// goes back to the list.
fn the_panes_the_filters_and_the_strip() {
    let ui = Ui::new(None, Controller::new("smoke"), 0, true);
    ui.incoming(Incoming::Connected);
    ui.incoming(Incoming::Reply {
        request: ClientFrame::chats(200),
        result: Ok(DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![
                Chat {
                    unread: 0,
                    ..chat(ANA, "Ana")
                },
                chat(BEN, "Ben"),
            ],
        }),
    });
    ui.take_undelivered();
    check(ui.drawn().0 == 2, "All lists both chats");
    ui.set_filter(Filter::Unread);
    check(ui.drawn().0 == 1, "Unread lists only Ben");
    ui.set_filter(Filter::Archived);
    check(ui.drawn().0 == 0, "nothing is archived");
    ui.set_filter(Filter::Unread);
    check(
        ui.roles(0, 0).0 == Some(gtk::AccessibleRole::ListItem),
        "a filtered row is a list item",
    );
    ui.set_filter(Filter::All);

    ui.incoming(Incoming::Event(DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: nuncio_client::Session::Linked,
        stop_reason: None,
    }));
    check(ui.connection_word() == "linked", "the rail says linked");
    check(ui.stop_strip().is_none(), "no strip without a stop reason");
    ui.incoming(Incoming::Event(DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: nuncio_client::Session::Offline,
        stop_reason: Some(nuncio_client::wire::StopReason::KeyringLocked),
    }));
    check(
        ui.stop_strip().as_deref() == Some("keyring locked: unlock it to connect"),
        "the stop reason is a strip under the header",
    );
    check(
        ui.connection_word() == "offline",
        "the rail keeps the short word",
    );

    ui.apply_layout(Layout::SinglePane);
    check(
        ui.panes_shown() == (false, true, false),
        "one pane: the list",
    );
    ui.open_chat(1);
    ui.take_undelivered();
    check(
        ui.panes_shown() == (false, false, true),
        "opening shows the conversation",
    );
    check(
        ui.conversation_title() == "Ben",
        "the header names the chat only",
    );
    ui.apply_layout(Layout::CompactRail);
    check(
        ui.panes_shown() == (true, true, true),
        "half a screen: three panes again",
    );
    ui.apply_layout(Layout::ThreePanes);
    check(
        ui.layout() == Layout::ThreePanes,
        "the layout is the one applied",
    );
}

/// A window listing Ana and Ben (both unread) and Cara (archived).
fn three_chats() -> Ui {
    let ui = Ui::new(None, Controller::new("smoke"), 0, true);
    ui.incoming(Incoming::Connected);
    ui.incoming(Incoming::Reply {
        request: ClientFrame::chats(200),
        result: Ok(DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![
                chat(ANA, "Ana"),
                chat(BEN, "Ben"),
                Chat {
                    archived: true,
                    unread: 0,
                    ..chat(CARA, "Cara")
                },
            ],
        }),
    });
    ui.take_undelivered();
    ui
}

/// Mark `jid` read, as the daemon does once its messages were shown.
fn read(ui: &Ui, jid: &str) {
    ui.incoming(Incoming::Event(DaemonFrame::EventUnread {
        v: PROTOCOL_VERSION,
        unread: 0,
        jid: jid.to_owned(),
        chat_unread: 0,
    }));
    ui.take_undelivered();
}

/// The open chat stays only in the filter it was opened from, the counts never include it, and
/// the opened chat's row is the selected one whatever the rows around it did.
fn a_filter_keeps_the_open_chat_and_its_selection() {
    let ui = three_chats();
    ui.set_filter(Filter::Unread);
    ui.open_chat(1);
    ui.take_undelivered();
    read(&ui, BEN);
    check(
        ui.drawn().0 == 2,
        "Ben, read, stays in Unread under the cursor",
    );
    check(
        ui.filter_count(Filter::Unread) == "1",
        "Unread counts only Ana",
    );
    check(
        ui.filter_count(Filter::Archived) == "1",
        "Archived counts only Cara",
    );
    ui.set_filter(Filter::Archived);
    check(
        ui.drawn().0 == 1,
        "Archived lists only Cara, not the open Ben",
    );
    ui.open_chat(2);
    ui.take_undelivered();
    check(
        ui.selected_chat_row() == Some(0),
        "opening Cara in Archived selects her row",
    );

    let ui = three_chats();
    ui.set_filter(Filter::Unread);
    ui.open_chat(0);
    ui.take_undelivered();
    read(&ui, ANA);
    check(
        ui.selected_chat_row() == Some(0),
        "Ana, read, stays selected",
    );
    ui.open_chat(1);
    ui.take_undelivered();
    check(
        ui.drawn().0 == 1,
        "Ana leaves Unread once another chat opens",
    );
    check(
        ui.selected_chat_row() == Some(0),
        "Ben, now at row 0, is selected",
    );
}

/// A new message gives a notification (kept, since no `GApplication` is attached: nothing is
/// shown on this desktop), and the `open-chat` action its `default` action names opens that chat
/// and withdraws it.
fn notifications_and_the_open_chat_action() {
    let ui = listed();
    ui.incoming(Incoming::Event(DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: BEN.to_owned(),
        message: message("M9", "secret plans", false),
    }));
    let alerts = ui.take_undelivered_alerts();
    check(
        matches!(alerts.as_slice(), [Alert::Show(n)] if n.chat_jid == BEN && n.title == "Ben" && n.body == "New message"),
        "a new message notifies with no text",
    );

    let shown = std::rc::Rc::new(std::cell::Cell::new(0));
    let action = ui.open_chat_action({
        let shown = std::rc::Rc::clone(&shown);
        move |_| shown.set(shown.get() + 1)
    });
    check(
        action.name() == "open-chat",
        "the action is the one notifications name (app.open-chat)",
    );
    let group = gtk::gio::SimpleActionGroup::new();
    group.add_action(&action);
    group.activate_action("open-chat", Some(&BEN.to_variant()));
    check(
        ui.controller().model().selected_jid() == Some(BEN) && ui.controller().model().is_open(),
        "the action opens the chat it names",
    );
    check(
        ui.take_undelivered() == [Command::Send(ClientFrame::messages(BEN, 200))],
        "and asks for its messages",
    );
    check(shown.get() == 1, "the window is shown");
    check(
        ui.conversation_title() == "Ben",
        "the header names that chat",
    );
    check(
        ui.take_undelivered_alerts() == [Alert::Withdraw(BEN.to_owned())],
        "opening the chat withdraws its notification",
    );
}

fn a_link_fills_the_composer_and_sends_nothing() {
    // `HANDLES_OPEN` hands a command-line link over as a `GFile`. GLib normalises the URI it
    // returns (`whatsapp://send?…` gains the empty path, `whatsapp://send/?…`), so what has to
    // survive is the request the link names, not the exact bytes.
    for link in [
        "whatsapp://send?phone=15551230002&text=hola%20Ben",
        "https://wa.me/15551230002?text=hola%20Ben",
    ] {
        let given = nuncio_client::parse_link(link);
        let normalised = gtk::gio::File::for_commandline_arg(link).uri();
        check(
            given.is_ok() && given == nuncio_client::parse_link(&normalised),
            &format!("the open handler gets the link's request through GFile: {link}"),
        );
    }
    let ui = listed();
    ui.open_link(
        &gtk::gio::File::for_commandline_arg("https://wa.me/15551230002?text=hola%20Ben").uri(),
    );
    check(
        ui.draft() == "hola Ben",
        "the link's text is in the composer",
    );
    check(
        ui.take_undelivered() == [Command::Send(ClientFrame::messages(BEN, 200))],
        "the chat opens and nothing is sent",
    );
    ui.open_link("https://chat.whatsapp.com/AbCdEf123");
    check(ui.draft() == "hola Ben", "a group invite changes nothing");
    check(ui.take_undelivered().is_empty(), "and asks nothing");
}

/// The rail's one filled button is "New message" (S6): it asks for a phone number in a §6.8
/// dialog; Escape cancels, an invalid number says why and opens nothing, a valid one opens its
/// chat as a `whatsapp://send?phone=…` link with no text does: an empty, focused composer and
/// nothing sent. The palette offers it too.
fn the_rail_button_asks_for_a_number_and_opens_its_chat() {
    let ui = listed();
    let (label, tooltip, named) = ui.rail_primary();
    check(
        label == "New message",
        &format!("the rail button's label: {label}"),
    );
    check(
        tooltip.as_deref() == Some("New message"),
        &format!("its tooltip, also on the icon strip: {tooltip:?}"),
    );
    check(named, "it has an accessible name");
    ui.apply_layout(Layout::CompactRail);
    check(
        ui.rail_primary_icon_shown(),
        "the icon strip shows its icon",
    );
    ui.apply_layout(Layout::ThreePanes);

    check(!ui.new_message_open(), "no dialog at first");
    ui.click_rail_primary();
    check(ui.new_message_open(), "the button asks for a number");
    check(ui.new_message_width() == 480, "a 480 px dialog (plan §6.8)");
    check(ui.new_message_focused(), "the number field has the focus");
    ui.new_message_type("+1 555 123 0003");
    check(ui.new_message_key(Key::Escape), "Escape is the dialog's");
    check(!ui.new_message_open(), "Escape cancels");
    check(
        !ui.controller().model().is_open() && ui.take_undelivered().is_empty(),
        "and opens nothing",
    );

    ui.click_rail_primary();
    ui.new_message_type("1555abc");
    check(ui.new_message_key(Key::Return), "Enter is the dialog's");
    check(
        ui.new_message_open(),
        "an invalid number keeps the dialog open",
    );
    check(
        ui.new_message_error().as_deref() == Some("the phone number is not 7 to 15 digits"),
        &format!("and says why: {:?}", ui.new_message_error()),
    );
    check(
        !ui.controller().model().is_open() && ui.take_undelivered().is_empty(),
        "an invalid number opens nothing and sends nothing",
    );
    ui.new_message_type("+1 555 123 0003");
    check(
        ui.new_message_error().is_none(),
        "typing again clears the error",
    );
    check(!ui.new_message_key(Key::a), "a letter is typed");
    check(ui.new_message_key(Key::Return), "Enter confirms");
    check(!ui.new_message_open(), "a valid number closes the dialog");
    check(
        ui.controller().model().selected_jid() == Some("15551230003@s.whatsapp.net")
            && ui.controller().model().is_open(),
        "and opens that chat",
    );
    check(
        ui.take_undelivered()
            == [Command::Send(ClientFrame::messages(
                "15551230003@s.whatsapp.net",
                200,
            ))],
        "it asks for its messages and sends nothing",
    );
    check(ui.draft().is_empty(), "the composer is empty");
    check(ui.composer_focused(), "and has the focus");

    // S8: a draft typed in another chat never goes to the new number, even with a double Enter.
    ui.open_chat(0);
    ui.take_undelivered();
    ui.type_draft("meant for ana");
    ui.click_rail_primary();
    ui.new_message_type("+1 555 123 0004");
    check(ui.new_message_key(Key::Return), "Enter confirms");
    check(
        ui.draft().is_empty() && ui.controller().model().composer().text().is_empty(),
        &format!(
            "New message always opens with an empty composer: {:?}",
            ui.draft()
        ),
    );
    check(ui.composer_focused(), "the empty composer has the focus");
    check(
        ui.composer_press(Key::Return, ModifierType::empty()),
        "the second Enter reaches the composer",
    );
    let after = ui.take_undelivered();
    check(
        !after
            .iter()
            .any(|c| matches!(c, Command::Send(ClientFrame::Send { .. }))),
        &format!("a double Enter sends nothing: {after:?}"),
    );
    // R9: Ana's draft was kept for Ana, not lost.
    ui.open_chat(0);
    check(
        ui.controller().model().selected_jid() == Some(ANA),
        "Ana's chat opens again",
    );
    check(
        ui.draft() == "meant for ana",
        &format!("and her draft is back: {:?}", ui.draft()),
    );
    ui.take_undelivered();

    // The palette's "New message…" opens the same dialog.
    check(ui.press("F1") && ui.palette_open(), "F1 opens the palette");
    ui.palette_type("new message");
    check(
        ui.palette_selected().as_deref() == Some("New message…"),
        "the palette lists New message",
    );
    ui.palette_enter();
    iterate();
    check(!ui.palette_open(), "the palette closes");
    check(ui.new_message_open(), "and the dialog asks for a number");
    ui.new_message_type("1-555-123-0002");
    check(
        ui.new_message_key(Key::KP_Enter),
        "the keypad's Enter confirms too",
    );
    check(
        ui.controller().model().selected_jid() == Some(BEN),
        "a listed chat opens as itself",
    );
    check(
        ui.take_undelivered() == [Command::Send(ClientFrame::messages(BEN, 200))],
        "and nothing is sent",
    );
}

/// A `width` x `height` gradient JPEG, as the demo's photo thumbnail (48x32) is.
fn gradient_jpeg(width: u32, height: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let picture = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            (x * 255 / (width - 1)) as u8,
            (y * 255 / (height - 1)) as u8,
            160,
        ])
    });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 70)
        .write_image(
            picture.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )
        .expect("encode a jpeg");
    bytes
}

/// A 64x64 PNG disc on a transparent square, as the demo's sticker thumbnails are.
fn disc_png() -> Vec<u8> {
    use image::ImageEncoder;
    let picture = image::RgbaImage::from_fn(64, 64, |x, y| {
        let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
        if dx * dx + dy * dy < 26.0 * 26.0 {
            image::Rgba([250, 200, 40, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(picture.as_raw(), 64, 64, image::ExtendedColorType::Rgba8)
        .expect("encode a png");
    bytes
}

/// A 512x512 animated WebP sticker of `frames` frames, each of its own colour, 80 ms each.
fn animated_sticker(frames: usize) -> Vec<u8> {
    use image::ImageEncoder;
    fn u24(value: u32) -> [u8; 3] {
        let [a, b, c, _] = value.to_le_bytes();
        [a, b, c]
    }
    fn chunk(name: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = name.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }
    let side = 512;
    let mut vp8x = vec![0x10 | 0x02, 0, 0, 0];
    vp8x.extend_from_slice(&u24(side - 1));
    vp8x.extend_from_slice(&u24(side - 1));
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for n in 0..frames {
        let colour = image::Rgba([(n * 60) as u8, 120, 255 - (n * 60) as u8, 255]);
        let picture = image::RgbaImage::from_pixel(side, side, colour);
        let mut still = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut still)
            .write_image(
                picture.as_raw(),
                side,
                side,
                image::ExtendedColorType::Rgba8,
            )
            .expect("encode a webp");
        let mut anmf = Vec::new();
        for value in [0, 0, side - 1, side - 1, 80] {
            anmf.extend_from_slice(&u24(value));
        }
        anmf.push(0x02);
        anmf.extend_from_slice(&still[12..]);
        body.extend(chunk(b"ANMF", &anmf));
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}

fn wait_for(ui: &Ui, done: impl Fn(&Ui) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(ui) && Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
    done(ui)
}

/// A photo and a sticker as the demo sends them: each bubble reserves its box from the message's
/// size before anything is decoded, its thumbnail becomes a texture of the thumbnail's size in
/// that box, nothing is downloaded until the picture is clicked, and the full picture the click
/// fetched replaces the thumbnail in the same box. A thumbnail that does not decode keeps the
/// label, with nothing drawn.
fn pictures_show_their_thumbnail_at_once_and_the_full_one_on_a_click() {
    use base64::Engine;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let ui = listed();
    ui.open_chat(0);
    ui.take_undelivered();
    let mut photo = message("DEMOP1", "the beach", false);
    photo.kind = "image".to_owned();
    photo.thumb = Some(encode(&gradient_jpeg(48, 32)));
    photo.thumb_mime = Some("image/jpeg".to_owned());
    (photo.width, photo.height) = (Some(480), Some(320));
    let mut sticker = message("DEMOS1", "", false);
    sticker.kind = "sticker".to_owned();
    sticker.thumb = Some(encode(&disc_png()));
    sticker.thumb_mime = Some("image/png".to_owned());
    (sticker.width, sticker.height) = (Some(512), Some(512));
    sticker.animated = true;
    let mut broken = sticker.clone();
    broken.id = "DEMOS2".to_owned();
    broken.thumb = Some(encode(b"not a picture"));
    let mut no_size = photo.clone();
    no_size.id = "DEMOP2".to_owned();
    (no_size.width, no_size.height) = (None, None);
    ui.incoming(Incoming::Reply {
        request: ClientFrame::messages(ANA, 200),
        result: Ok(DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![no_size, broken, sticker, photo],
        }),
    });
    let reserved = |ui: &Ui, index| ui.picture(index).map(|p| p.reserved);
    let laid_out = |ui: &Ui, index| ui.picture(index).map(|p| p.laid_out);
    let texture = |ui: &Ui, index| ui.picture(index).and_then(|p| p.texture);
    check(
        reserved(&ui, 0) == Some((300, 200))
            && laid_out(&ui, 0) == Some((300, 200))
            && texture(&ui, 0).is_none(),
        "the photo's box is reserved before its thumbnail is decoded",
    );
    check(
        reserved(&ui, 1) == Some((160, 160)),
        "the sticker's box too",
    );
    check(
        reserved(&ui, 3) == Some((-1, -1)),
        "a picture whose message states no size has no box yet",
    );
    check(
        wait_for(&ui, |ui| {
            texture(ui, 0).is_some() && texture(ui, 1).is_some() && texture(ui, 3).is_some()
        }),
        "the thumbnails are decoded off the main loop and drawn",
    );
    check(
        texture(&ui, 0) == Some((48, 32)) && laid_out(&ui, 0) == Some((300, 200)),
        "the photo bubble holds the 48x32 thumbnail in its 300x200 box",
    );
    check(
        texture(&ui, 1) == Some((64, 64)) && laid_out(&ui, 1) == Some((160, 160)),
        "the sticker bubble holds its thumbnail in its 160x160 box",
    );
    check(
        ui.picture(0).and_then(|p| p.alt).as_deref() == Some("Photo, the beach")
            && ui.picture(1).and_then(|p| p.alt).as_deref() == Some("Sticker"),
        "the pictures say what they are",
    );
    check(
        laid_out(&ui, 3) == Some((300, 200)),
        "a picture with no stated size takes its box from its first image's shape",
    );
    for _ in 0..50 {
        glib::MainContext::default().iteration(false);
    }
    check(
        texture(&ui, 2).is_none() && reserved(&ui, 2) == Some((160, 160)),
        "a thumbnail that does not decode leaves its box empty",
    );
    check(
        !ui.take_undelivered()
            .iter()
            .any(|c| matches!(c, Command::Send(ClientFrame::DownloadMedia { .. }))),
        "nothing is downloaded by itself",
    );

    let dir = std::env::temp_dir().join(format!("nuncio-smoke-pictures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let ready = |ui: &Ui, id: &str, path: &Path| {
        ui.incoming(Incoming::Reply {
            request: ClientFrame::download_media(ANA, id),
            result: Ok(DaemonFrame::MediaReady {
                v: PROTOCOL_VERSION,
                jid: ANA.to_owned(),
                id: id.to_owned(),
                path: path.to_string_lossy().into_owned(),
                mime: "image/jpeg".to_owned(),
            }),
        });
    };
    let asked = |ui: &Ui, id: &str| {
        ui.take_undelivered() == [Command::Send(ClientFrame::download_media(ANA, id))]
    };

    // While a full picture downloads, the thumbnail stays in the rebuilt bubble.
    ui.click_picture(1);
    check(
        asked(&ui, "DEMOS1"),
        "a click fetches the sticker's full picture",
    );
    check(
        texture(&ui, 1) == Some((64, 64)) && laid_out(&ui, 1) == Some((160, 160)),
        "while it downloads its thumbnail stays",
    );
    // A thumbnail that left the cache is asked for again.
    ui.evict_pictures();
    ui.click_picture(0);
    check(asked(&ui, "DEMOP1"), "a click fetches the full picture");
    check(
        laid_out(&ui, 0) == Some((300, 200)),
        "the rebuilt bubble keeps its box",
    );
    check(
        wait_for(&ui, |ui| texture(ui, 0) == Some((48, 32))),
        "an evicted thumbnail is decoded again while the full picture downloads",
    );
    let file = dir.join("DEMOP1.jpg");
    std::fs::write(&file, gradient_jpeg(480, 320)).expect("write the full picture");
    ready(&ui, "DEMOP1", &file);
    check(
        ui.take_undelivered().is_empty(),
        "the full picture is shown here, not opened outside",
    );
    check(
        wait_for(&ui, |ui| texture(ui, 0) == Some((480, 320))),
        "the full picture replaces the thumbnail",
    );
    check(
        laid_out(&ui, 0) == Some((300, 200)),
        "in the same box: nothing moves",
    );

    // No stated size: the full picture keeps the box the thumbnail gave, even with another shape.
    ui.click_picture(3);
    check(asked(&ui, "DEMOP2"), "a click fetches it");
    let wide = dir.join("DEMOP2.jpg");
    std::fs::write(&wide, gradient_jpeg(600, 200)).expect("write");
    ready(&ui, "DEMOP2", &wide);
    check(
        wait_for(&ui, |ui| texture(ui, 3) == Some((600, 200))),
        "its full picture arrives",
    );
    check(
        laid_out(&ui, 3) == Some((300, 200)),
        "and its box changed only once",
    );
    ui.evict_pictures();
    ui.click_picture(3);
    check(asked(&ui, "DEMOP2"), "asked again");
    check(
        laid_out(&ui, 3) == Some((300, 200)),
        "with its pictures evicted it keeps its box, never 0x0",
    );

    // A full picture that does not decode: a toast says why and the box stays empty.
    ui.click_picture(2);
    check(
        asked(&ui, "DEMOS2"),
        "a click on a picture with a broken thumbnail fetches it too",
    );
    let corrupt = dir.join("DEMOS2.webp");
    std::fs::write(&corrupt, b"RIFF\0\0\0\0WEBPVP8 broken").expect("write");
    ready(&ui, "DEMOS2", &corrupt);
    check(
        wait_for(&ui, |ui| {
            ui.last_toast()
                .is_some_and(|t| t.starts_with("cannot show the picture"))
        }),
        "a full picture that cannot be shown says so",
    );
    check(
        texture(&ui, 2).is_none() && laid_out(&ui, 2) == Some((160, 160)),
        "and its box stays as it was",
    );
    // The palette's Open goes the bubble's way: the failed picture is decoded once more.
    ui.run_palette_action(&Action::OpenMedia(Target {
        jid: ANA.to_owned(),
        id: "DEMOS2".to_owned(),
        client_id: None,
    }));
    check(asked(&ui, "DEMOS2"), "the palette's Open fetches it again");
    let good = dir.join("DEMOS2.png");
    std::fs::write(&good, disc_png()).expect("write");
    ready(&ui, "DEMOS2", &good);
    check(
        wait_for(&ui, |ui| texture(ui, 2) == Some((64, 64))),
        "and the picture that failed is shown once it decodes",
    );

    // The animated sticker's full picture, fetched by the click above, plays: every frame is
    // kept, shrunk to its box (not twice it, as a still picture). Its thumbnail did not.
    check(
        ui.picture(1).is_some_and(|p| p.frames == 1 && !p.animating),
        "an animated sticker's thumbnail stays still",
    );
    let moving = dir.join("DEMOS1.webp");
    std::fs::write(&moving, animated_sticker(3)).expect("write");
    ready(&ui, "DEMOS1", &moving);
    check(
        wait_for(&ui, |ui| {
            ui.picture(1)
                .is_some_and(|p| p.frames == 3 && p.animating && p.texture == Some((160, 160)))
        }),
        "an opened animated sticker plays its frames",
    );
    check(
        laid_out(&ui, 1) == Some((160, 160)),
        "in the same box: nothing moves",
    );
    // With the desktop asking for no animations it shows its first frame, still.
    let settings = gtk::Settings::default().expect("the default settings");
    settings.set_gtk_enable_animations(false);
    ui.evict_pictures();
    ui.click_picture(1);
    check(asked(&ui, "DEMOS1"), "asked again");
    ready(&ui, "DEMOS1", &moving);
    check(
        wait_for(&ui, |ui| {
            ui.picture(1)
                .is_some_and(|p| p.frames == 3 && p.texture == Some((160, 160)))
        }),
        "its frames are decoded again",
    );
    check(
        ui.picture(1).is_some_and(|p| !p.animating),
        "but nothing plays when animations are off",
    );
    settings.set_gtk_enable_animations(true);
    let _ = std::fs::remove_dir_all(&dir);
}

fn iterate() {
    for _ in 0..20 {
        glib::MainContext::default().iteration(false);
    }
}

/// F1 (and Ctrl+K) open the palette; typing filters it, Enter runs the chosen command, Escape
/// closes it. Choosing a chat opens it as a click does, and nothing is sent.
fn the_palette_opens_on_f1_and_jumps_to_a_chat() {
    let ui = listed();
    check(!ui.palette_open(), "no palette at first");
    check(ui.press("F1"), "F1 is a window shortcut");
    check(ui.palette_open(), "F1 opens the palette");
    let all = ui.palette_titles();
    for wanted in ["Settings", "Quit", "Ana", "Ben"] {
        check(
            all.iter().any(|t| t == wanted),
            &format!("the palette lists {wanted}: {all:?}"),
        );
    }
    check(
        !all.iter().any(|t| t.starts_with("Send")),
        "the palette has no send",
    );
    // Plan §6.5: a 560 px panel over a scrim, groups under headers, the first row selected.
    check(ui.palette_scrim_shown(), "a scrim dims the window behind");
    check(ui.palette_width() == 560, "the panel is 560 px wide");
    check(
        ui.palette_headers() == ["COMMANDS", "CHATS"],
        &format!("groups have headers: {:?}", ui.palette_headers()),
    );
    check(
        ui.palette_selected().as_deref() == Some("New message…"),
        "the first row is selected",
    );
    check(ui.palette_key(Key::Down), "Down is the palette's");
    check(
        ui.palette_selected().as_deref() == Some("Settings"),
        "Down moves the selection",
    );
    check(ui.palette_key(Key::Up), "Up is the palette's");
    check(
        ui.palette_selected().as_deref() == Some("New message…"),
        "Up moves it back",
    );
    check(ui.palette_key(Key::Up), "Up at the top");
    check(
        ui.palette_selected().as_deref() == Some("New message…"),
        "and stays on the first row",
    );
    check(!ui.palette_key(Key::a), "a letter is typed");
    ui.palette_type("BEN");
    check(
        ui.palette_titles().first().map(String::as_str) == Some("Ben"),
        "typing filters the palette, ignoring case",
    );
    check(
        ui.palette_selected().as_deref() == Some("Ben"),
        "and selects the first match",
    );
    ui.type_draft("half a thought");
    ui.palette_enter();
    iterate();
    check(!ui.palette_open(), "Enter closes the palette");
    check(
        ui.controller().model().selected_jid() == Some(BEN) && ui.controller().model().is_open(),
        "and opens the chat chosen",
    );
    check(ui.conversation_title() == "Ben", "the header names it");
    check(
        ui.take_undelivered() == [Command::Send(ClientFrame::messages(BEN, 200))],
        "it asks for its messages and sends nothing",
    );
    check(
        ui.draft().is_empty(),
        &format!("Ben shows his own draft, not Ana's: {:?}", ui.draft()),
    );
    ui.open_chat(0);
    check(
        ui.draft() == "half a thought",
        "Ana's draft stays unsent, for Ana",
    );
    ui.open_chat(1);
    ui.take_undelivered();

    check(
        ui.press("<Control>k") && ui.palette_open(),
        "Ctrl+K opens the palette too",
    );
    ui.palette_type("zzzz");
    check(ui.palette_titles().is_empty(), "nothing matches");
    ui.palette_enter();
    iterate();
    check(
        ui.take_undelivered().is_empty(),
        "Enter on nothing does nothing",
    );
    check(ui.palette_key(Key::Escape), "Escape is the palette's");
    iterate();
    check(!ui.palette_open(), "Escape closes the palette");
    check(!ui.palette_scrim_shown(), "and its scrim");
    // Another chat opening while the palette is open (a notification click, a link) closes it,
    // so none of its commands runs on the other conversation.
    ui.open_jid(ANA);
    ui.take_undelivered();
    check(
        ui.press("F1") && ui.palette_open(),
        "F1 opens the palette over Ana",
    );
    ui.open_jid(BEN);
    iterate();
    check(
        !ui.palette_open(),
        "opening another chat closes the palette",
    );
    ui.take_undelivered();
    check(
        ui.press("<Control>comma"),
        "Ctrl+, is the settings shortcut",
    );
}

/// Plan §6.6 with S7: the search of the open chat sits in the conversation, its results above the
/// transcript, its hint shows only while it is empty and focused, and `/` opens it unless the
/// composer is being typed in. Ctrl+F opens it too; the chat list keeps its title.
fn the_search_is_inline_in_the_conversation() {
    let empty = Ui::new(None, Controller::new("smoke"), 0, true);
    check(
        !empty.press("slash") && !empty.search_open(),
        "with no chat there is nothing to search",
    );
    let ui = listed();
    check(!ui.search_open(), "no search at first");
    ui.open_chat(0);
    ui.take_undelivered();
    check(ui.composer_focused(), "opening a chat focuses the composer");
    check(
        !ui.press("slash") && !ui.search_open(),
        "/ in the composer is typed, not a search",
    );
    ui.focus_chat_list();
    check(
        ui.press("slash") && ui.search_open(),
        "/ outside the composer opens the search",
    );
    check(
        ui.search_in_conversation(),
        "the input is in the conversation pane",
    );
    check(ui.list_title_shown(), "the chat list keeps its title");
    check(
        ui.search_hint()
            .is_some_and(|hint| hint.contains("every word")),
        &format!("the hint, while empty and focused: {:?}", ui.search_hint()),
    );
    ui.focus_chat_list();
    iterate();
    check(
        ui.search_hint().is_none(),
        "no hint once the input loses the focus",
    );
    check(
        ui.press("slash"),
        "/ takes the focus back to the open search",
    );
    check(ui.search_hint().is_some(), "and the hint comes back");
    ui.type_search("beach");
    check(ui.search_hint().is_none(), "no hint once typed");
    check(!ui.results_shown(), "no results until they come");
    ui.submit_search();
    let sent = ui.take_undelivered();
    let Some(Command::Send(request)) = sent.first().cloned() else {
        check(false, &format!("a search is asked: {sent:?}"));
        return;
    };
    check(
        matches!(&request, ClientFrame::Search { query, .. } if query == "beach"),
        &format!("the daemon is asked: {request:?}"),
    );
    ui.incoming(Incoming::Reply {
        request,
        result: Ok(DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("S1", "the beach at noon", false)],
        }),
    });
    check(
        ui.results_shown(),
        "the results show in the conversation, above the transcript",
    );
    check(ui.drawn().0 == 2, "and the chat rows stay listed");
    ui.close_search();
    check(!ui.search_open(), "Escape closes it");
    check(!ui.results_shown(), "and its results go");
    check(
        ui.press("<Control>f") && ui.search_open(),
        "Ctrl+F still opens the search",
    );
    ui.close_search();
}

/// The settings over a temporary `config.toml` (never the owner's): a change is written at once
/// and reaches the running app; a file that does not load is shown and never overwritten.
fn the_settings_write_the_file_and_apply_at_once() {
    let dir = std::env::temp_dir().join(format!("nuncio-smoke-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("nuncio").join("config.toml");
    let ui = listed();
    ui.set_config_path(Some(path.clone()));

    let view = ui.settings_view();
    check(view.problem.is_none(), "a missing file can be edited");
    check(!path.exists(), "opening the settings writes nothing");
    view.show_preview.set_active(true);
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    check(
        written.contains("show_preview = true") && written.starts_with('#'),
        &format!("the change is written with its header: {written}"),
    );
    check(
        ui.controller().config().notifications.show_preview,
        "and the running app follows it",
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| {
            std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0)
        };
        check(mode(&path) == 0o600, "the file is private (0600)");
        check(
            path.parent().is_some_and(|dir| mode(dir) == 0o700),
            "in a private directory (0700)",
        );
    }
    // Plan §6.7: one 480 px column, sections under meta headers.
    check(view.window.default_width() == 480, "a 480 px column");
    check(
        view.headers() == ["NOTIFICATIONS", "WINDOW", "TERMINAL CLIENT"],
        &format!("the sections' headers: {:?}", view.headers()),
    );
    check(ui.title_bar_shown(), "the title bar is shown at first");
    view.title_bar.set_selected(2);
    check(!ui.title_bar_shown(), "choosing Hidden hides it at once");
    view.title_bar.set_selected(1);
    check(ui.title_bar_shown(), "choosing Shown shows it again");
    // A hand edit to another key while the window is open is kept, and shown.
    let edited = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("start_hidden = false", "start_hidden = true");
    std::fs::write(&path, edited).expect("hand edit");
    view.background.set_active(false);
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    check(
        written.contains("background = false") && written.contains("start_hidden = true"),
        &format!("the change and the hand edit are both in the file: {written}"),
    );
    check(
        view.start_hidden.is_active(),
        "the window shows the hand edit",
    );
    view.window.destroy();

    let mine = "# my own notes\n[notifications]\nshow_previews = true\n";
    std::fs::write(&path, mine).expect("write");
    let view = ui.settings_view();
    check(
        view.problem
            .as_deref()
            .is_some_and(|p| p.contains("show_previews")),
        "a file with an unknown key is shown as the problem",
    );
    check(
        !view.notifications.is_sensitive(),
        "and its settings cannot be changed",
    );
    view.notifications.set_active(false);
    check(
        std::fs::read_to_string(&path).ok().as_deref() == Some(mine),
        "the file is never overwritten",
    );
    check(
        view.notifications.is_active(),
        "the refused switch goes back to the saved value",
    );
    check(
        ui.controller().config().notifications.enabled,
        "and the app keeps its settings",
    );
    view.window.destroy();
    let _ = std::fs::remove_dir_all(&dir);
}

/// `[app] background = true`: closing hides the window; the state and the worker stay until Quit.
fn closing_in_background_mode_hides_and_keeps_the_state_and_worker(socket: &Path) {
    let ui = Ui::new(None, Controller::new("bg"), 0, true);
    ui.set_lifecycle(Lifecycle::new(&App::default()));
    let window = ui.window().clone();
    ui.start_worker(Settings {
        path: socket.to_owned(),
        client: "nuncio-desktop/smoke".to_owned(),
        timeout: Duration::from_secs(5),
        send_wait: nuncio_client::driver::SEND_WAIT,
        backoff: Backoff::default(),
    });
    let weak = ui.downgrade();
    drop(ui);
    gtk::prelude::WidgetExt::realize(&window);
    window.close();
    for _ in 0..50 {
        glib::MainContext::default().iteration(false);
    }
    check(
        !window.is_visible(),
        "closing in background mode hides the window",
    );
    let alive = weak.upgrade();
    check(alive.is_some(), "the hidden window keeps its state");
    check(
        alive.as_ref().is_some_and(Ui::worker_running),
        "and its daemon worker",
    );
    // The worker still folds the daemon's answers in while hidden.
    let deadline = Instant::now() + Duration::from_secs(5);
    let loaded = |weak: &WeakUi| weak.upgrade().is_some_and(|ui| ui.drawn().0 > 0);
    while !loaded(&weak) && Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(5));
    }
    check(loaded(&weak), "the hidden window still loads the chats");
    if let Some(ui) = alive {
        ui.quit();
    }
    for _ in 0..50 {
        glib::MainContext::default().iteration(false);
    }
    check(
        weak.upgrade().is_none(),
        "Quit lets the state and the worker go",
    );
}

/// A daemon answering as `nunciod` and `nuncio-demo` do (the shared wire fixtures): hello on both
/// connections, subscribe on the second, then status and chats.
fn fake_daemon() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/ndsf-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    path
}

fn serve(stream: UnixStream) {
    let mut out = stream.try_clone().expect("clone");
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let reply = if line.contains("\"op\":\"hello\"") {
            include_str!("../../../proto/fixtures/hello_ok_daemon.json")
        } else if line.contains("\"op\":\"subscribe\"") {
            include_str!("../../../proto/fixtures/subscribed_response.json")
        } else if line.contains("\"op\":\"status\"") {
            include_str!("../../../proto/fixtures/status_response.json")
        } else if line.contains("\"op\":\"chats\"") {
            include_str!("../../../proto/fixtures/chats_response.json")
        } else {
            continue;
        };
        if writeln!(out, "{}", reply.trim()).is_err() {
            return;
        }
    }
}

/// The composer (plan §6.4, §7): Send shows only with text and says `Sending…` while a send
/// waits; Enter and Ctrl+Enter send, Alt+Enter starts a line, Escape cancels a reply, whose strip
/// shows the quoted message; the field grows to six lines, then scrolls.
fn the_composer_keys_send_button_and_reply_strip() {
    let ui = listed();
    ui.open_chat(0);
    ui.take_undelivered();
    ui.incoming(Incoming::Reply {
        request: ClientFrame::messages(ANA, 200),
        result: Ok(DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: vec![message("M1", "coffee?", false)],
        }),
    });
    ui.take_undelivered();

    check(ui.send_button().is_none(), "no Send button with no text");
    ui.type_draft("   \n ");
    check(
        ui.send_button().is_none(),
        "no Send button with only spaces",
    );
    ui.type_draft("hi");
    check(
        ui.send_button() == Some(("Send".to_owned(), true)),
        "Send shows with text",
    );
    check(
        ui.send_tooltip().contains("Enter") && !ui.send_tooltip().contains("Ctrl"),
        "the tooltip names Enter",
    );
    let (cap, line) = ui.composer_height_cap();
    check(
        line > 0 && cap == nuncio_desktop::style::composer_max_height(line),
        "the field stops growing at six lines",
    );

    check(
        ui.composer_press(Key::Return, ModifierType::ALT_MASK),
        "Alt+Enter is the composer's",
    );
    check(ui.draft() == "hi\n", "Alt+Enter starts a new line");
    check(ui.take_undelivered().is_empty(), "Alt+Enter sends nothing");

    check(
        ui.composer_press(Key::Return, ModifierType::empty()),
        "Enter is the composer's",
    );
    let sent = ui.take_undelivered();
    let Some(Command::Send(request)) = sent.first().cloned() else {
        check(false, "Enter sends");
        return;
    };
    check(
        matches!(&request, ClientFrame::Send { text, .. } if text == "hi\n") && sent.len() == 1,
        "Enter sends the draft once",
    );
    check(ui.draft().is_empty(), "the composer empties after Enter");
    check(
        ui.send_button() == Some(("Sending…".to_owned(), false)),
        "the button says Sending… and cannot be pressed while the send waits",
    );
    let ClientFrame::Send { client_id, .. } = &request else {
        unreachable!("matched above")
    };
    ui.incoming(Incoming::Reply {
        request: request.clone(),
        result: Ok(DaemonFrame::Sent {
            v: PROTOCOL_VERSION,
            client_id: client_id.clone(),
            state: nuncio_client::wire::SendState::Sent,
            id: Some("3EB0".to_owned()),
            timestamp: Some("2026-10-20T10:01:00Z".to_owned()),
        }),
    });
    ui.take_undelivered();
    check(ui.send_button().is_none(), "answered: no button again");

    ui.type_draft("again");
    check(
        ui.composer_press(Key::KP_Enter, ModifierType::CONTROL_MASK),
        "Ctrl+Enter is the composer's",
    );
    check(
        matches!(ui.take_undelivered().as_slice(), [Command::Send(ClientFrame::Send { text, .. })] if text == "again"),
        "Ctrl+Enter sends too",
    );

    check(ui.reply_strip().is_none(), "no reply strip before a reply");
    ui.run_palette_action(&Action::Reply(Target {
        jid: ANA.to_owned(),
        id: "M1".to_owned(),
        client_id: None,
    }));
    check(
        ui.reply_strip().as_deref() == Some("Replying to: coffee?"),
        "the reply strip quotes the message",
    );
    check(
        ui.composer_press(Key::Escape, ModifierType::empty()),
        "Escape is the composer's",
    );
    check(ui.reply_strip().is_none(), "Escape cancels the reply");
    check(
        ui.controller().model().composer().reply_to().is_none(),
        "the model no longer replies",
    );
}
