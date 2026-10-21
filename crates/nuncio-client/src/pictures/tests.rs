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
    const { assert!(THUMB_BOUNDS.side < FULL_BOUNDS.side && THUMB_BOUNDS.alloc < FULL_BOUNDS.alloc) };
}
