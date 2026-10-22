//! The picture rules every client follows, whatever it draws with (T15 Part 2.5, M3 D3d): how big
//! a picture's box is, and how the untrusted bytes of a thumbnail or a downloaded picture are
//! decoded.
//!
//! The thumbnail is the one WhatsApp embeds in the message (the wire's base64 `thumb`); the full
//! picture is the file a `download_media` wrote, fetched only when the user asks. Every byte
//! decoded here is untrusted: only JPEG, PNG and WebP are decoded (an animated WebP gives its first
//! frame), the size the header declares is checked before anything is decoded, and each variant
//! has its own bounds, so a tiny thumbnail declaring a huge picture costs nothing. Nothing here
//! panics on bad input: it says why in an `Err`.

use std::io::{Cursor, Read};
use std::path::Path;

use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

/// A full picture file larger than this is not decoded (WhatsApp sends far smaller ones).
pub const MAX_FILE: u64 = 64 << 20;
/// The largest side a picture may have, as the wire bounds `width` and `height`.
pub const MAX_SIDE: u32 = 16_384;

/// What a decode may cost: its largest side and its allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub side: u32,
    pub alloc: u64,
}

/// A thumbnail is tiny on the wire but its header can declare any size, so it gets far tighter
/// bounds than a full picture.
pub const THUMB_BOUNDS: Bounds = Bounds {
    side: 1024,
    alloc: 16 << 20,
};
/// A full picture: a 4096x4096 RGBA photo is 64 MiB. Every pixel buffer a decode holds at once
/// fits in 96 MiB, so a WebP, which may be animated, gets a third of the pixels (see [`decode`]).
pub const FULL_BOUNDS: Bounds = Bounds {
    side: MAX_SIDE,
    alloc: 96 << 20,
};

/// Which picture of a message: the embedded thumbnail or the full file fetched on request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Thumb,
    Full,
}

impl Variant {
    /// The bounds its decode is held to.
    pub fn bounds(self) -> Bounds {
        match self {
            Self::Thumb => THUMB_BOUNDS,
            Self::Full => FULL_BOUNDS,
        }
    }
}

/// The box a `width` x `height` pixel picture gets, counted in cells of `cell_w` x `cell_h` pixels
/// (a terminal's character cell, or 1x1 for a pixel box): as wide as it is in cells but at most
/// `max_cols`, as tall as that width keeps its shape, at most `max_rows` (the width then shrinks to
/// keep the shape), never empty and never larger than the picture itself. Sizes outside
/// 1..=[`MAX_SIDE`] are clamped; a cell or a maximum of zero is taken as one.
pub fn fit_box(
    width: u32,
    height: u32,
    max_cols: u32,
    max_rows: u32,
    cell_w: u32,
    cell_h: u32,
) -> (u32, u32) {
    let (cell_w, cell_h) = (u64::from(cell_w.max(1)), u64::from(cell_h.max(1)));
    let w = u64::from(width.clamp(1, MAX_SIDE));
    let h = u64::from(height.clamp(1, MAX_SIDE));
    let mut cols = w.div_ceil(cell_w).min(u64::from(max_cols.max(1))).max(1);
    let mut rows = (h * cols * cell_w)
        .div_ceil(w * cell_h)
        .clamp(1, h.div_ceil(cell_h).max(1));
    let cap = u64::from(max_rows.max(1));
    if rows > cap {
        rows = cap;
        cols = (rows * cell_h * w).div_ceil(h * cell_w).clamp(1, cols);
    }
    // Both are bounded by `max_cols` and `max_rows`, which are u32.
    (cols as u32, rows as u32)
}

/// The bytes of a wire `thumb` (standard base64).
pub fn thumb_bytes(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| "the thumbnail is not base64".to_owned())
}

/// Read a downloaded picture, refusing one larger than [`MAX_FILE`].
pub fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("the picture could not be read: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("the picture could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("the picture is larger than 64 MiB".to_owned());
    }
    Ok(bytes)
}

/// The bytes a pixel may cost at a decode's peak. A JPEG or PNG holds little beyond its output, an
/// RGBA pixel at most (a wider output is refused by `max_alloc`). A WebP may be animated, and its
/// first frame then needs the output, the canvas and the frame at once: three RGBA pixels.
fn peak_bytes_per_pixel(format: Option<ImageFormat>) -> u64 {
    match format {
        Some(ImageFormat::WebP) => 12,
        _ => 4,
    }
}

/// Decode untrusted bytes: only JPEG, PNG and WebP (an animated WebP gives its first frame), within
/// `bounds`. The size the header declares is checked first, so a small file declaring a huge
/// picture is refused without decoding anything; the pixel buffers a decode holds at once then
/// stay within `bounds.alloc` (the decoder's own working state adds well under 1 MiB).
pub fn decode(bytes: &[u8], bounds: Bounds) -> Result<DynamicImage, String> {
    let too_large = || "the picture is larger than nuncio draws".to_owned();
    let header = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "the picture could not be read".to_owned())?;
    let per_pixel = peak_bytes_per_pixel(header.format());
    let (width, height) = header
        .into_dimensions()
        .map_err(|error| format!("the picture could not be read: {error}"))?;
    if width > bounds.side
        || height > bounds.side
        || u64::from(width) * u64::from(height) * per_pixel > bounds.alloc
    {
        return Err(too_large());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "the picture could not be read".to_owned())?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err("not a JPEG, PNG or WebP picture".to_owned());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(bounds.side);
    limits.max_image_height = Some(bounds.side);
    limits.max_alloc = Some(bounds.alloc);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("the picture could not be decoded: {error}"))
}

#[cfg(test)]
mod tests;
