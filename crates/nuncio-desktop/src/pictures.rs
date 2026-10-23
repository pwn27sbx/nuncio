//! Pictures inside the conversation, without GTK: the box an image or sticker bubble reserves,
//! what to decode for a bubble, the two decoding threads, and the cache of what was decoded. The
//! window ([`crate::window`]) turns each decoded picture into a `gdk::Texture` and keeps them in a
//! [`Cache`] of textures.
//!
//! The rules are nuncio-tui's (T15 Part 2.5), with its decode rules shared through
//! [`nuncio_client::pictures`]: the thumbnail the message embeds is drawn at once, in a box sized
//! from the `width` and `height` the message states, so nothing moves when it (or later the full
//! picture) arrives; the full file is fetched only when the user asks (a click, Enter, `o`, or
//! Open in the message's menu), never by itself; every byte is untrusted, and a picture that does
//! not decode or is too large keeps its label. A sticker's transparent pixels show the bubble
//! behind it. An animated sticker plays only once its full picture was fetched: its frames are
//! decoded at its box within [`ANIMATION_BYTES`] and the one to show at a time is [`frame_at`]'s;
//! its thumbnail, and a sticker whose frames cannot all be kept, give the first frame.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use image::imageops::FilterType;
use nuncio_client::Model;
use nuncio_client::pictures::{
    FRAME_MIN_BYTES, decode, decode_frames, fit_box, read_capped, thumb_bytes,
};

pub use nuncio_client::pictures::Variant;

use crate::present::Bubble;

/// The largest box a photo gets, in pixels (each side).
pub const PHOTO_BOX: u32 = 300;
/// The largest box a sticker gets, in pixels (each side).
pub const STICKER_BOX: u32 = 160;
/// A decoded picture larger than its box times this is shrunk to it (sharp on a 2x screen).
pub const SCALE: u32 = 2;
/// How many decoded pictures the cache keeps: more than a conversation loads (200 messages).
pub const CACHE_ENTRIES: usize = 256;
/// About how many bytes the cache's pictures may take (four per pixel).
pub const CACHE_BYTES: usize = 64 << 20;
/// The most bytes an animated sticker's frames may keep (four per pixel, after shrinking to its
/// box, not [`SCALE`] times it, and at least [`FRAME_MIN_BYTES`] a frame); one whose frames need
/// more shows its first frame only, at [`SCALE`] times its box as a still sticker. A 512 px
/// sticker in its 160 px box keeps 160x160 frames (100 KiB each): 163 fit.
pub const ANIMATION_BYTES: usize = 16 << 20;
/// How many pictures are decoded at once, each on its own thread.
pub const WORKERS: usize = 2;

/// The box a `width` x `height` picture reserves in its bubble: its shape, at most
/// [`PHOTO_BOX`] (or [`STICKER_BOX`]) on each side, never larger than the picture, never empty.
pub fn picture_box(width: u32, height: u32, sticker: bool) -> (u32, u32) {
    let side = if sticker { STICKER_BOX } else { PHOTO_BOX };
    fit_box(width, height, side, side, 1, 1)
}

/// The box of a picture whose message states no size, from its first decoded image: a
/// thumbnail's shape scaled up to the caps of [`picture_box`] (a thumbnail is smaller than the
/// picture it stands for), a full picture's own [`picture_box`].
pub fn first_image_box(variant: Variant, source: (u32, u32), sticker: bool) -> (u32, u32) {
    let (width, height) = (source.0.max(1), source.1.max(1));
    match variant {
        Variant::Full => picture_box(width, height, sticker),
        Variant::Thumb => {
            let side = if sticker { STICKER_BOX } else { PHOTO_BOX };
            let scale = side.div_ceil(width.max(height)).max(1);
            picture_box(
                width.saturating_mul(scale),
                height.saturating_mul(scale),
                sticker,
            )
        }
    }
}

/// How many boxes of pictures with no stated size are remembered.
const BOX_ENTRIES: usize = 4096;

/// One decoded picture: a message's thumbnail or full picture.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub chat: String,
    pub id: String,
    pub variant: Variant,
}

impl Key {
    pub fn new(chat: &str, id: &str, variant: Variant) -> Self {
        Self {
            chat: chat.to_owned(),
            id: id.to_owned(),
            variant,
        }
    }
}

/// Where a picture's bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The wire's base64 `thumb`.
    Base64(String),
    /// The file `download_media` wrote.
    File(PathBuf),
}

/// A picture to decode, to fit `target` (its box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub key: Key,
    pub origin: Origin,
    pub target: (u32, u32),
    /// Decode its frames: the full picture of an animated sticker.
    pub animated: bool,
}

/// A decoded picture: `width` x `height` RGBA pixels, four bytes each, not premultiplied. An
/// animation has more frames of that size after `rgba`, its first.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    /// The size it had before it was shrunk to its box.
    pub source: (u32, u32),
    pub rgba: Vec<u8>,
    /// How long each frame shows, in milliseconds: one per frame, `rgba` first; empty for a
    /// picture that does not animate.
    pub delays_ms: Vec<u32>,
    /// The frames after the first.
    pub later: Vec<Vec<u8>>,
}

impl Pixels {
    /// The bytes all its frames take: a still picture its pixels, an animation each frame's
    /// pixels but at least [`FRAME_MIN_BYTES`] (its texture), as [`decode_frames`] charged them.
    pub fn bytes(&self) -> usize {
        if self.later.is_empty() {
            return self.rgba.len();
        }
        let least = FRAME_MIN_BYTES as usize;
        std::iter::once(&self.rgba)
            .chain(&self.later)
            .map(|frame| frame.len().max(least))
            .sum()
    }
}

impl std::fmt::Debug for Pixels {
    // Never the bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pixels")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// A decode's result.
#[derive(Debug)]
pub struct Decoded {
    pub key: Key,
    pub outcome: Result<Pixels, String>,
}

/// What `bubble` needs decoded now, given what `cache` holds: its full picture once the user had
/// it fetched, and its thumbnail unless the full picture is already there. Nothing for a bubble
/// with no picture, for what is decoded, being decoded or failed, and never a download: the full
/// picture is decoded only from the file a requested download wrote.
pub fn requests<T: Clone>(model: &Model, bubble: &Bubble, cache: &Cache<T>) -> Vec<Request> {
    let Some(picture) = &bubble.picture else {
        return Vec::new();
    };
    let target = picture
        .size
        .or_else(|| cache.known_box(&picture.chat, &picture.id))
        .unwrap_or_else(|| {
            let side = if picture.sticker {
                STICKER_BOX
            } else {
                PHOTO_BOX
            };
            (side, side)
        });
    let mut wanted = Vec::new();
    let full = Key::new(&picture.chat, &picture.id, Variant::Full);
    if let Some(path) = &picture.full
        && cache.wants(&full)
    {
        wanted.push(Request {
            key: full.clone(),
            origin: Origin::File(path.clone()),
            target,
            animated: picture.animated,
        });
    }
    let thumb = Key::new(&picture.chat, &picture.id, Variant::Thumb);
    let text = model
        .loaded()
        .get(bubble.index)
        .filter(|message| message.id == picture.id)
        .and_then(|message| message.thumb.clone());
    if let Some(text) = text
        && !cache.contains(&full)
        && cache.wants(&thumb)
    {
        wanted.insert(
            0,
            Request {
                key: thumb,
                origin: Origin::Base64(text),
                target,
                animated: false,
            },
        );
    }
    wanted
}

/// Decode `request`'s untrusted bytes within its variant's bounds, and shrink the picture to at
/// most [`SCALE`] times its box (keeping its shape) so a large photo costs no more than its box.
/// An animated request gives every frame, each shrunk to at most its box (once, not [`SCALE`]
/// times: more frames fit), when they keep at most [`ANIMATION_BYTES`]; else (as any other) its
/// first frame at [`SCALE`] times its box.
pub fn decode_picture(request: &Request) -> Result<Pixels, String> {
    let bytes = match &request.origin {
        Origin::Base64(text) => thumb_bytes(text)?,
        Origin::File(path) => read_capped(path)?,
    };
    let bounds = request.key.variant.bounds();
    let (box_w, box_h) = (request.target.0.max(1), request.target.1.max(1));
    let (max_w, max_h) = (box_w * SCALE, box_h * SCALE);
    let frames = request
        .animated
        .then(|| decode_frames(&bytes, bounds, (box_w, box_h), ANIMATION_BYTES as u64))
        .flatten();
    if let Some(animation) = frames {
        let (first, _) = &animation.frames[0];
        let (width, height) = first.dimensions();
        let delays_ms = animation.frames.iter().map(|(_, delay)| *delay).collect();
        let mut frames = animation
            .frames
            .into_iter()
            .map(|(image, _)| image.into_raw());
        return Ok(Pixels {
            width,
            height,
            source: animation.canvas,
            rgba: frames.next().unwrap_or_default(),
            delays_ms,
            later: frames.collect(),
        });
    }
    let picture = decode(&bytes, bounds)?;
    let source = (picture.width(), picture.height());
    let picture = if picture.width() > max_w || picture.height() > max_h {
        picture.resize(max_w, max_h, FilterType::Triangle)
    } else {
        picture
    };
    let rgba = picture.into_rgba8();
    Ok(Pixels {
        width: rgba.width(),
        height: rgba.height(),
        source,
        rgba: rgba.into_raw(),
        delays_ms: Vec::new(),
        later: Vec::new(),
    })
}

/// The frame of an animation to show `elapsed_ms` after it started, each frame shown for its
/// delay in `delays_ms`, looping forever; the first when there are no frames or no time.
pub fn frame_at(delays_ms: &[u32], elapsed_ms: u64) -> usize {
    let total: u64 = delays_ms.iter().map(|&delay| u64::from(delay)).sum();
    if total == 0 {
        return 0;
    }
    let mut left = elapsed_ms % total;
    for (index, &delay) in delays_ms.iter().enumerate() {
        let delay = u64::from(delay);
        if left < delay {
            return index;
        }
        left -= delay;
    }
    0
}

/// The [`WORKERS`] decoding threads. Dropping it ends them once they finish what they decode.
pub struct Decoder {
    jobs: Sender<Request>,
}

impl Decoder {
    /// Start the threads; each result is handed to `post` (on a decoding thread).
    pub fn spawn(post: impl Fn(Decoded) + Send + Clone + 'static) -> Self {
        let (jobs, queue) = mpsc::channel::<Request>();
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..WORKERS {
            let (queue, post) = (Arc::clone(&queue), post.clone());
            // Without a thread nothing is decoded and every picture keeps its label: still usable.
            let _ = thread::Builder::new()
                .name("nuncio-pictures".to_owned())
                .spawn(move || work(&queue, &post));
        }
        Self { jobs }
    }

    /// Decode `request` on the next free thread.
    pub fn decode(&self, request: Request) {
        let _ = self.jobs.send(request);
    }
}

fn work(queue: &Mutex<Receiver<Request>>, post: &impl Fn(Decoded)) {
    loop {
        let request = {
            let Ok(queue) = queue.lock() else {
                return;
            };
            match queue.recv() {
                Ok(request) => request,
                Err(_) => return,
            }
        };
        // The decoders are meant never to panic on bad input; if one does, this thread survives
        // and the picture keeps its label.
        let outcome = std::panic::catch_unwind(|| decode_picture(&request))
            .unwrap_or_else(|_| Err("the picture could not be decoded".to_owned()));
        post(Decoded {
            key: request.key,
            outcome,
        });
    }
}

struct Entry<T> {
    value: T,
    bytes: usize,
    used: u64,
}

/// The decoded pictures (a least-recently-used cache bounded by [`CACHE_ENTRIES`] and
/// [`CACHE_BYTES`]), the ones being decoded and the ones that cannot be shown.
pub struct Cache<T> {
    entries: HashMap<Key, Entry<T>>,
    bytes: usize,
    clock: u64,
    pending: HashSet<Key>,
    /// Pictures that cannot be shown: asked for again only after [`Cache::retry_full`].
    failed: HashSet<Key>,
    /// The size of a full picture once decoded, for a message that stated none.
    pixels: HashMap<(String, String), (u32, u32)>,
    /// The box a picture whose message states no size took from its first image: kept when its
    /// pictures leave the cache, so it never changes again.
    boxes: HashMap<(String, String), (u32, u32)>,
    entry_limit: usize,
    byte_limit: usize,
}

impl<T: Clone> Default for Cache<T> {
    fn default() -> Self {
        Self::with_limits(CACHE_ENTRIES, CACHE_BYTES)
    }
}

impl<T: Clone> Cache<T> {
    /// A cache of at most `entries` pictures and about `bytes` bytes (at least one picture).
    pub fn with_limits(entries: usize, bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
            pending: HashSet::new(),
            failed: HashSet::new(),
            pixels: HashMap::new(),
            boxes: HashMap::new(),
            entry_limit: entries.max(1),
            byte_limit: bytes,
        }
    }

    /// The decoded picture, marked as just used.
    pub fn get(&mut self, key: &Key) -> Option<T> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|entry| {
            entry.used = clock;
            entry.value.clone()
        })
    }

    /// The best picture of a message there is: the full one, else the thumbnail.
    pub fn best(&mut self, chat: &str, id: &str) -> Option<(T, Variant)> {
        [Variant::Full, Variant::Thumb]
            .into_iter()
            .find_map(|variant| Some((self.get(&Key::new(chat, id, variant))?, variant)))
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.entries.contains_key(key)
    }

    pub fn is_pending(&self, key: &Key) -> bool {
        self.pending.contains(key)
    }

    pub fn has_failed(&self, key: &Key) -> bool {
        self.failed.contains(key)
    }

    /// Whether `key` still has to be decoded: not decoded, not being decoded, not failed.
    pub fn wants(&self, key: &Key) -> bool {
        !self.entries.contains_key(key) && !self.pending.contains(key) && !self.failed.contains(key)
    }

    /// `key` is being decoded now; false when it is not wanted (see [`Cache::wants`]).
    pub fn start(&mut self, key: &Key) -> bool {
        self.wants(key) && self.pending.insert(key.clone())
    }

    /// Take a decode's result in: `value` of `bytes` bytes from a picture of `source` pixels, or
    /// why it failed. False when nothing was waiting for it (it was forgotten).
    pub fn finish(&mut self, key: Key, outcome: Result<(T, usize, (u32, u32)), String>) -> bool {
        if !self.pending.remove(&key) {
            return false;
        }
        match outcome {
            Ok((value, bytes, source)) => {
                if key.variant == Variant::Full {
                    self.pixels
                        .insert((key.chat.clone(), key.id.clone()), source);
                }
                self.clock += 1;
                self.bytes += bytes;
                let entry = Entry {
                    value,
                    bytes,
                    used: self.clock,
                };
                if let Some(old) = self.entries.insert(key, entry) {
                    self.bytes -= old.bytes;
                }
                self.evict();
            }
            Err(_) => {
                self.failed.insert(key);
            }
        }
        true
    }

    /// The user asked for the full picture again: one that failed may be decoded once more.
    pub fn retry_full(&mut self, chat: &str, id: &str) {
        self.failed.remove(&Key::new(chat, id, Variant::Full));
    }

    /// The box of a picture whose message states no size, once its first image decoded.
    pub fn known_box(&self, chat: &str, id: &str) -> Option<(u32, u32)> {
        self.boxes.get(&(chat.to_owned(), id.to_owned())).copied()
    }

    /// The box of a picture whose message states no size: the one it already has, else the one
    /// its first decoded image (`variant`, `source` pixels) gives it ([`first_image_box`]),
    /// remembered from now on.
    pub fn settle_box(&mut self, key: &Key, source: (u32, u32), sticker: bool) -> (u32, u32) {
        let id = (key.chat.clone(), key.id.clone());
        if let Some(known) = self.boxes.get(&id) {
            return *known;
        }
        if self.boxes.len() >= BOX_ENTRIES {
            self.boxes.clear();
        }
        let fixed = first_image_box(key.variant, source, sticker);
        self.boxes.insert(id, fixed);
        fixed
    }

    /// Forget every decoded picture (what eviction does to them all), keeping the boxes, what
    /// failed and what is being decoded.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// The decoded size of a full picture, once known.
    pub fn full_pixels(&self, chat: &str, id: &str) -> Option<(u32, u32)> {
        self.pixels.get(&(chat.to_owned(), id.to_owned())).copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drop the least recently used pictures until the cache is within its bounds, keeping the
    /// newest even when it alone is over the byte bound.
    fn evict(&mut self) {
        while self.entries.len() > self.entry_limit
            || (self.bytes > self.byte_limit && self.entries.len() > 1)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests;
