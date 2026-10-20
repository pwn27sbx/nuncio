//! Inline pictures in the conversation (T15 Part 2.5): how the terminal is asked which graphics
//! protocol it speaks, how big a picture's box is, and the cache and workers that turn a picture
//! into an encoded `ratatui-image` protocol off the UI thread.
//!
//! A picture message (an `image` or a `sticker`) that states its size gets a box of fixed height in
//! the transcript before anything is decoded, so nothing moves when the thumbnail, or later the full
//! picture, is drawn in it. The thumbnail is the one WhatsApp embeds in the message (the wire's
//! `thumb`); the full picture is fetched only when the user asks (`o`). Every byte decoded here is
//! untrusted: a picture that is not a JPEG, PNG or WebP, is larger than its bounds, or does not
//! decode keeps its box with a label, and nothing panics.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader, Limits, Rgba, RgbaImage, imageops};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FilterType, FontSize, Resize};

/// The environment variable that chooses the protocol instead of asking the terminal.
pub const OVERRIDE: &str = "NUNCIO_IMAGE_PROTOCOL";
/// The tallest a picture's box gets, in rows.
pub const IMAGE_ROWS: u16 = 12;
/// How many encoded pictures the cache keeps.
pub const CACHE_ENTRIES: usize = 48;
/// About how many bytes the encoded pictures in the cache may take (estimated as the pixels drawn,
/// four bytes each).
pub const CACHE_BYTES: usize = 96 << 20;
/// How many pictures are encoded at once, each on its own worker thread.
const WORKERS: usize = 2;
/// A full picture file larger than this is not decoded (WhatsApp sends far smaller ones).
const MAX_FILE: u64 = 64 << 20;
/// The largest side a picture may have, as the wire bounds `width` and `height`.
const MAX_SIDE: u32 = 16_384;
/// What a decode may cost: its largest side and its allocation. A thumbnail is tiny on the wire
/// but its header can declare any size, so it gets far tighter bounds than a full picture (a
/// 4096x4096 RGBA photo is 64 MiB).
#[derive(Clone, Copy)]
struct Bounds {
    side: u32,
    alloc: u64,
}

const THUMB_BOUNDS: Bounds = Bounds {
    side: 1024,
    alloc: 16 << 20,
};
const FULL_BOUNDS: Bounds = Bounds {
    side: MAX_SIDE,
    alloc: 160 << 20,
};
/// The cell size assumed when the terminal reports none.
const FALLBACK_FONT: FontSize = FontSize::new(10, 20);

/// A protocol named in [`OVERRIDE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolChoice {
    /// Kitty graphics with Unicode placeholders (kitty, Ghostty).
    Kitty,
    /// Sixel (foot, zellij).
    Sixel,
    /// The iTerm2 inline image protocol (iTerm2, WezTerm).
    Iterm2,
    /// Coloured half blocks: every terminal.
    Halfblocks,
    /// No pictures: messages show their `[image]` label only and `o` opens them externally.
    Off,
}

/// The value of [`OVERRIDE`]: `kitty`, `sixel`, `iterm2`, `halfblocks` or `off` (any case, spaces
/// around it ignored).
pub fn parse_protocol(value: &str) -> Result<ProtocolChoice, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "kitty" => Ok(ProtocolChoice::Kitty),
        "sixel" => Ok(ProtocolChoice::Sixel),
        "iterm2" => Ok(ProtocolChoice::Iterm2),
        "halfblocks" => Ok(ProtocolChoice::Halfblocks),
        "off" => Ok(ProtocolChoice::Off),
        _ => Err(format!(
            "{OVERRIDE} must be kitty, sixel, iterm2, halfblocks or off"
        )),
    }
}

/// How the protocol is chosen at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Use `choice` without asking the terminal anything; `why` says what decided it.
    Fixed {
        choice: ProtocolChoice,
        why: &'static str,
    },
    /// Ask the terminal (ratatui-image's query). Under zellij only sixel or half blocks are used
    /// ([`zellij_protocol`]).
    Query { zellij: bool },
}

/// Decide how to choose the protocol from the [`OVERRIDE`] value, whether `ZELLIJ` is set, and
/// whether stdin and stdout are both a terminal. The override wins. Otherwise only a terminal is
/// queried: ratatui-image 11.1 reads the answer from stdin on a thread it never stops when no
/// answer comes (issue #202), so with stdin not a terminal (where crossterm reads `/dev/tty`
/// instead) the answer could never arrive and half blocks are used. A value that is not one of the
/// five is ignored, and the second value returned says so.
pub fn plan(override_value: Option<&str>, zellij: bool, terminal: bool) -> (Plan, Option<String>) {
    let mut warning = None;
    if let Some(value) = override_value {
        match parse_protocol(value) {
            Ok(choice) => {
                return (
                    Plan::Fixed {
                        choice,
                        why: OVERRIDE,
                    },
                    None,
                );
            }
            Err(error) => warning = Some(format!("{error}; asking the terminal instead")),
        }
    }
    if !terminal {
        return (
            Plan::Fixed {
                choice: ProtocolChoice::Halfblocks,
                why: "stdin is not a terminal",
            },
            warning,
        );
    }
    (Plan::Query { zellij }, warning)
}

/// The protocol to use under zellij, given what the query detected and whether the terminal
/// announced sixel: zellij rejects kitty's Unicode placeholders and does not pass iTerm2's
/// sequence on, so it is sixel when announced, else half blocks.
pub fn zellij_protocol(detected: ProtocolType, sixel: bool) -> ProtocolType {
    if detected == ProtocolType::Sixel || sixel {
        ProtocolType::Sixel
    } else {
        ProtocolType::Halfblocks
    }
}

/// Whether the startup query may have left ratatui-image's reader thread blocked on stdin: it took
/// at least its `timeout` and the terminal never `answered` (an answer means the reader got to its
/// end). Before the timeout, the query only returns once its reader is done.
pub fn query_left_stdin_unsafe(elapsed: Duration, timeout: Duration, answered: bool) -> bool {
    !answered && elapsed >= timeout
}

/// Take Rust's stdin lock on a thread and keep it for the rest of the process; the receiver gets
/// `()` once it is taken. ratatui-image's query reads through that lock, so a reader it left
/// blocked can finish at most the read it is in (the terminal's late answer, or one key) and never
/// reads again. crossterm reads the terminal's file descriptor directly, without that lock, so the
/// TUI's own input is not affected.
pub fn hold_stdin() -> Receiver<()> {
    let (taken, receiver) = mpsc::channel();
    thread::spawn(move || {
        std::mem::forget(std::io::stdin().lock());
        let _ = taken.send(());
    });
    receiver
}

/// Throw away whatever the terminal sent that nobody read yet: after a query that timed out, a late
/// answer split across reads would otherwise reach the TUI as keys (`=31;OK\` in the composer).
pub fn discard_pending_input() {
    // SAFETY: tcflush only takes a file descriptor and a constant; on a descriptor that is not a
    // terminal it fails with ENOTTY and does nothing.
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

/// The protocol the transcript draws pictures with, as chosen at startup.
#[derive(Clone)]
pub struct Graphics {
    picker: Picker,
    how: String,
}

impl Graphics {
    /// Draw with `picker`; `how` says what chose it (shown once in the footer).
    pub fn new(picker: Picker, how: impl Into<String>) -> Self {
        Self {
            picker,
            how: how.into(),
        }
    }

    /// Half blocks at a 10x20 cell: deterministic, for every terminal (and the tests).
    pub fn halfblocks() -> Self {
        Self::new(Picker::halfblocks(), "half blocks")
    }

    pub fn picker(&self) -> &Picker {
        &self.picker
    }

    /// The protocol's name: `kitty`, `sixel`, `iTerm2` or `half blocks`.
    pub fn protocol_name(&self) -> &'static str {
        protocol_name(self.picker.protocol_type())
    }

    /// The footer's one-time line, e.g. `pictures: kitty (detected)`.
    pub fn notice(&self) -> String {
        format!("pictures: {} ({})", self.protocol_name(), self.how)
    }
}

pub fn protocol_name(protocol: ProtocolType) -> &'static str {
    match protocol {
        ProtocolType::Kitty => "kitty",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Iterm2 => "iTerm2",
        ProtocolType::Halfblocks => "half blocks",
    }
}

/// The box a `width` x `height` pixel picture gets in a transcript `columns` wide, at the
/// terminal's cell size `font`: as wide as it is in cells but no wider than the column, as tall as
/// that width keeps its shape, at most [`IMAGE_ROWS`] rows (the width then shrinks to keep the
/// shape), never empty. Sizes outside 1..=16384 are clamped, and a cell size of zero is taken as
/// 10x20.
pub fn box_size(width: u32, height: u32, columns: u16, font: FontSize) -> Size {
    let font = if font.width == 0 || font.height == 0 {
        FALLBACK_FONT
    } else {
        font
    };
    let (cell_w, cell_h) = (u64::from(font.width), u64::from(font.height));
    let w = u64::from(width.clamp(1, MAX_SIDE));
    let h = u64::from(height.clamp(1, MAX_SIDE));
    let mut cols = w.div_ceil(cell_w).min(u64::from(columns.max(1))).max(1);
    let mut rows = (h * cols * cell_w)
        .div_ceil(w * cell_h)
        .clamp(1, h.div_ceil(cell_h).max(1));
    let cap = u64::from(IMAGE_ROWS);
    if rows > cap {
        rows = cap;
        cols = (rows * cell_h * w).div_ceil(h * cell_w).clamp(1, cols);
    }
    // Both are bounded by `columns` and IMAGE_ROWS, so they fit.
    Size::new(cols as u16, rows as u16)
}

/// Which picture of a message: the embedded thumbnail or the full file fetched with `o`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    Thumb,
    Full,
}

/// One encoded picture in the cache: a message's picture at a transcript width.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub chat: String,
    pub id: String,
    pub variant: Variant,
    pub columns: u16,
}

impl Key {
    fn picture(&self) -> (String, String, Variant) {
        (self.chat.clone(), self.id.clone(), self.variant)
    }
}

/// Where a picture's bytes are.
pub(crate) enum Origin {
    /// The wire's base64 `thumb`.
    Base64(String),
    /// A file `download_media` wrote.
    File(PathBuf),
}

struct Job {
    /// The [`Images::generation`] it was queued in: a later resize makes it stale.
    generation: u64,
    key: Key,
    origin: Origin,
    /// The box; `None` for a picture whose message states no size (taken from the decoded one).
    size: Option<Size>,
    /// The colour behind the picture, for a protocol that cannot draw transparency.
    backdrop: Option<[u8; 3]>,
}

struct Encoded {
    protocol: SlicedProtocol,
    pixels: (u32, u32),
    size: Size,
}

struct Done {
    key: Key,
    outcome: Result<Encoded, String>,
}

struct Entry {
    protocol: SlicedProtocol,
    bytes: usize,
    used: u64,
}

/// The encoded pictures (an LRU bounded by [`CACHE_ENTRIES`] and [`CACHE_BYTES`]), the pictures
/// being encoded, the ones that cannot be shown, and the [`WORKERS`] threads that encode them.
pub(crate) struct Images {
    font: FontSize,
    cache: HashMap<Key, Entry>,
    bytes: usize,
    clock: u64,
    pending: HashSet<Key>,
    /// Pictures that cannot be shown, with why; by chat, id and variant, since a decode error
    /// does not depend on the width.
    failed: HashMap<(String, String, Variant), String>,
    /// The size of a full picture once decoded, for a message that stated none.
    pixels: HashMap<(String, String), (u32, u32)>,
    /// The transcript width the cache holds pictures for.
    columns: Option<u16>,
    /// Half blocks paint every cell, so a transparent pixel is drawn in the conversation's
    /// background colour, which the cache's pictures are encoded on; `None` for the protocols
    /// that keep the alpha (kitty, sixel, iTerm2).
    backdrop: Option<[u8; 3]>,
    halfblocks: bool,
    encodes: u64,
    /// Bumped by every resize; a worker skips a job queued before it.
    generation: Arc<AtomicU64>,
    /// How many jobs the workers ran (skipped ones not counted).
    jobs_run: Arc<AtomicU64>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Images {
    pub(crate) fn new(graphics: &Graphics) -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let queue = Arc::new(Mutex::new(queue));
        let (report, done) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let jobs_run = Arc::new(AtomicU64::new(0));
        for _ in 0..WORKERS {
            let worker = Worker {
                picker: graphics.picker.clone(),
                queue: Arc::clone(&queue),
                report: report.clone(),
                generation: Arc::clone(&generation),
                jobs_run: Arc::clone(&jobs_run),
            };
            // Without a worker nothing is encoded and every box keeps its label: still usable.
            let _ = thread::Builder::new()
                .name("nuncio-pictures".to_owned())
                .spawn(move || worker.run());
        }
        let font = graphics.picker.font_size();
        Self {
            font: if font.width == 0 || font.height == 0 {
                FALLBACK_FONT
            } else {
                font
            },
            cache: HashMap::new(),
            bytes: 0,
            clock: 0,
            pending: HashSet::new(),
            failed: HashMap::new(),
            pixels: HashMap::new(),
            columns: None,
            backdrop: None,
            halfblocks: graphics.picker.protocol_type() == ProtocolType::Halfblocks,
            encodes: 0,
            generation,
            jobs_run,
            jobs,
            done,
        }
    }

    pub(crate) fn font(&self) -> FontSize {
        self.font
    }

    pub(crate) fn encodes(&self) -> u64 {
        self.encodes
    }

    pub(crate) fn jobs_run(&self) -> u64 {
        self.jobs_run.load(Ordering::SeqCst)
    }

    pub(crate) fn len(&self) -> usize {
        self.cache.len()
    }

    /// The transcript is `columns` wide now: a different width drops every encoded picture (they
    /// are encoded for their box, which follows the width) and forgets what was being encoded.
    pub(crate) fn resized(&mut self, columns: u16) {
        if self.columns != Some(columns) {
            self.columns = Some(columns);
            // Everything queued for the old width is dropped by the workers unencoded.
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.cache.clear();
            self.bytes = 0;
            self.pending.clear();
        }
    }

    /// The conversation's background is `colour` now. With half blocks a different one drops every
    /// encoded picture, like a resize, since their transparent pixels were painted in the old one;
    /// the other protocols keep the alpha and ignore it.
    pub(crate) fn backdrop(&mut self, colour: [u8; 3]) {
        if self.halfblocks && self.backdrop != Some(colour) {
            self.backdrop = Some(colour);
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.cache.clear();
            self.bytes = 0;
            self.pending.clear();
        }
    }

    /// The encoded picture, marked as just used.
    pub(crate) fn get(&mut self, key: &Key) -> Option<&SlicedProtocol> {
        self.clock += 1;
        let clock = self.clock;
        self.cache.get_mut(key).map(|entry| {
            entry.used = clock;
            &entry.protocol
        })
    }

    pub(crate) fn is_pending(&self, key: &Key) -> bool {
        self.pending.contains(key)
    }

    pub(crate) fn has_failed(&self, chat: &str, id: &str, variant: Variant) -> bool {
        self.failed
            .contains_key(&(chat.to_owned(), id.to_owned(), variant))
    }

    /// Forget that the full picture could not be shown (the user asked for it again).
    pub(crate) fn retry_full(&mut self, chat: &str, id: &str) {
        self.failed
            .remove(&(chat.to_owned(), id.to_owned(), Variant::Full));
    }

    /// The decoded size of a full picture, once known.
    pub(crate) fn full_pixels(&self, chat: &str, id: &str) -> Option<(u32, u32)> {
        self.pixels.get(&(chat.to_owned(), id.to_owned())).copied()
    }

    /// Whether `key` still has to be asked for: not encoded, not being encoded, not failed.
    pub(crate) fn wants(&self, key: &Key) -> bool {
        !self.cache.contains_key(key)
            && !self.pending.contains(key)
            && !self.failed.contains_key(&key.picture())
    }

    /// Encode `key` from `origin` into a box of `size` on a worker, unless it is already done,
    /// under way or failed.
    pub(crate) fn request(&mut self, key: Key, origin: Origin, size: Option<Size>) {
        if !self.wants(&key) {
            return;
        }
        self.pending.insert(key.clone());
        let generation = self.generation.load(Ordering::SeqCst);
        let job = Job {
            generation,
            key,
            origin,
            size,
            backdrop: self.backdrop,
        };
        if self.jobs.send(job).is_err() {
            self.pending.clear();
        }
    }

    /// Take in every encoded picture the workers finished. Returns why each full picture that
    /// cannot be shown failed (a thumbnail that fails just keeps its label).
    pub(crate) fn collect(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while let Ok(done) = self.done.try_recv() {
            self.take(done, &mut failures);
        }
        failures
    }

    /// Wait until nothing is being encoded, or `timeout`; `true` when nothing is.
    pub(crate) fn wait(&mut self, timeout: Duration) -> (bool, Vec<String>) {
        let deadline = Instant::now() + timeout;
        let mut failures = self.collect();
        while !self.pending.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.done.recv_timeout(left) {
                Ok(done) => self.take(done, &mut failures),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        (self.pending.is_empty(), failures)
    }

    fn take(&mut self, done: Done, failures: &mut Vec<String>) {
        // A result for a width the transcript no longer has, or one forgotten, is dropped.
        if !self.pending.remove(&done.key) {
            return;
        }
        match done.outcome {
            Ok(encoded) => {
                if done.key.variant == Variant::Full {
                    self.pixels
                        .insert((done.key.chat.clone(), done.key.id.clone()), encoded.pixels);
                }
                let bytes = usize::from(encoded.size.width)
                    * usize::from(encoded.size.height)
                    * usize::from(self.font.width)
                    * usize::from(self.font.height)
                    * 4;
                self.clock += 1;
                self.encodes += 1;
                self.bytes += bytes;
                if let Some(old) = self.cache.insert(
                    done.key,
                    Entry {
                        protocol: encoded.protocol,
                        bytes,
                        used: self.clock,
                    },
                ) {
                    self.bytes -= old.bytes;
                }
                self.evict();
            }
            Err(error) => {
                if done.key.variant == Variant::Full {
                    failures.push(error.clone());
                }
                self.failed.insert(done.key.picture(), error);
            }
        }
    }

    /// Drop the least recently drawn pictures until the cache is within its bounds.
    fn evict(&mut self) {
        while self.cache.len() > CACHE_ENTRIES || (self.bytes > CACHE_BYTES && self.cache.len() > 1)
        {
            let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            if let Some(entry) = self.cache.remove(&oldest) {
                self.bytes -= entry.bytes;
            }
        }
    }
}

/// One encoding thread and what it shares with [`Images`].
struct Worker {
    picker: Picker,
    queue: Arc<Mutex<Receiver<Job>>>,
    report: Sender<Done>,
    generation: Arc<AtomicU64>,
    jobs_run: Arc<AtomicU64>,
}

impl Worker {
    /// Encode jobs until the [`Images`] that sends them is gone, skipping the stale ones.
    fn run(self) {
        loop {
            let job = {
                let Ok(queue) = self.queue.lock() else {
                    return;
                };
                match queue.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            };
            // Queued for a width the transcript no longer has: its result would be dropped.
            if job.generation != self.generation.load(Ordering::SeqCst) {
                continue;
            }
            self.jobs_run.fetch_add(1, Ordering::SeqCst);
            let Job {
                key,
                origin,
                size,
                backdrop,
                ..
            } = job;
            let columns = key.columns;
            let bounds = match key.variant {
                Variant::Thumb => THUMB_BOUNDS,
                Variant::Full => FULL_BOUNDS,
            };
            // The decoders are meant never to panic on bad input; if one does, this worker
            // survives and the picture keeps its label.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                encode(&self.picker, origin, size, columns, bounds, backdrop)
            }))
            .unwrap_or_else(|_| Err("the picture could not be decoded".to_owned()));
            if self.report.send(Done { key, outcome }).is_err() {
                return;
            }
        }
    }
}

/// Decode a picture and encode it for the terminal, scaled to fit its box. With a `backdrop` the
/// picture is laid on that colour first (half blocks would paint a transparent pixel black), and
/// so is any margin the box leaves around it.
fn encode(
    picker: &Picker,
    origin: Origin,
    size: Option<Size>,
    columns: u16,
    bounds: Bounds,
    backdrop: Option<[u8; 3]>,
) -> Result<Encoded, String> {
    let bytes = match origin {
        Origin::Base64(text) => base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| "the thumbnail is not base64".to_owned())?,
        Origin::File(path) => read_capped(&path)?,
    };
    let picture = decode(&bytes, bounds)?;
    let pixels = (picture.width(), picture.height());
    let font = picker.font_size();
    let size = size.unwrap_or_else(|| box_size(pixels.0, pixels.1, columns, font));
    // Shrink to the box's pixels at once, so the encoder never works on (or copies) more.
    let (box_w, box_h) = (
        u32::from(size.width) * u32::from(font.width.max(1)),
        u32::from(size.height) * u32::from(font.height.max(1)),
    );
    let picture = if picture.width() > box_w || picture.height() > box_h {
        picture.resize(box_w.max(1), box_h.max(1), FilterType::Triangle)
    } else {
        picture
    };
    let mut on_backdrop = None;
    let picture = match backdrop {
        Some([r, g, b]) => {
            let colour = Rgba([r, g, b, 255]);
            let mut base = RgbaImage::from_pixel(picture.width(), picture.height(), colour);
            imageops::overlay(&mut base, &picture.to_rgba8(), 0, 0);
            let mut picker = picker.clone();
            picker.set_background_color(Some(colour));
            on_backdrop = Some(picker);
            DynamicImage::ImageRgba8(base)
        }
        None => picture,
    };
    let protocol = SlicedProtocol::new_with_resize(
        on_backdrop.as_ref().unwrap_or(picker),
        picture,
        size,
        Resize::Scale(Some(FilterType::Triangle)),
    )
    .map_err(|error| format!("the picture could not be encoded: {error}"))?;
    Ok(Encoded {
        protocol,
        pixels,
        size,
    })
}

/// Read a downloaded picture, refusing one larger than [`MAX_FILE`].
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
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

/// Decode untrusted bytes: only JPEG, PNG and WebP (an animated WebP gives its first frame), within
/// `bounds`. The size the header declares is checked first, so a small file declaring a huge
/// picture is refused without decoding anything.
fn decode(bytes: &[u8], bounds: Bounds) -> Result<DynamicImage, String> {
    let too_large = || "the picture is larger than nuncio draws".to_owned();
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "the picture could not be read".to_owned())?
        .into_dimensions()
        .map_err(|error| format!("the picture could not be read: {error}"))?;
    if width > bounds.side
        || height > bounds.side
        || u64::from(width) * u64::from(height) * 4 > bounds.alloc
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
