//! The window built for real on GTK, without a daemon and without showing it: the chat list and
//! the transcript are drawn from the model, rows carry their accessible roles, opening a chat and
//! sending go through the controller, and Enter-to-send empties the composer.
//!
//! GTK must run on the process's main thread (macOS enforces it), so this test has its own `main`
//! (`harness = false`). Where GTK cannot start (no display), it says so and passes: the logic it
//! drives is covered by the unit tests.

use adw::prelude::*;
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
    the_palette_opens_on_f1_and_jumps_to_a_chat();
    the_settings_write_the_file_and_apply_at_once();
    pictures_show_their_thumbnail_at_once_and_the_full_one_on_a_click();
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
    for wanted in ["Settings", "Quit", "Chat: Ana", "Chat: Ben"] {
        check(
            all.iter().any(|t| t == wanted),
            &format!("the palette lists {wanted}: {all:?}"),
        );
    }
    check(
        !all.iter().any(|t| t.starts_with("Send")),
        "the palette has no send",
    );
    ui.palette_type("BEN");
    check(
        ui.palette_titles().first().map(String::as_str) == Some("Chat: Ben"),
        "typing filters the palette, ignoring case",
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
    check(ui.draft() == "half a thought", "the draft stays unsent");

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
    ui.palette_escape();
    iterate();
    check(!ui.palette_open(), "Escape closes the palette");
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
