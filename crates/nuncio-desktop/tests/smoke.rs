//! The window built for real on GTK, without a daemon and without showing it: the chat list and
//! the transcript are drawn from the model, rows carry their accessible roles, opening a chat and
//! sending go through the controller, and Enter-to-send empties the composer.
//!
//! GTK must run on the process's main thread (macOS enforces it), so this test has its own `main`
//! (`harness = false`). Where GTK cannot start (no display), it says so and passes: the logic it
//! drives is covered by the unit tests.

use gtk::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nuncio_client::config::App;
use nuncio_client::wire::{MessageState, PROTOCOL_VERSION};
use nuncio_client::{Backoff, Chat, ClientFrame, DaemonFrame, Message};
use nuncio_desktop::controller::{Alert, Command, Controller};
use nuncio_desktop::lifecycle::Lifecycle;
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
    closing_in_background_mode_hides_and_keeps_the_state_and_worker(&daemon);
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
    // `HANDLES_OPEN` hands a command-line link over as a `GFile`: its URI must come back intact.
    for link in [
        "whatsapp://send?phone=15551230002&text=hola%20Ben",
        "https://wa.me/15551230002?text=hola%20Ben",
    ] {
        check(
            gtk::gio::File::for_commandline_arg(link).uri() == link,
            &format!("the open handler gets the link as given: {link}"),
        );
    }
    let ui = listed();
    ui.open_link("https://wa.me/15551230002?text=hola%20Ben");
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

/// `[app] background = true`: closing hides the window; the state and the worker stay until Quit.
fn closing_in_background_mode_hides_and_keeps_the_state_and_worker(socket: &Path) {
    let ui = Ui::new(None, Controller::new("bg"), 0, true);
    ui.set_lifecycle(Lifecycle::new(&App::default()));
    let window = ui.window().clone();
    ui.start_worker(Settings {
        path: socket.to_owned(),
        client: "nuncio-desktop/smoke".to_owned(),
        timeout: Duration::from_secs(5),
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
