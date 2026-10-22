use super::*;
use image::{ImageEncoder, Rgba, RgbaImage};

/// A `width` x `height` PNG of one colour, fully transparent when `alpha` is 0.
fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
    let picture = RgbaImage::from_pixel(width, height, Rgba([200, 40, 90, alpha]));
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

/// A `width` x `height` JPEG, as WhatsApp's JPEGThumbnail is.
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

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A real 1x1 PNG whose header (IHDR, with a valid checksum) declares `width` x `height`: tiny
/// on the wire, huge if decoded.
fn png_declaring(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png(1, 1, 255);
    // Signature (8), IHDR length (4), "IHDR" (4), then width and height.
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    bytes
}

#[test]
fn a_pixel_box_keeps_the_shape_within_its_maximum_and_never_grows_a_picture() {
    // The demo photo, in a 300x300 box: as wide as allowed, as tall as its shape says.
    assert_eq!(fit_box(480, 320, 300, 300, 1, 1), (300, 200));
    // A 512x512 sticker in a 160x160 box.
    assert_eq!(fit_box(512, 512, 160, 160, 1, 1), (160, 160));
    // Tall: the height caps it and the width shrinks with it.
    assert_eq!(fit_box(1000, 4000, 300, 300, 1, 1), (75, 300));
    // Smaller than the box: its own size.
    assert_eq!(fit_box(48, 32, 300, 300, 1, 1), (48, 32));
}

#[test]
fn a_cell_box_is_the_terminal_box_nuncio_tui_draws() {
    // nuncio-tui's box_size at a 10x20 cell, 46 columns, 12 rows.
    assert_eq!(fit_box(320, 240, 46, 12, 10, 20), (32, 12));
    assert_eq!(fit_box(4000, 1000, 46, 12, 10, 20), (46, 6));
    assert_eq!(fit_box(1000, 4000, 46, 12, 10, 20), (6, 12));
}

#[test]
fn absurd_sizes_and_zero_cells_or_maximums_give_a_small_non_empty_box() {
    for (width, height) in [
        (0, 0),
        (1, 1),
        (u32::MAX, u32::MAX),
        (u32::MAX, 1),
        (1, u32::MAX),
    ] {
        let (cols, rows) = fit_box(width, height, 300, 300, 1, 1);
        assert!(
            (1..=300).contains(&cols) && (1..=300).contains(&rows),
            "{width}x{height} gave {cols}x{rows}"
        );
    }
    assert_eq!(fit_box(480, 320, 0, 0, 0, 0), (1, 1));
}

#[test]
fn a_jpeg_png_and_transparent_png_decode_to_their_size() {
    let photo = decode(&jpeg(48, 32), THUMB_BOUNDS).expect("a jpeg thumbnail");
    assert_eq!((photo.width(), photo.height()), (48, 32));
    let sticker = decode(&png(64, 64, 0), THUMB_BOUNDS).expect("a png thumbnail");
    assert_eq!((sticker.width(), sticker.height()), (64, 64));
    assert_eq!(sticker.to_rgba8().get_pixel(0, 0).0[3], 0, "alpha is kept");
}

#[test]
fn a_header_declaring_a_huge_picture_is_refused_without_decoding() {
    let huge = png_declaring(60_000, 60_000);
    assert!(huge.len() < 200, "the file itself is tiny");
    for bounds in [THUMB_BOUNDS, FULL_BOUNDS] {
        assert_eq!(
            decode(&huge, bounds).map(|_| ()),
            Err("the picture is larger than nuncio draws".to_owned())
        );
    }
    // A thumbnail may not declare what a full picture may.
    let large = png_declaring(2048, 2048);
    assert_eq!(
        decode(&large, THUMB_BOUNDS).map(|_| ()),
        Err("the picture is larger than nuncio draws".to_owned())
    );
    assert_ne!(
        decode(&large, FULL_BOUNDS).map(|_| ()),
        Err("the picture is larger than nuncio draws".to_owned()),
        "within a full picture's bounds the header passes (its data then fails)"
    );
}

#[test]
fn corrupt_truncated_or_foreign_bytes_are_an_error_not_a_panic() {
    let mut truncated = jpeg(48, 32);
    truncated.truncate(truncated.len() / 2);
    let bmp = b"BM\x3a\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0\x01\0\0\0\x01\0\0\0\x01\0\x18\0".to_vec();
    for bytes in [
        Vec::new(),
        b"not a picture at all".to_vec(),
        vec![0xFF, 0xD8, 0xFF],
        truncated,
        bmp,
        png(4, 4, 255)[..40].to_vec(),
    ] {
        assert!(decode(&bytes, THUMB_BOUNDS).is_err(), "{bytes:?}");
        assert!(decode(&bytes, FULL_BOUNDS).is_err(), "{bytes:?}");
    }
}

#[test]
fn a_thumbnail_is_standard_base64_and_anything_else_says_so() {
    let bytes = jpeg(8, 8);
    let text = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert_eq!(thumb_bytes(&format!(" {text}\n")), Ok(bytes));
    assert_eq!(
        thumb_bytes("@@not base64@@"),
        Err("the thumbnail is not base64".to_owned())
    );
}

#[test]
fn a_downloaded_file_over_the_cap_is_refused_and_a_missing_one_says_why() {
    let dir = std::env::temp_dir().join(format!("nuncio-pictures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let small = dir.join("small.png");
    std::fs::write(&small, png(2, 2, 255)).expect("write");
    assert_eq!(
        read_capped(&small).map(|b| b.len()),
        Ok(png(2, 2, 255).len())
    );
    let big = dir.join("big.bin");
    let file = std::fs::File::create(&big).expect("create");
    file.set_len(MAX_FILE + 1).expect("sparse file");
    assert_eq!(
        read_capped(&big),
        Err("the picture is larger than 64 MiB".to_owned())
    );
    assert!(
        read_capped(&dir.join("missing.png"))
            .is_err_and(|e| e.starts_with("the picture could not be read"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn each_variant_has_its_own_bounds() {
    assert_eq!(Variant::Thumb.bounds(), THUMB_BOUNDS);
    assert_eq!(Variant::Full.bounds(), FULL_BOUNDS);
    assert_eq!(
        FULL_BOUNDS.alloc,
        96 << 20,
        "a full picture may cost 96 MiB"
    );
    const { assert!(THUMB_BOUNDS.side < FULL_BOUNDS.side && THUMB_BOUNDS.alloc < FULL_BOUNDS.alloc) };
}

/// Counts what this thread allocates, so a test can measure a decode's real peak.
mod peak {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Tracking;

    thread_local! {
        static NOW: Cell<isize> = const { Cell::new(0) };
        static PEAK: Cell<isize> = const { Cell::new(0) };
    }

    fn add(bytes: isize) {
        let _ = NOW.try_with(|now| {
            now.set(now.get() + bytes);
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now.get())));
        });
    }

    // SAFETY: every call forwards to the system allocator unchanged; only a counter is updated.
    unsafe impl GlobalAlloc for Tracking {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: forwarded as is.
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                add(layout.size() as isize);
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: forwarded as is.
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                add(layout.size() as isize);
            }
            pointer
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: forwarded as is.
            unsafe { System.dealloc(pointer, layout) };
            add(-(layout.size() as isize));
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            // SAFETY: forwarded as is.
            let moved = unsafe { System.realloc(pointer, layout, size) };
            if !moved.is_null() {
                add(size as isize - layout.size() as isize);
            }
            moved
        }
    }

    #[global_allocator]
    static TRACKING: Tracking = Tracking;

    /// The most `run` had allocated at once on this thread, beyond what was allocated before it.
    pub fn of<T>(run: impl FnOnce() -> T) -> (T, u64) {
        let start = NOW.with(Cell::get);
        PEAK.with(|peak| peak.set(start));
        let value = run();
        let peak = PEAK.with(Cell::get) - start;
        (value, peak.max(0) as u64)
    }
}

/// A lossless WebP of `width` x `height` pixels with alpha.
fn webp(width: u32, height: u32) -> Vec<u8> {
    let picture = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([(x * 3) as u8, (y * 5) as u8, ((x ^ y) * 7) as u8, 200])
    });
    let mut bytes = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
        .write_image(
            picture.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("encode a webp");
    bytes
}

/// An animated WebP whose one frame, at the top left, is a `frame_w` x `frame_h` lossless picture
/// on a `width` x `height` canvas, as a sticker is.
fn animated_webp(width: u32, height: u32, frame_w: u32, frame_h: u32) -> Vec<u8> {
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
    // The still WebP is RIFF, size, WEBP, then its one VP8L chunk.
    let frame = webp(frame_w, frame_h)[12..].to_vec();
    let mut vp8x = vec![0x10 | 0x02, 0, 0, 0];
    vp8x.extend_from_slice(&u24(width - 1));
    vp8x.extend_from_slice(&u24(height - 1));
    let mut anmf = Vec::new();
    for value in [0, 0, frame_w - 1, frame_h - 1, 100] {
        anmf.extend_from_slice(&u24(value));
    }
    anmf.push(0);
    anmf.extend_from_slice(&frame);
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    body.extend(chunk(b"ANMF", &anmf));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}

const TOO_LARGE: &str = "the picture is larger than nuncio draws";

/// What a decoder holds beyond its pixel buffers (measured under 300 KiB).
const WORKING_STATE: u64 = 1 << 20;

#[test]
fn a_full_picture_header_over_96_mib_is_refused_and_one_at_96_mib_passes() {
    // 4096x6144 RGBA is exactly 96 MiB; one more row is over, and 5600x5600 (119 MiB) was
    // admitted under the old 160 MiB cap.
    assert_ne!(
        decode(&png_declaring(4096, 6144), FULL_BOUNDS).map(|_| ()),
        Err(TOO_LARGE.to_owned()),
        "at the cap the header passes (its data then fails)"
    );
    for (width, height) in [(4096, 6145), (5600, 5600)] {
        assert_eq!(
            decode(&png_declaring(width, height), FULL_BOUNDS).map(|_| ()),
            Err(TOO_LARGE.to_owned()),
            "{width}x{height}"
        );
    }
}

#[test]
fn an_animated_webp_counts_its_canvas_and_frame_against_the_cap() {
    // A tiny frame on a canvas of 2048x4096 pixels: three RGBA buffers of it are exactly 96 MiB.
    let at_cap = decode(&animated_webp(2048, 4096, 16, 16), FULL_BOUNDS).expect("at the cap");
    assert_eq!((at_cap.width(), at_cap.height()), (2048, 4096));
    // One more row is over, though its output alone (32 MiB) is far under.
    assert_eq!(
        decode(&animated_webp(2048, 4097, 16, 16), FULL_BOUNDS).map(|_| ()),
        Err(TOO_LARGE.to_owned())
    );
}

#[test]
fn a_decode_never_holds_more_than_its_bounds_admit() {
    let rgb_webp = {
        let picture = image::RgbImage::from_fn(512, 512, |x, y| {
            image::Rgb([(x * 3) as u8, (y * 5) as u8, ((x ^ y) * 7) as u8])
        });
        let mut bytes = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
            .write_image(picture.as_raw(), 512, 512, image::ExtendedColorType::Rgb8)
            .expect("encode a webp");
        bytes
    };
    let deep_png = {
        let picture = image::ImageBuffer::<Rgba<u16>, Vec<u16>>::from_pixel(
            512,
            512,
            Rgba([1000, 2000, 3000, 65535]),
        );
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba16(picture)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode a 16-bit png");
        bytes
    };
    let pixels = 512 * 512;
    // The smallest allocation each one is admitted with: four bytes a pixel, eight for a 16-bit
    // PNG's output, twelve for a WebP (it may be animated).
    for (name, bytes, admitted) in [
        ("png", png(512, 512, 255), pixels * 4),
        ("jpeg", jpeg(512, 512), pixels * 4),
        ("16-bit png", deep_png, pixels * 8),
        ("webp", webp(512, 512), pixels * 12),
        ("rgb webp", rgb_webp, pixels * 12),
        (
            "animated webp",
            animated_webp(512, 512, 512, 512),
            pixels * 12,
        ),
        (
            "animated webp, small frame",
            animated_webp(512, 512, 8, 8),
            pixels * 12,
        ),
    ] {
        let bounds = Bounds {
            side: MAX_SIDE,
            alloc: admitted,
        };
        let (decoded, peak) = peak::of(|| decode(&bytes, bounds));
        assert!(decoded.is_ok(), "{name}: {:?}", decoded.map(|_| ()));
        assert!(
            peak <= admitted + WORKING_STATE,
            "{name}: held {peak} bytes, admitted {admitted}"
        );
        let tighter = Bounds {
            side: MAX_SIDE,
            alloc: admitted - 1,
        };
        assert!(decode(&bytes, tighter).is_err(), "{name} under its bounds");
    }
}

#[test]
fn a_picture_within_the_cap_decodes_exactly_as_the_decoder_alone_does() {
    for (name, bytes) in [
        ("png", png(640, 480, 128)),
        ("jpeg", jpeg(1024, 768)),
        ("webp", webp(512, 512)),
        ("animated webp", animated_webp(512, 512, 300, 200)),
    ] {
        let ours = decode(&bytes, FULL_BOUNDS).expect(name);
        let plain = image::load_from_memory(&bytes).expect(name);
        assert_eq!(
            (ours.width(), ours.height()),
            (plain.width(), plain.height()),
            "{name}"
        );
        assert_eq!(ours.color(), plain.color(), "{name}");
        assert!(ours.as_bytes() == plain.as_bytes(), "{name}: pixels differ");
    }
}
