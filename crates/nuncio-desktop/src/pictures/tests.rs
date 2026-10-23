use std::sync::{Condvar, mpsc};
use std::time::Duration;

use base64::Engine;
use image::{ImageEncoder, Rgba, RgbaImage};
use nuncio_client::wire::PROTOCOL_VERSION;
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message};

use super::*;
use crate::controller::{Command, Controller};
use crate::present::bubbles;
use crate::worker::Incoming;

const ANA: &str = "15551230001@s.whatsapp.net";
const WAIT: Duration = Duration::from_secs(10);

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

fn message(id: &str, kind: &str, text: &str) -> Message {
    Message {
        id: id.to_owned(),
        kind: kind.to_owned(),
        text: text.to_owned(),
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

/// A `width` x `height` JPEG gradient, as the demo's photo thumbnail is.
fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 7) as u8, 160])
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

/// A `width` x `height` PNG, transparent but for its centre pixel.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut picture = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    picture.put_pixel(width / 2, height / 2, Rgba([250, 200, 40, 255]));
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            picture.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("encode a png");
    bytes
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The demo's photo: a 480x320 picture whose message embeds a 48x32 JPEG thumbnail.
fn photo(id: &str) -> Message {
    let mut m = message(id, "image", "the beach");
    m.thumb = Some(base64(&jpeg(48, 32)));
    m.thumb_mime = Some("image/jpeg".to_owned());
    m.width = Some(480);
    m.height = Some(320);
    m
}

fn reply(request: &ClientFrame, frame: DaemonFrame) -> Incoming {
    Incoming::Reply {
        request: request.clone(),
        result: Ok(frame),
    }
}

/// Connected, Ana listed and opened with `messages` (newest first) loaded; every command the
/// controller made on the way is returned too.
fn opened(messages: Vec<Message>) -> (Controller, Vec<Command>) {
    let mut c = Controller::new("d");
    let mut made = c.incoming(Incoming::Connected);
    let more = c.incoming(reply(
        &ClientFrame::chats(200),
        DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![chat(ANA, "Ana")],
        },
    ));
    made.extend(more);
    let open = c.open_chat(0);
    made.extend(open);
    let more = c.incoming(reply(
        &ClientFrame::messages(ANA, 200),
        DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages,
        },
    ));
    made.extend(more);
    (c, made)
}

fn is_download(command: &Command) -> bool {
    matches!(command, Command::Send(ClientFrame::DownloadMedia { .. }))
}

fn ready(id: &str, path: &str) -> Incoming {
    reply(
        &ClientFrame::download_media(ANA, id),
        DaemonFrame::MediaReady {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            id: id.to_owned(),
            path: path.to_owned(),
            mime: "image/png".to_owned(),
        },
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nuncio-pictures-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn a_bubble_reserves_its_box_from_the_size_the_message_states() {
    assert_eq!(picture_box(480, 320, false), (300, 200));
    assert_eq!(picture_box(512, 512, true), (160, 160));
    assert_eq!(
        picture_box(48, 32, false),
        (48, 32),
        "never larger than itself"
    );
    assert_eq!(picture_box(1000, 4000, false), (75, 300));
    let (c, _) = opened(vec![photo("P1")]);
    let shown = bubbles(c.model(), 0);
    let picture = shown[0].picture.as_ref().expect("a photo has a picture");
    assert_eq!(picture.size, Some((300, 200)));
    let mut no_size = photo("P2");
    no_size.width = None;
    let (c, _) = opened(vec![no_size]);
    assert_eq!(
        bubbles(c.model(), 0)[0].picture.as_ref().unwrap().size,
        None
    );
}

#[test]
fn a_picture_bubble_is_labelled_photo_or_sticker_and_other_messages_have_no_picture() {
    let mut sticker = message("S1", "sticker", "");
    sticker.animated = true;
    sticker.width = Some(512);
    sticker.height = Some(512);
    let (c, _) = opened(vec![
        message("T1", "text", "hi"),
        message("D1", "document", ""),
        sticker,
        photo("P1"),
    ]);
    let shown = bubbles(c.model(), 0);
    // Newest first on the wire: the conversation is P1, S1, D1, T1.
    let photo = shown[0].picture.as_ref().expect("photo");
    assert_eq!(
        (photo.alt.as_str(), photo.sticker, photo.thumb),
        ("Photo, the beach", false, true)
    );
    let sticker = shown[1].picture.as_ref().expect("sticker");
    assert_eq!(
        (
            sticker.alt.as_str(),
            sticker.sticker,
            sticker.animated,
            sticker.thumb
        ),
        ("Sticker", true, true, false)
    );
    assert_eq!(shown[1].media.as_deref(), Some("[sticker] ▶"));
    assert!(shown[2].picture.is_none() && shown[3].picture.is_none());
    // A screen reader hears what the picture is, then its caption.
    assert_eq!(shown[0].label, "Ana, 10:00: Photo, the beach");
    assert_eq!(shown[1].label, "Ana, 10:00: Sticker");
    assert_eq!(
        shown[2].label, "Ana, 10:00: [document]",
        "other media as before"
    );
}

#[test]
fn a_thumbnail_is_asked_for_at_once_and_nothing_is_ever_downloaded_by_itself() {
    let (mut c, made) = opened(vec![photo("P1")]);
    assert!(!made.iter().any(is_download), "{made:?}");
    let cache = Cache::<u8>::default();
    let shown = bubbles(c.model(), 0);
    assert_eq!(
        requests(c.model(), &shown[0], &cache),
        [Request {
            key: Key::new(ANA, "P1", Variant::Thumb),
            origin: Origin::Base64(base64(&jpeg(48, 32))),
            target: (300, 200),
            animated: false,
        }]
    );
    // Events, a resync and the window being seen fetch nothing.
    let mut later = c.incoming(Incoming::Event(DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: ANA.to_owned(),
        message: photo("P2"),
    }));
    later.extend(c.set_seen(true));
    later.extend(c.incoming(Incoming::Connected));
    assert!(!later.iter().any(is_download), "{later:?}");
    for bubble in bubbles(c.model(), 0) {
        assert!(
            requests(c.model(), &bubble, &cache)
                .iter()
                .all(|r| r.key.variant == Variant::Thumb),
            "only thumbnails before a click"
        );
    }
}

#[test]
fn the_full_picture_is_fetched_only_when_asked_and_shown_in_the_bubble() {
    let (mut c, _) = opened(vec![photo("P1")]);
    let mut cache = Cache::<u8>::default();
    let thumb = Key::new(ANA, "P1", Variant::Thumb);
    assert!(cache.start(&thumb));
    assert!(cache.finish(thumb, Ok((1, 4, (48, 32)))));
    let asked = c.open_media(0);
    assert_eq!(
        asked,
        [Command::Send(ClientFrame::download_media(ANA, "P1"))]
    );
    let shown = bubbles(c.model(), 0);
    assert_eq!(shown[0].media.as_deref(), Some("[image] downloading…"));
    assert_eq!(shown[0].label, "Ana, 10:00: Photo, downloading…, the beach");
    assert!(requests(c.model(), &shown[0], &cache).is_empty());
    let path = "/tmp/nuncio-media/P1.png";
    assert!(
        c.incoming(ready("P1", path)).is_empty(),
        "shown here, not opened outside"
    );
    let shown = bubbles(c.model(), 0);
    let picture = shown[0].picture.as_ref().unwrap();
    assert_eq!(picture.full, Some(PathBuf::from(path)));
    assert_eq!(
        requests(c.model(), &shown[0], &cache),
        [Request {
            key: Key::new(ANA, "P1", Variant::Full),
            origin: Origin::File(PathBuf::from(path)),
            target: (300, 200),
            animated: false,
        }]
    );
    // Open externally keeps today's behaviour: the file is opened outside the app.
    assert_eq!(
        c.open_externally(0),
        [Command::Send(ClientFrame::download_media(ANA, "P1"))]
    );
    assert_eq!(
        c.incoming(ready("P1", path)),
        [Command::Open(PathBuf::from(path))]
    );
}

#[test]
fn a_cached_picture_is_not_decoded_again_and_the_full_one_replaces_the_thumbnail() {
    let (mut c, _) = opened(vec![photo("P1")]);
    let mut cache = Cache::<&str>::default();
    let thumb = Key::new(ANA, "P1", Variant::Thumb);
    let full = Key::new(ANA, "P1", Variant::Full);
    let shown = bubbles(c.model(), 0);
    assert!(cache.start(&thumb));
    assert!(
        requests(c.model(), &shown[0], &cache).is_empty(),
        "being decoded"
    );
    assert!(!cache.start(&thumb), "not twice");
    cache.finish(thumb.clone(), Ok(("thumb", 10, (48, 32))));
    assert_eq!(cache.best(ANA, "P1"), Some(("thumb", Variant::Thumb)));
    assert!(requests(c.model(), &shown[0], &cache).is_empty(), "a hit");
    c.open_media(0);
    c.incoming(ready("P1", "/tmp/nuncio-media/P1.png"));
    let shown = bubbles(c.model(), 0);
    assert!(cache.start(&full));
    cache.finish(full, Ok(("full", 10, (480, 320))));
    assert_eq!(cache.best(ANA, "P1"), Some(("full", Variant::Full)));
    assert_eq!(cache.full_pixels(ANA, "P1"), Some((480, 320)));
    assert!(requests(c.model(), &shown[0], &cache).is_empty());
    // With the full picture there, the thumbnail is not wanted even when the cache has none.
    let mut only_full = Cache::<&str>::default();
    let full = Key::new(ANA, "P1", Variant::Full);
    assert!(only_full.start(&full));
    only_full.finish(full, Ok(("full", 10, (480, 320))));
    assert!(requests(c.model(), &shown[0], &only_full).is_empty());
}

#[test]
fn a_full_picture_that_cannot_be_shown_says_why() {
    let (mut c, _) = opened(vec![photo("P1")]);
    c.picture_failed("the picture could not be decoded");
    assert_eq!(
        c.take_notice().as_deref(),
        Some("cannot show the picture: the picture could not be decoded")
    );
}

#[test]
fn the_least_recently_used_pictures_are_evicted_by_count_and_by_bytes() {
    let key = |id: &str| Key::new(ANA, id, Variant::Thumb);
    let mut cache = Cache::<u32>::with_limits(2, 100);
    for (n, id) in ["A", "B"].into_iter().enumerate() {
        assert!(cache.start(&key(id)));
        cache.finish(key(id), Ok((n as u32, 10, (1, 1))));
    }
    assert_eq!(cache.get(&key("A")), Some(0), "A used after B");
    assert!(cache.start(&key("C")));
    cache.finish(key("C"), Ok((2, 10, (1, 1))));
    assert_eq!(cache.len(), 2);
    assert!(cache.contains(&key("A")) && cache.contains(&key("C")));
    assert!(!cache.contains(&key("B")), "the least recently used goes");
    assert!(cache.wants(&key("B")), "and can be decoded again");
    // Over the bytes: the older ones go, the newest stays even alone.
    assert!(cache.start(&key("D")));
    cache.finish(key("D"), Ok((3, 150, (1, 1))));
    assert_eq!((cache.len(), cache.bytes()), (1, 150));
    assert!(cache.contains(&key("D")));
}

#[test]
fn a_picture_that_fails_is_not_asked_for_again_until_its_full_one_is_retried() {
    let (c, _) = opened(vec![photo("P1")]);
    let mut cache = Cache::<u8>::default();
    let thumb = Key::new(ANA, "P1", Variant::Thumb);
    let full = Key::new(ANA, "P1", Variant::Full);
    assert!(cache.start(&thumb));
    assert!(cache.finish(thumb.clone(), Err("bad".to_owned())));
    assert!(cache.has_failed(&thumb) && cache.best(ANA, "P1").is_none());
    let shown = bubbles(c.model(), 0);
    assert!(requests(c.model(), &shown[0], &cache).is_empty());
    assert!(cache.start(&full));
    cache.finish(full.clone(), Err("bad".to_owned()));
    assert!(!cache.wants(&full));
    cache.retry_full(ANA, "P1");
    assert!(cache.wants(&full) && !cache.wants(&thumb));
    assert!(
        !cache.finish(Key::new(ANA, "P9", Variant::Thumb), Ok((1, 1, (1, 1)))),
        "a result nobody waits for is dropped"
    );
    assert!(cache.is_empty());
}

#[test]
fn a_thumbnail_decodes_to_its_own_size_and_a_large_picture_shrinks_to_twice_its_box() {
    let request = |variant, origin| Request {
        key: Key::new(ANA, "P1", variant),
        origin,
        target: (300, 200),
        animated: false,
    };
    let thumb = decode_picture(&request(
        Variant::Thumb,
        Origin::Base64(base64(&jpeg(48, 32))),
    ))
    .expect("the demo thumbnail");
    assert_eq!(
        (thumb.width, thumb.height, thumb.source),
        (48, 32, (48, 32))
    );
    assert_eq!(thumb.rgba.len(), 48 * 32 * 4);
    let dir = temp_dir("shrink");
    let file = dir.join("big.jpg");
    std::fs::write(&file, jpeg(1200, 800)).expect("write");
    let full = decode_picture(&request(Variant::Full, Origin::File(file))).expect("full");
    assert_eq!(
        (full.width, full.height, full.source),
        (600, 400, (1200, 800))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sticker_keeps_its_transparency_and_bad_bytes_are_an_error() {
    let sticker = decode_picture(&Request {
        key: Key::new(ANA, "S1", Variant::Thumb),
        origin: Origin::Base64(base64(&png(64, 64))),
        target: (160, 160),
        animated: false,
    })
    .expect("a png sticker thumbnail");
    assert_eq!(sticker.rgba[3], 0, "a transparent corner stays transparent");
    let centre = ((32 * 64 + 32) * 4) as usize;
    assert_eq!(&sticker.rgba[centre..centre + 4], &[250, 200, 40, 255]);
    for origin in [
        Origin::Base64("not base64!".to_owned()),
        Origin::Base64(base64(b"not a picture")),
        Origin::File(PathBuf::from("/nonexistent/nuncio/picture.png")),
    ] {
        let request = Request {
            key: Key::new(ANA, "S1", Variant::Thumb),
            origin,
            target: (160, 160),
            animated: false,
        };
        assert!(decode_picture(&request).is_err(), "{request:?}");
    }
    // A thumbnail whose header declares more than a thumbnail may be.
    let mut big = jpeg(1100, 8);
    big.truncate(big.len() - 2);
    assert_eq!(
        decode_picture(&Request {
            key: Key::new(ANA, "S1", Variant::Thumb),
            origin: Origin::Base64(base64(&big)),
            target: (160, 160),
            animated: false,
        }),
        Err("the picture is larger than nuncio draws".to_owned())
    );
}

#[test]
fn at_most_two_pictures_are_decoded_at_once_and_every_one_is_answered() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (tx, rx) = mpsc::channel();
    let post = {
        let gate = Arc::clone(&gate);
        move |decoded: Decoded| {
            let _ = tx.send(decoded);
            let (open, opened) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
        }
    };
    let decoder = Decoder::spawn(post);
    for id in ["A", "B", "C"] {
        decoder.decode(Request {
            key: Key::new(ANA, id, Variant::Thumb),
            origin: Origin::Base64(base64(&jpeg(8, 8))),
            target: (8, 8),
            animated: false,
        });
    }
    let first = rx.recv_timeout(WAIT).expect("one");
    let second = rx.recv_timeout(WAIT).expect("two");
    assert!(first.outcome.is_ok() && second.outcome.is_ok());
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "both threads are busy: the third waits"
    );
    let (open, opened) = &*gate;
    *open.lock().unwrap() = true;
    opened.notify_all();
    let third = rx.recv_timeout(WAIT).expect("three");
    let mut ids = [first.key.id, second.key.id, third.key.id];
    ids.sort();
    assert_eq!(ids, ["A", "B", "C"]);
}

#[test]
fn a_picture_with_no_stated_size_takes_its_box_once_from_its_first_image() {
    // A thumbnail is smaller than its picture: its shape, scaled up to the caps.
    assert_eq!(first_image_box(Variant::Thumb, (48, 32), false), (300, 200));
    assert_eq!(first_image_box(Variant::Thumb, (64, 64), true), (160, 160));
    // A full picture: its own box, never larger than itself.
    assert_eq!(first_image_box(Variant::Full, (100, 80), false), (100, 80));
    assert_eq!(first_image_box(Variant::Full, (0, 0), false), (1, 1));
    let mut cache = Cache::<u8>::default();
    let thumb = Key::new(ANA, "P2", Variant::Thumb);
    let full = Key::new(ANA, "P2", Variant::Full);
    assert_eq!(cache.known_box(ANA, "P2"), None);
    assert_eq!(cache.settle_box(&thumb, (48, 32), false), (300, 200));
    assert_eq!(
        cache.settle_box(&full, (600, 200), false),
        (300, 200),
        "the full picture keeps the box"
    );
    cache.clear();
    assert_eq!(
        cache.known_box(ANA, "P2"),
        Some((300, 200)),
        "kept once the cache let its pictures go"
    );
    let first_full = Key::new(ANA, "P3", Variant::Full);
    assert_eq!(cache.settle_box(&first_full, (100, 80), false), (100, 80));
    assert_eq!(
        cache.settle_box(&Key::new(ANA, "P3", Variant::Thumb), (48, 32), false),
        (100, 80)
    );
    // The box it settled on is what its pictures are decoded for.
    let mut no_size = photo("P2");
    no_size.width = None;
    let (c, _) = opened(vec![no_size]);
    let shown = bubbles(c.model(), 0);
    assert_eq!(requests(c.model(), &shown[0], &cache)[0].target, (300, 200));
}

#[test]
fn while_the_full_picture_downloads_an_evicted_thumbnail_is_asked_for_again() {
    let (mut c, _) = opened(vec![photo("P1")]);
    let mut cache = Cache::<u8>::default();
    let thumb = Key::new(ANA, "P1", Variant::Thumb);
    assert!(cache.start(&thumb));
    cache.finish(thumb.clone(), Ok((1, 4, (48, 32))));
    c.open_media(0);
    let shown = bubbles(c.model(), 0);
    assert!(
        shown[0]
            .media
            .as_deref()
            .is_some_and(|m| m.ends_with("downloading…"))
    );
    assert_eq!(
        cache.best(ANA, "P1"),
        Some((1, Variant::Thumb)),
        "drawn meanwhile"
    );
    cache.clear();
    assert_eq!(
        requests(c.model(), &shown[0], &cache)
            .into_iter()
            .map(|r| r.key)
            .collect::<Vec<_>>(),
        [thumb]
    );
}

#[test]
fn opening_a_picture_asks_for_its_retry_and_other_media_do_not() {
    let (mut c, _) = opened(vec![message("D1", "document", ""), photo("P1")]);
    assert_eq!(c.take_picture_retries(), Vec::<(String, String)>::new());
    c.open_media(0);
    assert_eq!(
        c.take_picture_retries(),
        [(ANA.to_owned(), "P1".to_owned())]
    );
    assert!(c.take_picture_retries().is_empty(), "taken once");
    c.open_media(1);
    c.open_externally(0);
    assert!(c.take_picture_retries().is_empty());
}

/// An animated WebP sticker of `side` x `side` whose `frames` fill the canvas, each of its own
/// colour and shown for `delay_ms`, as WhatsApp's animated stickers are.
fn animated_sticker(side: u32, frames: usize, delay_ms: u32) -> Vec<u8> {
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
    let mut vp8x = vec![0x10 | 0x02, 0, 0, 0];
    vp8x.extend_from_slice(&u24(side - 1));
    vp8x.extend_from_slice(&u24(side - 1));
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for n in 0..frames {
        let colour = Rgba([(n * 40) as u8, 255 - (n * 30) as u8, 90, 255]);
        let picture = RgbaImage::from_pixel(side, side, colour);
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
        for value in [0, 0, side - 1, side - 1, delay_ms] {
            anmf.extend_from_slice(&u24(value));
        }
        // Drawn over the canvas, not blended.
        anmf.push(0x02);
        anmf.extend_from_slice(&still[12..]);
        body.extend(chunk(b"ANMF", &anmf));
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}

fn sticker_request(file: PathBuf, variant: Variant, animated: bool) -> Request {
    Request {
        key: Key::new(ANA, "S1", variant),
        origin: Origin::File(file),
        target: (160, 160),
        animated,
    }
}

#[test]
fn the_frame_shown_is_the_one_whose_delay_the_elapsed_time_is_in_looping_forever() {
    let delays = [100, 50, 200];
    for (elapsed, index) in [
        (0, 0),
        (99, 0),
        (100, 1),
        (149, 1),
        (150, 2),
        (349, 2),
        // Past the last frame it starts again, as often as it plays.
        (350, 0),
        (449, 0),
        (450, 1),
        (350 * 1000 + 120, 1),
        (u64::MAX, frame_at(&delays, u64::MAX % 350)),
    ] {
        assert_eq!(frame_at(&delays, elapsed), index, "{elapsed} ms");
    }
    assert_eq!(frame_at(&[], 500), 0, "no frames: the first");
    assert_eq!(frame_at(&[0, 0], 500), 0, "no time: the first");
}

#[test]
fn an_opened_animated_sticker_keeps_every_frame_shrunk_to_its_box() {
    let dir = temp_dir("animated");
    let file = dir.join("S1.webp");
    std::fs::write(&file, animated_sticker(512, 3, 5)).expect("write");
    let full = decode_picture(&sticker_request(file.clone(), Variant::Full, true)).expect("full");
    assert_eq!(
        (full.width, full.height, full.source),
        (160, 160, (512, 512)),
        "an animation's frames are at its box, not twice it"
    );
    assert_eq!(full.rgba.len(), 160 * 160 * 4, "its first frame too");
    assert_eq!(full.delays_ms, [100, 100, 100], "under 10 ms is 100 ms");
    assert_eq!(full.later.len(), 2);
    assert!(full.later.iter().all(|frame| frame.len() == 160 * 160 * 4));
    assert_ne!(full.later[0], full.rgba, "each frame its own");
    assert_eq!(full.bytes(), 3 * 160 * 160 * 4, "every frame is counted");
    // A sticker that does not animate still shows its first frame at twice its box, as before.
    let still = decode_picture(&sticker_request(file, Variant::Full, false)).expect("still");
    assert_eq!(
        (still.width, still.height, still.source),
        (320, 320, (512, 512))
    );
    assert!(still.delays_ms.is_empty() && still.later.is_empty());
    assert_eq!(still.bytes(), 320 * 320 * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_animated_sticker_over_its_frame_cap_shows_its_first_frame_only() {
    // Each frame is kept at the 160 px box: 100 KiB, over the 4 KiB floor, so 163 fit.
    let frame = 160 * 160 * 4;
    let fit = ANIMATION_BYTES / frame;
    assert_eq!(ANIMATION_BYTES, 16 << 20);
    assert_eq!(fit, 163);
    let dir = temp_dir("animation-cap");
    for (frames, animates) in [(fit, true), (fit + 1, false)] {
        let file = dir.join(format!("S{frames}.webp"));
        std::fs::write(&file, animated_sticker(512, frames, 40)).expect("write");
        let full = decode_picture(&sticker_request(file, Variant::Full, true)).expect("full");
        assert_eq!(
            (
                full.later.len() + 1 == frames,
                full.delays_ms.len() == frames
            ),
            (animates, animates),
            "{frames} frames"
        );
        if animates {
            assert_eq!((full.width, full.height), (160, 160));
        } else {
            // Its first frame, as a still sticker shows it: twice its box.
            assert_eq!((full.width, full.height), (320, 320));
            assert!(full.later.is_empty() && full.delays_ms.is_empty());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_full_picture_of_an_animated_sticker_is_asked_to_animate() {
    let mut animated = message("S1", "sticker", "");
    animated.animated = true;
    animated.thumb = Some(base64(&png(64, 64)));
    let mut still = animated.clone();
    still.id = "S2".to_owned();
    still.animated = false;
    let (mut c, _) = opened(vec![photo("P1"), still, animated]);
    let cache = Cache::<u8>::default();
    // Newest first on the wire: S1, S2, P1.
    let asked = |c: &Controller| {
        bubbles(c.model(), 0)
            .iter()
            .flat_map(|bubble| requests(c.model(), bubble, &cache))
            .map(|request| (request.key.id, request.key.variant, request.animated))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        asked(&c),
        [
            ("S1".to_owned(), Variant::Thumb, false),
            ("S2".to_owned(), Variant::Thumb, false),
            ("P1".to_owned(), Variant::Thumb, false),
        ],
        "a thumbnail stays still"
    );
    for (index, id) in [(0, "S1"), (1, "S2"), (2, "P1")] {
        c.open_media(index);
        c.incoming(ready(id, &format!("/tmp/nuncio-media/{id}.webp")));
    }
    let full: Vec<_> = asked(&c)
        .into_iter()
        .filter(|(_, variant, _)| *variant == Variant::Full)
        .collect();
    assert_eq!(
        full,
        [
            ("S1".to_owned(), Variant::Full, true),
            ("S2".to_owned(), Variant::Full, false),
            ("P1".to_owned(), Variant::Full, false),
        ]
    );
}

#[test]
fn an_animated_thumbnail_or_a_photo_decodes_as_before() {
    let dir = temp_dir("still");
    let file = dir.join("S1.webp");
    std::fs::write(&file, animated_sticker(64, 3, 100)).expect("write");
    let thumb = decode_picture(&sticker_request(file, Variant::Thumb, false)).expect("thumb");
    assert!(thumb.later.is_empty() && thumb.delays_ms.is_empty());
    let photo = dir.join("P1.jpg");
    std::fs::write(&photo, jpeg(480, 320)).expect("write");
    let full = decode_picture(&Request {
        key: Key::new(ANA, "P1", Variant::Full),
        origin: Origin::File(photo),
        target: (300, 200),
        animated: false,
    })
    .expect("photo");
    assert_eq!((full.width, full.height), (480, 320));
    assert!(full.later.is_empty() && full.delays_ms.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_animated_sticker_costs_at_least_a_minimum_per_frame_and_a_still_its_pixels() {
    let dir = temp_dir("frame-cost");
    let file = dir.join("S1.webp");
    std::fs::write(&file, animated_sticker(8, 3, 100)).expect("write");
    let full = decode_picture(&sticker_request(file.clone(), Variant::Full, true)).expect("full");
    assert_eq!(full.later.len(), 2);
    assert_eq!(
        full.bytes(),
        3 * nuncio_client::pictures::FRAME_MIN_BYTES as usize,
        "each 256-byte frame costs its texture"
    );
    let still = decode_picture(&sticker_request(file, Variant::Full, false)).expect("still");
    assert_eq!(
        still.bytes(),
        8 * 8 * 4,
        "a still picture costs its pixels, as before"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
