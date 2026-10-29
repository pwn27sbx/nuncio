//! Inline pictures (T15 Part 2.5, slices 2–3): an image or sticker message with a size gets a box
//! of fixed height before anything is ready, its thumbnail is drawn in it (cut at the top or the
//! bottom when scrolled), `o` fetches the full picture into the same box, and nothing is ever
//! downloaded without a key. All through `App`'s public interface, a `TestBackend` buffer and the
//! deterministic Halfblocks protocol, plus `Daemon` against a scripted fake daemon.

use std::io::{BufRead, BufReader, Cursor, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine;
use nuncio_client::{
    Chat, ClientFrame, DaemonFrame, Media, Message, Palette, Rgb, Session, Snapshot,
};
use nuncio_tui::images::{
    CACHE_ENTRIES, Graphics, IMAGE_ROWS, Plan, ProtocolChoice, box_size, parse_protocol, plan,
    query_left_stdin_unsafe, zellij_protocol,
};
use nuncio_tui::{Action, App, Daemon, Pane, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui_image::FontSize;
use ratatui_image::picker::ProtocolType;

const ADA: &str = "15551230001@s.whatsapp.net";
const GRACE: &str = "15551230002@s.whatsapp.net";
const WAIT: Duration = Duration::from_secs(10);

// --- fixtures --------------------------------------------------------------------------------

fn message(id: &str, text: &str, from_me: bool, minute: u32) -> Message {
    Message {
        id: id.to_owned(),
        kind: "text".to_owned(),
        text: text.to_owned(),
        from_me,
        timestamp: format!("2026-10-18T17:{:02}:00Z", minute % 60),
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

fn snapshot() -> Snapshot {
    Snapshot {
        daemon: "nunciod/0.1.1".to_owned(),
        linked: true,
        session: Session::Linked,
        unread: 0,
        chats: vec![chat(ADA, "Ada Lovelace"), chat(GRACE, "Grace Hopper")],
        stop_reason: None,
    }
}

/// An app that draws pictures with the deterministic Halfblocks protocol (font 10x20).
fn pictures_app() -> App {
    App::new(snapshot(), Source::Daemon, Palette::default()).with_graphics(Graphics::halfblocks())
}

/// A solid `colour` picture `width` x `height` pixels, encoded as `format`.
fn picture(colour: [u8; 3], width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb(colour));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, format)
        .expect("encode a test picture");
    bytes.into_inner()
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

const RED: [u8; 3] = [220, 20, 20];
const BLUE: [u8; 3] = [20, 20, 220];
const GREEN: [u8; 3] = [20, 200, 20];

/// A photo of 320x240 pixels whose embedded thumbnail is a small red JPEG.
fn photo(id: &str, minute: u32) -> Message {
    let mut photo = message(id, "the view", false, minute);
    photo.kind = "image".to_owned();
    photo.media = Some(Media {
        mime: "image/jpeg".to_owned(),
        size: 40_960,
        file_name: String::new(),
    });
    photo.thumb = Some(base64(&picture(RED, 32, 24, image::ImageFormat::Jpeg)));
    photo.thumb_mime = Some("image/jpeg".to_owned());
    photo.width = Some(320);
    photo.height = Some(240);
    photo
}

/// A 512x512 sticker; its thumbnail, when given, is a small green PNG.
fn sticker(id: &str, minute: u32, thumb: bool, animated: bool) -> Message {
    let mut sticker = message(id, "", false, minute);
    sticker.kind = "sticker".to_owned();
    sticker.media = Some(Media {
        mime: "image/webp".to_owned(),
        size: 20_480,
        file_name: String::new(),
    });
    if thumb {
        sticker.thumb = Some(base64(&picture(GREEN, 32, 32, image::ImageFormat::Png)));
        sticker.thumb_mime = Some("image/png".to_owned());
    }
    sticker.width = Some(512);
    sticker.height = Some(512);
    sticker.animated = animated;
    sticker
}

fn texts(prefix: &str, count: u32, first_minute: u32) -> Vec<Message> {
    (0..count)
        .map(|n| {
            message(
                &format!("{prefix}{n}"),
                &format!("{prefix} line {n}"),
                false,
                first_minute + n,
            )
        })
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Open Ada's chat with `oldest_first` loaded, in the conversation pane.
fn open_ada(app: &mut App, oldest_first: Vec<Message>) {
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Request(ClientFrame::messages(ADA, 200)))
    );
    let mut messages = oldest_first;
    messages.reverse();
    app.apply(&DaemonFrame::Messages {
        v: 1,
        jid: ADA.to_owned(),
        messages,
    });
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.pane(), Pane::Messages);
}

/// A temporary file holding `bytes`, removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ntui-img-{}-{n}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write a test picture");
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// --- the screen ------------------------------------------------------------------------------

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    terminal.backend().buffer().clone()
}

/// At 80x24 the conversation's message rows are y 2..21 (under the header rule, over the
/// composer rule), columns 33..79 (the last column is kept free).
const ROWS: std::ops::Range<u16> = 2..21;
const COLUMNS: std::ops::Range<u16> = 33..79;

fn row_text(buffer: &Buffer, y: u16) -> String {
    COLUMNS
        .map(|x| buffer.cell((x, y)).map_or(" ", |c| c.symbol()).to_owned())
        .collect()
}

fn is(colour: [u8; 3]) -> impl Fn(Color) -> bool {
    move |c| match c {
        Color::Rgb(r, g, b) => [r, g, b]
            .iter()
            .zip(colour)
            .all(|(&got, want)| got.abs_diff(want) < 40),
        _ => false,
    }
}

/// The message rows whose first picture column (x 33) is drawn in `colour`.
fn rows_in(buffer: &Buffer, colour: [u8; 3]) -> Vec<u16> {
    let matches = is(colour);
    ROWS.filter(|&y| {
        buffer
            .cell((COLUMNS.start, y))
            .is_some_and(|c| matches(c.bg) || matches(c.fg))
    })
    .collect()
}

/// The row whose text contains `needle`.
fn row_of(buffer: &Buffer, needle: &str) -> Option<u16> {
    ROWS.clone().find(|&y| row_text(buffer, y).contains(needle))
}

fn screen(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).map_or("", |c| c.symbol()))
                .collect::<String>()
                + "\n"
        })
        .collect()
}

// --- the box ---------------------------------------------------------------------------------

#[test]
fn a_box_is_reserved_before_the_thumbnail_is_ready_and_the_thumbnail_fills_it() {
    let mut app = pictures_app();
    open_ada(
        &mut app,
        vec![photo("IMG1", 1), message("T1", "after the photo", false, 2)],
    );
    let before = draw(&mut app, 80, 24);
    let caption = row_of(&before, "[image] the view").expect("the caption line");
    let after_row = row_of(&before, "Ada Lovelace · 17:02").expect("the next message");
    // 320x240 at a 10x20 font in 46 columns: 32 columns, 12 rows, right under the caption.
    assert_eq!(
        after_row - caption - 1,
        12,
        "the box is reserved at once:\n{}",
        screen(&before)
    );
    assert!(
        rows_in(&before, RED).is_empty(),
        "nothing drawn before it is ready"
    );

    assert!(app.wait_for_images(WAIT), "the thumbnail is encoded");
    let ready = draw(&mut app, 80, 24);
    assert_eq!(
        row_of(&ready, "Ada Lovelace · 17:02"),
        Some(after_row),
        "nothing moves when the thumbnail is drawn"
    );
    assert_eq!(
        rows_in(&ready, RED),
        ((caption + 1)..after_row).collect::<Vec<_>>(),
        "the thumbnail fills the box:\n{}",
        screen(&ready)
    );
}

#[test]
fn a_picture_with_no_size_gets_no_box_and_no_thumbnail() {
    let mut app = pictures_app();
    let mut no_size = photo("IMG1", 1);
    no_size.width = None;
    no_size.height = None;
    open_ada(
        &mut app,
        vec![no_size, message("T1", "after the photo", false, 2)],
    );
    let buffer = draw(&mut app, 80, 24);
    let caption = row_of(&buffer, "[image] the view").expect("the caption line");
    assert_eq!(row_of(&buffer, "Ada Lovelace · 17:02"), Some(caption + 1));
}

#[test]
fn a_partially_visible_picture_is_cut_at_the_top_and_at_the_bottom() {
    // Cut at the top: ten one-row messages after the photo scroll its first rows away.
    let mut app = pictures_app();
    let mut messages = vec![photo("IMG1", 1)];
    messages.extend(texts("after", 10, 10));
    open_ada(&mut app, messages);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let top_cut = draw(&mut app, 80, 24);
    // 2 + 12 + 10 rows in 19: the day label, the caption line and 3 picture rows are above the
    // view.
    assert_eq!(
        rows_in(&top_cut, RED),
        (2..11).collect::<Vec<_>>(),
        "{}",
        screen(&top_cut)
    );
    assert_eq!(row_of(&top_cut, "after line 0"), Some(11));

    // Cut at the bottom: the same photo after eight messages, scrolled to the oldest.
    let mut app = pictures_app();
    let mut messages = texts("before", 8, 1);
    messages.push(photo("IMG1", 9));
    open_ada(&mut app, messages);
    app.handle_key(key(KeyCode::Char('g')));
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let bottom_cut = draw(&mut app, 80, 24);
    // The start marker costs one row: 8 picture rows fit before the composer.
    assert_eq!(row_of(&bottom_cut, "Beginning of conversation"), Some(2));
    assert_eq!(row_of(&bottom_cut, "before line 0"), Some(4));
    assert_eq!(
        rows_in(&bottom_cut, RED),
        (13..21).collect::<Vec<_>>(),
        "{}",
        screen(&bottom_cut)
    );
}

#[test]
fn a_corrupt_thumbnail_keeps_the_box_with_a_label_and_never_panics() {
    let mut app = pictures_app();
    let mut not_base64 = photo("IMG1", 1);
    not_base64.thumb = Some("this is not base64 !!".to_owned());
    let mut not_a_picture = sticker("STK1", 2, true, false);
    not_a_picture.thumb = Some(base64(b"\x89PNG\r\n\x1a\n then garbage"));
    open_ada(
        &mut app,
        vec![
            not_base64,
            not_a_picture,
            message("T1", "still here", false, 3),
        ],
    );
    draw(&mut app, 120, 40);
    assert!(app.wait_for_images(WAIT));
    let buffer = draw(&mut app, 120, 40);
    let text = screen(&buffer);
    assert!(text.contains("still here"), "{text}");
    let image_box = text
        .lines()
        .skip_while(|line| !line.contains("[image] the view"))
        .nth(1)
        .unwrap_or_default();
    assert!(
        image_box.contains("[image]"),
        "the box keeps a label:\n{text}"
    );
    // The sticker's caption is on its message line; its box starts on the next row.
    let sticker_box = text
        .lines()
        .skip_while(|line| !line.contains("Ada Lovelace · 17:02"))
        .nth(1)
        .unwrap_or_default();
    assert!(
        sticker_box.contains("[sticker]"),
        "the box keeps a label:\n{text}"
    );
    // The boxes keep their height: 12 rows each, so the last message is far below.
    let rows = text.lines().collect::<Vec<_>>();
    let caption = rows.iter().position(|l| l.contains("[image] the view"));
    let sticker_header = rows.iter().position(|l| l.contains("Ada Lovelace · 17:02"));
    assert_eq!(
        sticker_header.zip(caption).map(|(s, c)| s - c - 1),
        Some(12)
    );
}

// --- the cache -------------------------------------------------------------------------------

#[test]
fn a_redraw_reuses_the_encoded_picture_and_a_resize_encodes_it_again() {
    let mut app = pictures_app();
    open_ada(&mut app, vec![photo("IMG1", 1)]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    // An encode already running when the width changes cannot be cancelled; the queue behind it
    // is dropped, so at most the newest width is added. Under a slow CI the in-flight one lands
    // in the count too, which is why this is a bound rather than an exact number.
    assert!(
        app.image_encodes() <= 2,
        "only the running encode and the newest width, ran {}",
        app.image_encodes()
    );
    for _ in 0..3 {
        draw(&mut app, 80, 24);
        assert!(app.wait_for_images(WAIT));
    }
    assert_eq!(app.image_encodes(), 1, "a redraw is a cache hit");
    assert_eq!(app.image_cache_len(), 1);

    draw(&mut app, 100, 24);
    assert!(app.wait_for_images(WAIT));
    assert_eq!(app.image_encodes(), 2, "another width encodes it again");
    assert_eq!(
        app.image_cache_len(),
        1,
        "the old width's entry was dropped"
    );
}

#[test]
fn the_cache_never_holds_more_entries_than_its_bound() {
    let mut app = pictures_app();
    let photos: Vec<Message> = (0..80).map(|n| photo(&format!("IMG{n}"), n)).collect();
    open_ada(&mut app, photos);
    app.handle_key(key(KeyCode::Char('g')));
    for _ in 0..120 {
        draw(&mut app, 80, 24);
        assert!(app.wait_for_images(WAIT));
        assert!(app.image_cache_len() <= CACHE_ENTRIES);
        app.handle_key(key(KeyCode::PageDown));
    }
    assert!(
        app.image_encodes() > CACHE_ENTRIES as u64,
        "every photo was drawn"
    );
}

// --- the full picture on demand ------------------------------------------------------------

static NEXT_SOCKET: AtomicU32 = AtomicU32::new(0);

fn socket_path() -> PathBuf {
    let n = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from(format!("/tmp/ntimg-{}-{n}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn frame_line(value: serde_json::Value) -> String {
    format!("{value}\n")
}

/// Removes the socket file when dropped, so a failed test leaves nothing in /tmp.
struct Socket(PathBuf);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A fake daemon: both handshakes and the subscribe, then every request is answered by `answer`
/// until the client hangs up; returns the ops it was asked for.
fn fake_daemon(
    path: &Path,
    mut answer: impl FnMut(serde_json::Value) -> serde_json::Value + Send + 'static,
) -> thread::JoinHandle<Vec<String>> {
    let listener = UnixListener::bind(path).expect("bind");
    thread::spawn(move || {
        let hello_ok = frame_line(serde_json::json!(
            {"v":1,"op":"hello_ok","daemon":"fake","session":"linked","unread":0}
        ));
        let mut line = String::new();
        let (requests, _) = listener.accept().expect("request connection");
        let mut writer = requests.try_clone().expect("clone");
        let mut reader = BufReader::new(requests);
        reader.read_line(&mut line).expect("hello");
        writer.write_all(hello_ok.as_bytes()).expect("w");
        let (events, _) = listener.accept().expect("event connection");
        let mut events_writer = events.try_clone().expect("clone");
        let mut events_reader = BufReader::new(events);
        line.clear();
        events_reader.read_line(&mut line).expect("hello");
        events_writer.write_all(hello_ok.as_bytes()).expect("w");
        line.clear();
        events_reader.read_line(&mut line).expect("subscribe");
        events_writer
            .write_all(frame_line(serde_json::json!({"v":1,"op":"subscribed"})).as_bytes())
            .expect("w");
        let mut ops = Vec::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return ops;
            }
            let request: serde_json::Value = serde_json::from_str(&line).expect("json");
            ops.push(request["op"].as_str().unwrap_or_default().to_owned());
            writer
                .write_all(frame_line(answer(request)).as_bytes())
                .expect("w");
        }
    })
}

type Opened = Arc<Mutex<Vec<PathBuf>>>;

fn connect(path: &Path) -> (Daemon, Opened) {
    let opened: Opened = Arc::default();
    let record = opened.clone();
    let daemon = Daemon::connect(path, "nuncio-tui/test")
        .expect("connect")
        .with_opener(move |file: &Path| {
            record.lock().expect("lock").push(file.to_owned());
            Ok(())
        });
    (daemon, opened)
}

#[test]
fn o_on_a_photo_fetches_the_full_picture_into_the_same_box_and_opens_nothing() {
    let full = TempFile::new(
        "full.png",
        &picture(BLUE, 320, 240, image::ImageFormat::Png),
    );
    let served = full.0.to_string_lossy().into_owned();
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, move |request| {
        assert_eq!(request["op"], "download_media");
        assert_eq!(request["id"], "IMG1");
        serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"IMG1",
            "path":served,"mime":"image/png"})
    });
    let (mut daemon, opened) = connect(&path);
    let mut app = pictures_app();
    open_ada(
        &mut app,
        vec![photo("IMG1", 1), message("T1", "after the photo", false, 2)],
    );
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let thumb = draw(&mut app, 80, 24);
    let red = rows_in(&thumb, RED);
    assert_eq!(red.len(), 12);
    let next = row_of(&thumb, "Ada Lovelace · 17:02");

    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the photo");
    };
    assert_eq!(frame, ClientFrame::download_media(ADA, "IMG1"));
    let downloading = draw(&mut app, 80, 24);
    assert!(
        row_text(&downloading, red[0] - 1).contains("Downloading…"),
        "{}",
        screen(&downloading)
    );
    assert_eq!(
        rows_in(&downloading, RED),
        red,
        "the thumbnail stays meanwhile"
    );

    daemon.perform(&mut app, &frame).expect("perform");
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let replaced = draw(&mut app, 80, 24);
    assert_eq!(
        rows_in(&replaced, BLUE),
        red,
        "the full picture is in the same box:\n{}",
        screen(&replaced)
    );
    assert!(rows_in(&replaced, RED).is_empty());
    assert_eq!(
        row_of(&replaced, "Ada Lovelace · 17:02"),
        next,
        "nothing moved"
    );
    assert!(!screen(&replaced).contains("Downloading…"));
    // Redraws reuse the encoded full picture.
    let encodes = app.image_encodes();
    for _ in 0..3 {
        draw(&mut app, 80, 24);
        assert!(app.wait_for_images(WAIT));
    }
    assert_eq!(app.image_encodes(), encodes, "a redraw is a cache hit");
    assert!(
        opened.lock().expect("lock").is_empty(),
        "o shows it, it opens nothing"
    );
    drop(daemon);
    assert_eq!(server.join().expect("fake daemon"), ["download_media"]);
}

#[test]
fn a_refused_or_unreadable_full_picture_shows_the_error_and_keeps_the_thumbnail() {
    let broken = TempFile::new("broken.jpg", b"\xff\xd8\xff not really a jpeg");
    let served = broken.0.to_string_lossy().into_owned();
    let path = socket_path();
    let _socket = Socket(path.clone());
    let mut answers = 0;
    let server = fake_daemon(&path, move |_| {
        answers += 1;
        if answers == 1 {
            serde_json::json!({"v":1,"op":"error","code":"internal",
                "message":"the WhatsApp session is offline, so the media was not downloaded"})
        } else {
            serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"IMG1",
                "path":served,"mime":"image/jpeg"})
        }
    });
    let (mut daemon, opened) = connect(&path);
    let mut app = pictures_app();
    open_ada(&mut app, vec![photo("IMG1", 1)]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    app.handle_key(key(KeyCode::Char('k')));
    for attempt in ["so the media was not", "cannot show the picture"] {
        let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
            panic!("o fetches the photo");
        };
        daemon.perform(&mut app, &frame).expect("perform");
        draw(&mut app, 80, 24);
        assert!(app.wait_for_images(WAIT));
        let buffer = draw(&mut app, 80, 24);
        let footer = (0..80)
            .map(|x| buffer.cell((x, 23)).map_or(" ", |c| c.symbol()))
            .collect::<String>();
        assert!(footer.contains(attempt), "{footer}");
        assert_eq!(rows_in(&buffer, RED).len(), 12, "the thumbnail is kept");
        assert!(!screen(&buffer).contains("Downloading…"));
    }
    assert!(opened.lock().expect("lock").is_empty());
    drop(daemon);
    assert_eq!(
        server.join().expect("fake daemon"),
        ["download_media", "download_media"]
    );
}

#[test]
fn nothing_is_downloaded_without_a_key() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, |request| {
        panic!("nothing is asked for without a key, got {request}");
    });
    let (mut daemon, opened) = connect(&path);
    let mut app = pictures_app();
    open_ada(
        &mut app,
        vec![
            photo("IMG1", 1),
            sticker("STK1", 2, false, false),
            sticker("STK2", 3, true, true),
        ],
    );
    for (width, height) in [(80, 24), (240, 60), (80, 24)] {
        draw(&mut app, width, height);
        assert!(app.wait_for_images(WAIT));
        daemon.drain_events(&mut app).expect("drain");
    }
    // Moving through the messages selects them; it never fetches.
    for code in [KeyCode::Char('k'), KeyCode::Char('k'), KeyCode::Char('j')] {
        assert_eq!(app.handle_key(key(code)), None);
    }
    drop(daemon);
    assert_eq!(server.join().expect("fake daemon"), Vec::<String>::new());
    assert!(opened.lock().expect("lock").is_empty());
}

#[test]
fn shift_o_opens_a_picture_externally_and_o_still_opens_a_document() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, |request| {
        let id = request["id"].as_str().unwrap_or_default().to_owned();
        serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":id,
            "path":format!("/nonexistent/{id}"),"mime":"application/pdf"})
    });
    let (mut daemon, opened) = connect(&path);
    let mut app = pictures_app();
    let mut document = message("DOC1", "invoice", false, 2);
    document.kind = "document".to_owned();
    document.media = Some(Media {
        mime: "application/pdf".to_owned(),
        size: 100,
        file_name: "invoice.pdf".to_owned(),
    });
    open_ada(&mut app, vec![photo("IMG1", 1), document]);
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the document");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('O'))) else {
        panic!("O fetches the photo to open it");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    assert_eq!(
        *opened.lock().expect("lock"),
        [
            PathBuf::from("/nonexistent/DOC1"),
            PathBuf::from("/nonexistent/IMG1")
        ]
    );
    drop(daemon);
    assert_eq!(
        server.join().expect("fake daemon"),
        ["download_media", "download_media"]
    );
}

// --- stickers --------------------------------------------------------------------------------

#[test]
fn a_sticker_shows_its_first_frame_and_an_animated_one_a_marker() {
    let mut app = pictures_app();
    open_ada(
        &mut app,
        vec![
            sticker("STK1", 1, true, false),
            sticker("STK2", 2, true, true),
        ],
    );
    draw(&mut app, 120, 60);
    assert!(app.wait_for_images(WAIT));
    let buffer = draw(&mut app, 120, 60);
    let text = screen(&buffer);
    let lines: Vec<&str> = text.lines().collect();
    let still = lines
        .iter()
        .position(|l| l.contains("Ada Lovelace · 17:01"))
        .expect("the first sticker");
    let moving = lines
        .iter()
        .position(|l| l.contains("Ada Lovelace · 17:02"))
        .expect("the second sticker");
    assert!(
        lines[still].contains("[sticker]") && !lines[still].contains('▶'),
        "{text}"
    );
    assert!(
        lines[moving].contains("[sticker] ▶"),
        "an animated sticker is marked:\n{text}"
    );
    // Both are drawn from their PNG thumbnails, green, in 12-row boxes.
    let green = is(GREEN);
    let drawn = |from: usize| {
        (from + 1..from + 1 + usize::from(IMAGE_ROWS))
            .filter(|&y| {
                (0..buffer.area.width).any(|x| {
                    buffer
                        .cell((x, y as u16))
                        .is_some_and(|c| green(c.bg) || green(c.fg))
                })
            })
            .count()
    };
    assert_eq!(drawn(still), usize::from(IMAGE_ROWS), "{text}");
    assert_eq!(drawn(moving), usize::from(IMAGE_ROWS), "{text}");
}

#[test]
fn a_sticker_with_no_thumbnail_shows_its_label_until_o_fetches_it() {
    let webp = TempFile::new(
        "sticker.webp",
        &picture(GREEN, 512, 512, image::ImageFormat::WebP),
    );
    let served = webp.0.to_string_lossy().into_owned();
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, move |_| {
        serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"STK1",
            "path":served,"mime":"image/webp"})
    });
    let (mut daemon, opened) = connect(&path);
    let mut app = pictures_app();
    open_ada(&mut app, vec![sticker("STK1", 1, false, false)]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let label = draw(&mut app, 80, 24);
    assert!(screen(&label).contains("[sticker]"));
    assert!(rows_in(&label, GREEN).is_empty());
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the sticker");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let drawn = draw(&mut app, 80, 24);
    assert_eq!(
        rows_in(&drawn, GREEN).len(),
        usize::from(IMAGE_ROWS),
        "the WebP is drawn in the box:\n{}",
        screen(&drawn)
    );
    assert!(opened.lock().expect("lock").is_empty());
    drop(daemon);
    server.join().expect("fake daemon");
}

/// A 32x32 PNG with a disc of `colour` in the middle and transparent corners, like a real sticker.
fn transparent_disc(colour: [u8; 3]) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(32, 32, |x, y| {
        let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
        if dx * dx + dy * dy < 10.0 * 10.0 {
            image::Rgba([colour[0], colour[1], colour[2], 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode a test picture");
    bytes.into_inner()
}

fn palette_on(background: Rgb) -> Palette {
    Palette {
        background,
        ..Palette::default()
    }
}

#[test]
fn half_blocks_draw_a_transparent_corner_in_the_conversation_background() {
    let first = Rgb::new(90, 60, 120);
    let mut app = App::new(snapshot(), Source::Daemon, palette_on(first))
        .with_graphics(Graphics::halfblocks());
    let mut disc = sticker("STK1", 1, true, false);
    disc.thumb = Some(base64(&transparent_disc(GREEN)));
    open_ada(&mut app, vec![disc]);
    let corner = |app: &mut App| {
        draw(app, 80, 24);
        assert!(app.wait_for_images(WAIT));
        let buffer = draw(app, 80, 24);
        let top = row_of(&buffer, "[sticker]").expect("the caption") + 1;
        let cell = buffer
            .cell((COLUMNS.start, top))
            .expect("the corner")
            .clone();
        let green = is(GREEN);
        assert!(
            buffer.content().iter().any(|c| green(c.fg) || green(c.bg)),
            "the disc is drawn:\n{}",
            screen(&buffer)
        );
        (cell.fg, cell.bg)
    };
    let pane = |c: Rgb| Color::Rgb(c.r, c.g, c.b);
    assert_eq!(corner(&mut app), (pane(first), pane(first)));
    // A theme switch redraws the corner in the new background.
    let second = Rgb::new(200, 210, 190);
    app.set_palette(palette_on(second));
    assert_eq!(corner(&mut app), (pane(second), pane(second)));
}

#[test]
fn half_blocks_draw_the_margin_a_picture_leaves_in_its_box_in_the_conversation_background() {
    let background = Rgb::new(90, 60, 120);
    let mut app = App::new(snapshot(), Source::Daemon, palette_on(background))
        .with_graphics(Graphics::halfblocks());
    // An odd aspect ratio: the picture, scaled to fit its box, leaves a margin of pixels.
    let mut wide = photo("IMG1", 1);
    wide.thumb = Some(base64(&picture(RED, 37, 23, image::ImageFormat::Png)));
    wide.width = Some(333);
    wide.height = Some(207);
    open_ada(&mut app, vec![wide]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let buffer = draw(&mut app, 80, 24);
    // Red and the background both have some blue; a margin blended with black would have less.
    let darkened = |c: Color| matches!(c, Color::Rgb(_, _, b) if b < RED[2]);
    let dark: Vec<_> = buffer
        .content()
        .iter()
        .filter(|c| darkened(c.fg) || darkened(c.bg))
        .map(|c| (c.fg, c.bg))
        .collect();
    assert!(
        dark.is_empty(),
        "darkened cells {dark:?}:\n{}",
        screen(&buffer)
    );
    assert!(!rows_in(&buffer, RED).is_empty(), "the photo is drawn");
}

/// How many times `needle` shows on the screen.
fn count(buffer: &Buffer, needle: &str) -> usize {
    screen(buffer).matches(needle).count()
}

#[test]
fn a_picture_with_no_thumbnail_says_its_label_once_and_its_box_stays_empty() {
    let mut app = pictures_app();
    let mut bare_photo = photo("IMG1", 3);
    bare_photo.thumb = None;
    bare_photo.thumb_mime = None;
    open_ada(
        &mut app,
        vec![
            sticker("STK1", 1, false, false),
            sticker("STK2", 2, false, true),
            bare_photo,
        ],
    );
    draw(&mut app, 120, 60);
    assert!(app.wait_for_images(WAIT));
    let buffer = draw(&mut app, 120, 60);
    let text = screen(&buffer);
    assert_eq!(count(&buffer, "[sticker]"), 2, "one per sticker:\n{text}");
    assert_eq!(count(&buffer, "[sticker] ▶"), 1, "{text}");
    assert_eq!(count(&buffer, "[image]"), 1, "{text}");
    // Each label is the caption on its message line; the 12 rows below it are blank.
    let lines: Vec<&str> = text.lines().collect();
    for minute in 1..=3 {
        let header = lines
            .iter()
            .position(|l| l.contains(&format!("Ada Lovelace · 17:0{minute}")))
            .expect("the header");
        assert!(
            lines[header].contains("[sticker]") || lines[header].contains("[image]"),
            "{text}"
        );
        for row in &lines[header + 1..header + 1 + usize::from(IMAGE_ROWS)] {
            let inside = row.split('│').nth(1).unwrap_or_default();
            assert!(inside.trim().is_empty(), "the box is empty:\n{text}");
        }
    }
}

// --- frames ----------------------------------------------------------------------------------

#[test]
fn frames_with_pictures_stay_intact_at_eighty_by_twenty_four_and_two_forty_by_sixty() {
    for (width, height) in [(80u16, 24u16), (240, 60)] {
        let mut app = pictures_app();
        let mut messages = texts("before", 3, 1);
        messages.push(photo("IMG1", 10));
        messages.push(sticker("STK1", 11, true, true));
        messages.extend(texts("after", 2, 20));
        open_ada(&mut app, messages);
        for scroll in [KeyCode::Char('G'), KeyCode::PageUp, KeyCode::Char('g')] {
            app.handle_key(key(scroll));
            draw(&mut app, width, height);
            assert!(app.wait_for_images(WAIT));
            let buffer = draw(&mut app, width, height);
            let text = screen(&buffer);
            let rows: Vec<&str> = text.lines().collect();
            assert_eq!(rows.len(), usize::from(height));
            assert!(rows[0].starts_with("nuncio"), "{text}");
            assert!(
                rows[usize::from(height) - 1].contains("? all keys"),
                "{text}"
            );
            // On every body row the line between the panes stays in its column and the
            // conversation's last column stays free: no picture spilled over either.
            let split = (0..width)
                .find(|&x| buffer[(x, 1)].symbol() == "┬")
                .expect("the line between the panes");
            for y in 2..height - 2 {
                assert!(
                    ["│", "├"].contains(&buffer[(split, y)].symbol())
                        && [" ", "─"].contains(&buffer[(width - 1, y)].symbol()),
                    "row {y} lost its frame at {width}x{height}:\n{text}"
                );
            }
        }
    }
}

// --- capability detection ------------------------------------------------------------------

#[test]
fn the_protocol_override_is_parsed() {
    for (value, choice) in [
        ("kitty", ProtocolChoice::Kitty),
        ("sixel", ProtocolChoice::Sixel),
        ("iterm2", ProtocolChoice::Iterm2),
        ("halfblocks", ProtocolChoice::Halfblocks),
        ("off", ProtocolChoice::Off),
        (" Kitty\n", ProtocolChoice::Kitty),
        ("OFF", ProtocolChoice::Off),
    ] {
        assert_eq!(parse_protocol(value), Ok(choice), "{value:?}");
    }
    for value in ["", "chafa", "kitty,sixel", "1"] {
        assert!(parse_protocol(value).is_err(), "{value:?}");
    }
}

#[test]
fn the_override_wins_and_only_a_terminal_is_queried() {
    // The override, whatever else holds.
    for (zellij, terminal) in [(false, true), (true, true), (false, false)] {
        let (chosen, warning) = plan(Some("sixel"), zellij, terminal);
        assert_eq!(
            chosen,
            Plan::Fixed {
                choice: ProtocolChoice::Sixel,
                why: "NUNCIO_IMAGE_PROTOCOL"
            }
        );
        assert_eq!(warning, None);
    }
    // No override: a terminal is queried, zellij or not.
    assert_eq!(plan(None, false, true).0, Plan::Query { zellij: false });
    assert_eq!(plan(None, true, true).0, Plan::Query { zellij: true });
    // stdin or stdout is not a terminal: never queried (11.1 would leave a reader on stdin).
    assert!(matches!(
        plan(None, false, false).0,
        Plan::Fixed {
            choice: ProtocolChoice::Halfblocks,
            ..
        }
    ));
    // A value that is not one of the five is said and ignored.
    let (chosen, warning) = plan(Some("chafa"), false, true);
    assert_eq!(chosen, Plan::Query { zellij: false });
    assert!(warning.is_some_and(|w| w.contains("NUNCIO_IMAGE_PROTOCOL")));
}

#[test]
fn under_zellij_only_sixel_or_halfblocks_are_used() {
    // zellij rejects kitty's Unicode placeholders.
    assert_eq!(
        zellij_protocol(ProtocolType::Kitty, true),
        ProtocolType::Sixel
    );
    assert_eq!(
        zellij_protocol(ProtocolType::Kitty, false),
        ProtocolType::Halfblocks
    );
    assert_eq!(
        zellij_protocol(ProtocolType::Sixel, true),
        ProtocolType::Sixel
    );
    assert_eq!(
        zellij_protocol(ProtocolType::Iterm2, false),
        ProtocolType::Halfblocks
    );
    assert_eq!(
        zellij_protocol(ProtocolType::Halfblocks, false),
        ProtocolType::Halfblocks
    );
}

#[test]
fn a_query_that_ran_out_its_timeout_without_an_answer_leaves_stdin_unsafe() {
    let timeout = Duration::from_millis(1000);
    assert!(!query_left_stdin_unsafe(
        Duration::from_millis(40),
        timeout,
        false
    ));
    assert!(!query_left_stdin_unsafe(
        Duration::from_millis(40),
        timeout,
        true
    ));
    assert!(query_left_stdin_unsafe(timeout, timeout, false));
    assert!(query_left_stdin_unsafe(
        Duration::from_millis(1500),
        timeout,
        false
    ));
    // An answer means the reader finished, however slow it was.
    assert!(!query_left_stdin_unsafe(
        Duration::from_millis(1500),
        timeout,
        true
    ));
}

#[test]
fn the_box_size_follows_the_picture_and_absurd_sizes_are_clamped() {
    let font = FontSize::new(10, 20);
    assert_eq!(box_size(320, 240, 46, font), Size::new(32, 12));
    // Wide: the columns cap it; tall: the rows cap it and the columns shrink with them.
    assert_eq!(box_size(4000, 1000, 46, font), Size::new(46, 6));
    assert_eq!(box_size(1000, 4000, 46, font), Size::new(6, 12));
    // Never wider than the column, never taller than IMAGE_ROWS, never empty.
    for (width, height) in [
        (1, 16_384),
        (16_384, 1),
        (1, 1),
        (u32::MAX, u32::MAX),
        (u32::MAX, 1),
        (0, 0),
    ] {
        let size = box_size(width, height, 46, font);
        assert!(
            (1..=46).contains(&size.width) && (1..=IMAGE_ROWS).contains(&size.height),
            "{width}x{height}: {size:?}"
        );
    }
    let size = box_size(320, 240, 0, FontSize::new(0, 0));
    assert!(size.width >= 1 && size.height >= 1);
}

// --- kitty placeholders across scrolling ----------------------------------------------------

/// The kitty image ids (the cell's foreground colour) of the placeholder cells on each message row,
/// as a terminal shows them after every diff was applied.
fn kitty_ids(buffer: &Buffer, rows: std::ops::Range<u16>) -> Vec<(u16, Vec<Color>)> {
    rows.map(|y| {
        let mut ids: Vec<Color> = COLUMNS
            .filter_map(|x| buffer.cell((x, y)))
            .filter(|c| c.symbol().contains('\u{10EEEE}'))
            .map(|c| c.fg)
            .collect();
        ids.dedup();
        (y, ids)
    })
    .collect()
}

#[test]
fn kitty_placeholders_follow_their_own_picture_across_a_scroll() {
    #[allow(deprecated)]
    let mut picker = ratatui_image::picker::Picker::from_fontsize(FontSize::new(10, 20));
    picker.set_protocol_type(ProtocolType::Kitty);
    let mut app = App::new(snapshot(), Source::Daemon, Palette::default())
        .with_graphics(Graphics::new(picker, "test"));
    let mut messages = vec![photo("IMG1", 1)];
    messages.push(sticker("STK1", 2, true, false));
    messages.push(sticker("STK2", 3, true, true));
    messages.push(sticker("STK3", 4, false, false));
    open_ada(&mut app, messages);
    // One terminal for every frame, so only the cells ratatui's diff rewrites change, as on a
    // real terminal.
    let mut terminal = Terminal::new(TestBackend::new(80, 50)).expect("a test terminal");
    let mut frame = |app: &mut App| {
        terminal.draw(|f| app.draw(f)).expect("draw");
        assert!(app.wait_for_images(WAIT));
        terminal.draw(|f| app.draw(f)).expect("draw");
        let shown = terminal.backend().buffer().clone();
        // What a fresh terminal would show for the same frame.
        let mut fresh = Terminal::new(TestBackend::new(80, 50)).expect("a test terminal");
        fresh.draw(|f| app.draw(f)).expect("draw");
        (shown, fresh.backend().buffer().clone())
    };
    let ids_of = |buffer: &Buffer, header: &str| -> Vec<Color> {
        let Some(start) = (2..46).find(|&y| row_text(buffer, y).contains(header)) else {
            return Vec::new();
        };
        let mut ids: Vec<Color> = kitty_ids(buffer, (start + 2)..46u16.min(start + 14))
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .collect();
        ids.sort_by_key(|c| format!("{c:?}"));
        ids.dedup();
        ids
    };
    let (bottom, _) = frame(&mut app);
    let still = ids_of(&bottom, "Ada Lovelace · 17:02");
    let moving = ids_of(&bottom, "Ada Lovelace · 17:03");
    assert_eq!(still.len(), 1, "{}", screen(&bottom));
    assert_eq!(moving.len(), 1, "{}", screen(&bottom));
    assert_ne!(still, moving, "two stickers, two kitty images");
    assert!(ids_of(&bottom, "Ada Lovelace · 17:04").is_empty());

    for _ in 0..3 {
        app.handle_key(key(KeyCode::PageUp));
        let (shown, fresh) = frame(&mut app);
        assert_eq!(
            screen(&shown),
            screen(&fresh),
            "the terminal shows what was drawn"
        );
        for (header, want) in [
            ("Ada Lovelace · 17:02", &still),
            ("Ada Lovelace · 17:03", &moving),
        ] {
            let got = ids_of(&shown, header);
            assert!(
                got.is_empty() || &got == want,
                "{header}: {got:?}, want {want:?}"
            );
        }
        assert!(
            ids_of(&shown, "Ada Lovelace · 17:04").is_empty(),
            "the sticker without a thumbnail draws no picture"
        );
    }
}

// --- bounds and queueing (verifier B1, A2–A4) ---------------------------------------------

#[test]
fn a_thumbnail_declaring_a_huge_picture_keeps_its_label_without_decoding() {
    let mut app = pictures_app();
    let mut bomb = photo("IMG1", 1);
    let mut jpeg = picture(RED, 16, 16, image::ImageFormat::Jpeg);
    let sof = jpeg
        .windows(2)
        .position(|w| w == [0xff, 0xc0])
        .expect("SOF0");
    jpeg[sof + 5..sof + 7].copy_from_slice(&10_000u16.to_be_bytes());
    jpeg[sof + 7..sof + 9].copy_from_slice(&16_384u16.to_be_bytes());
    bomb.thumb = Some(base64(&jpeg));
    open_ada(&mut app, vec![bomb, message("T1", "still here", false, 2)]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let text = screen(&draw(&mut app, 80, 24));
    let image_box = text
        .lines()
        .skip_while(|l| !l.contains("[image] the view"))
        .nth(1)
        .unwrap_or_default();
    assert!(
        image_box.contains("[image]"),
        "refused, box keeps its label:\n{text}"
    );
}

#[test]
fn a_full_picture_declaring_a_huge_size_is_refused_before_decoding() {
    let mut jpeg = picture(BLUE, 16, 16, image::ImageFormat::Jpeg);
    let sof = jpeg
        .windows(2)
        .position(|w| w == [0xff, 0xc0])
        .expect("SOF0");
    jpeg[sof + 5..sof + 7].copy_from_slice(&16_000u16.to_be_bytes());
    jpeg[sof + 7..sof + 9].copy_from_slice(&16_000u16.to_be_bytes());
    let bomb = TempFile::new("bomb.jpg", &jpeg);
    let served = bomb.0.to_string_lossy().into_owned();
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, move |_| {
        serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"IMG1",
            "path":served,"mime":"image/jpeg"})
    });
    let (mut daemon, _opened) = connect(&path);
    let mut app = pictures_app();
    open_ada(&mut app, vec![photo("IMG1", 1)]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the photo");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    let started = std::time::Instant::now();
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "refused from its header, not decoded: {:?}",
        started.elapsed()
    );
    let buffer = draw(&mut app, 80, 24);
    let footer: String = (0..80)
        .map(|x| buffer.cell((x, 23)).map_or(" ", |c| c.symbol()))
        .collect();
    assert!(footer.contains("cannot show the picture"), "{footer}");
    assert_eq!(rows_in(&buffer, RED).len(), 12, "the thumbnail is kept");
    drop(daemon);
    server.join().expect("fake daemon");
}

#[test]
fn a_resize_drops_the_work_queued_for_the_old_width() {
    let mut app = pictures_app();
    // A larger thumbnail, so each encode takes a while.
    let mut big = photo("IMG1", 1);
    big.thumb = Some(base64(&picture(RED, 640, 480, image::ImageFormat::Png)));
    open_ada(&mut app, vec![big]);
    for width in 60..140 {
        draw(&mut app, width, 24);
    }
    assert!(app.wait_for_images(WAIT));
    assert!(
        app.image_jobs_run() < 40,
        "only the newest widths are encoded, ran {}",
        app.image_jobs_run()
    );
    assert_eq!(app.image_encodes(), 1);
}

#[test]
fn a_full_picture_scrolled_out_of_view_is_not_encoded_again_on_a_resize() {
    let full = TempFile::new(
        "full.png",
        &picture(BLUE, 320, 240, image::ImageFormat::Png),
    );
    let served = full.0.to_string_lossy().into_owned();
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(&path, move |_| {
        serde_json::json!({"v":1,"op":"media_ready","jid":ADA,"id":"IMG1",
            "path":served,"mime":"image/png"})
    });
    let (mut daemon, _opened) = connect(&path);
    let mut app = pictures_app();
    let mut messages = vec![photo("IMG1", 1)];
    messages.extend(texts("after", 20, 10));
    open_ada(&mut app, messages);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char('k')));
    for _ in 0..20 {
        app.handle_key(key(KeyCode::Char('k')));
    }
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the photo");
    };
    daemon.perform(&mut app, &frame).expect("perform");
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let ran = app.image_jobs_run();
    // Scrolled to the newest messages, the photo is out of view: resizes encode nothing.
    app.handle_key(key(KeyCode::Char('G')));
    for width in [81, 82, 83] {
        draw(&mut app, width, 24);
        assert!(app.wait_for_images(WAIT));
    }
    assert_eq!(
        app.image_jobs_run(),
        ran,
        "nothing was out of view to encode"
    );
    drop(daemon);
    server.join().expect("fake daemon");
}

#[test]
fn downloading_never_moves_the_box() {
    let path = socket_path();
    let _socket = Socket(path.clone());
    let server = fake_daemon(
        &path,
        |_| serde_json::json!({"v":1,"op":"error","code":"internal","message":"later"}),
    );
    let (mut daemon, _opened) = connect(&path);
    let mut app = pictures_app();
    // `[image] ` and a caption that fills the 46-column row exactly.
    let mut long = photo("IMG1", 1);
    long.text = "x".repeat(46 - "[image] ".len());
    open_ada(
        &mut app,
        vec![long, message("T1", "after the photo", false, 2)],
    );
    let before = draw(&mut app, 80, 24);
    // Bottom-aligned: a caption that grows a row moves the photo's header up.
    let next = row_of(&before, "Ada Lovelace · 17:01");
    assert!(next.is_some());
    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('k')));
    let Some(Action::Request(frame)) = app.handle_key(key(KeyCode::Char('o'))) else {
        panic!("o fetches the photo");
    };
    let downloading = draw(&mut app, 80, 24);
    assert!(
        screen(&downloading).contains("Downloading…"),
        "{}",
        screen(&downloading)
    );
    assert_eq!(
        row_of(&downloading, "Ada Lovelace · 17:01"),
        next,
        "nothing moved:\n{}",
        screen(&downloading)
    );
    daemon.perform(&mut app, &frame).expect("perform");
    drop(daemon);
    server.join().expect("fake daemon");
}

#[test]
fn the_help_says_what_o_does_with_pictures_on_and_off() {
    let help = |app: &mut App| {
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Char('?')));
        screen(&draw(app, 80, 24))
    };
    let on = help(&mut pictures_app());
    assert!(on.contains("show the picture in full"), "{on}");
    let mut off = App::new(snapshot(), Source::Daemon, Palette::default());
    let off = help(&mut off);
    assert!(!off.contains("show the picture in full"), "{off}");
    assert!(off.contains("o open its media externally"), "{off}");
}

#[test]
fn a_thumbnail_larger_than_a_thumbnail_is_refused_though_a_full_picture_that_size_is_not() {
    // 4000x3000 is a normal photo but no thumbnail: refused as one, from its header.
    let mut jpeg = picture(RED, 16, 16, image::ImageFormat::Jpeg);
    let sof = jpeg
        .windows(2)
        .position(|w| w == [0xff, 0xc0])
        .expect("SOF0");
    jpeg[sof + 5..sof + 7].copy_from_slice(&3_000u16.to_be_bytes());
    jpeg[sof + 7..sof + 9].copy_from_slice(&4_000u16.to_be_bytes());
    let mut app = pictures_app();
    let mut large = photo("IMG1", 1);
    large.thumb = Some(base64(&jpeg));
    open_ada(&mut app, vec![large]);
    draw(&mut app, 80, 24);
    assert!(app.wait_for_images(WAIT));
    let text = screen(&draw(&mut app, 80, 24));
    let image_box = text
        .lines()
        .skip_while(|l| !l.contains("[image] the view"))
        .nth(1)
        .unwrap_or_default();
    assert!(image_box.contains("[image]"), "{text}");
    assert_eq!(app.image_encodes(), 0);
}
