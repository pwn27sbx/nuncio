//! Following the Omarchy theme.
//!
//! Omarchy writes the active theme's palette to
//! `~/.local/state/omarchy/current/theme/colors.toml`: a flat file of
//! `key = "#rrggbb"` entries plus `mode = "dark" | "light"`. This module parses it into a
//! small [`Palette`] so both clients can colour themselves like the rest of the desktop.
//!
//! Parsing lives here, and so does the live watch ([`ThemeWatcher`]): following the theme is
//! client-side work that the TUI and the desktop app both need, so it has one implementation.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use notify::Watcher as _;

/// A 24-bit colour, as written in `colors.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb` — the only form Omarchy writes, in either case.
    ///
    /// Returns `None` for anything else, including a missing `#`, a short value, and a
    /// value with non-ASCII bytes in it.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#')?.as_bytes();
        if digits.len() != 6 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let nibble = |byte: u8| (byte as char).to_digit(16).expect("checked hex") as u8;
        Some(Self::new(
            nibble(digits[0]) << 4 | nibble(digits[1]),
            nibble(digits[2]) << 4 | nibble(digits[3]),
            nibble(digits[4]) << 4 | nibble(digits[5]),
        ))
    }

    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text)
            .ok_or_else(|| D::Error::custom(format!("expected \"#rrggbb\", got {text:?}")))
    }
}

/// The theme's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
    /// A mode this build does not know; `mode` is a string so Omarchy can add one.
    #[default]
    #[serde(other)]
    Unknown,
}

/// The Omarchy palette, parsed.
///
/// Missing keys fall back to [`Palette::default`], so a partial or unrecognised
/// `colors.toml` still yields something a client can draw with. The `bright_*` entries are
/// optional because not every Omarchy theme writes them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub mode: Mode,
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
    pub red: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
    pub blue: Rgb,
    pub magenta: Rgb,
    pub cyan: Rgb,
    pub bright_foreground: Option<Rgb>,
    pub bright_red: Option<Rgb>,
    pub bright_green: Option<Rgb>,
    pub bright_yellow: Option<Rgb>,
    pub bright_blue: Option<Rgb>,
    pub bright_magenta: Option<Rgb>,
    pub bright_cyan: Option<Rgb>,
}

impl Palette {
    /// The palette file, relative to `$XDG_STATE_HOME` / `~/.local/state`.
    pub const FILE: &'static str = "omarchy/current/theme/colors.toml";

    pub fn from_toml_str(text: &str) -> Result<Self, ThemeError> {
        Ok(toml::from_str(text)?)
    }

    pub fn load(path: &Path) -> Result<Self, ThemeError> {
        let text = std::fs::read_to_string(path).map_err(|source| ThemeError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml_str(&text)
    }

    /// The palette a client should actually draw with: the Omarchy one when it can be
    /// read, otherwise the built-in default. A missing or broken palette must never stop a
    /// client from starting.
    pub fn load_or_default(path: Option<&Path>) -> Self {
        path.and_then(|path| Self::load(path).ok())
            .unwrap_or_default()
    }

    /// `~/.local/state/omarchy/current/theme/colors.toml`, honouring `XDG_STATE_HOME`.
    pub fn default_path() -> Option<PathBuf> {
        default_path_from(
            env::var_os("XDG_STATE_HOME").as_deref(),
            env::var_os("HOME").as_deref(),
        )
    }
}

impl Default for Palette {
    /// A neutral dark palette, used when Omarchy has not written one.
    fn default() -> Self {
        Self {
            mode: Mode::Dark,
            background: Rgb::new(0x1d, 0x20, 0x21),
            foreground: Rgb::new(0xeb, 0xdb, 0xb2),
            accent: Rgb::new(0x26, 0x8b, 0xd3),
            red: Rgb::new(0xcc, 0x24, 0x1d),
            green: Rgb::new(0x98, 0x97, 0x1a),
            yellow: Rgb::new(0xd7, 0x99, 0x21),
            blue: Rgb::new(0x45, 0x85, 0x88),
            magenta: Rgb::new(0xb1, 0x62, 0x86),
            cyan: Rgb::new(0x68, 0x9d, 0x6a),
            bright_foreground: None,
            bright_red: None,
            bright_green: None,
            bright_yellow: None,
            bright_blue: None,
            bright_magenta: None,
            bright_cyan: None,
        }
    }
}

/// Why a palette could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid Omarchy palette: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot watch {path}: {source}")]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
    #[error("{0} has no parent directory to watch for a theme switch")]
    NoWatchRoot(PathBuf),
}

/// The palette path for a given `$XDG_STATE_HOME` and `$HOME`, split out so it is testable
/// without touching the process environment.
///
/// An empty `$XDG_STATE_HOME` counts as unset, as the XDG base directory spec says.
pub fn default_path_from(state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = match state_home.filter(|dir| !dir.is_empty()) {
        Some(state_home) => PathBuf::from(state_home),
        None => {
            let home = home.filter(|dir| !dir.is_empty())?;
            Path::new(home).join(".local/state")
        }
    };
    Some(base.join(Palette::FILE))
}

#[cfg(test)]
mod tests {
    use super::{Palette, Rgb, default_path_from};
    use std::ffi::OsStr;
    use std::path::Path;

    /// The real palette this machine's Omarchy theme writes, verbatim.
    const OMARCHY_COLORS: &str = r##"
# Solarized Osaka (dark) for Omarchy 4.
# Palette from craftzdog/solarized-osaka.nvim @ a21176e (Apache-2.0):
#   extras/lua/solarized_osaka_dark.lua (hex values and semantic names),
#   lua/solarized-osaka/groups/editor.lua + syntax.lua (UI roles),
#   extras/ghostty/solarized_osaka_dark (bright colours).
# Not set on purpose (Omarchy derives them): dark_background, darker_background, brown.
mode = "dark"

accent = "#268bd3"
selection = "#063540"
muted = "#576d74"

background = "#001419"
lighter_background = "#002c38"

foreground = "#839395"
dark_foreground = "#576d74"
light_foreground = "#9eabac"
bright_foreground = "#adb7b7"

red = "#db302d"
yellow = "#b28500"
orange = "#c94c16"
green = "#849900"
cyan = "#29a298"
blue = "#268bd3"
magenta = "#d23681"

bright_red = "#f55350"
bright_yellow = "#ffbf00"
bright_green = "#b7f900"
bright_cyan = "#2aeddd"
bright_blue = "#46acf5"
bright_magenta = "#f254a0"
"##;

    #[test]
    fn parses_the_real_omarchy_palette() {
        let palette = Palette::from_toml_str(OMARCHY_COLORS).expect("the real palette parses");

        assert_eq!(palette.mode, super::Mode::Dark);
        assert_eq!(palette.background.to_hex(), "#001419");
        assert_eq!(palette.foreground.to_hex(), "#839395");
        assert_eq!(palette.accent.to_hex(), "#268bd3");
        assert_eq!(palette.red.to_hex(), "#db302d");
        assert_eq!(palette.yellow.to_hex(), "#b28500");
        assert_eq!(palette.green.to_hex(), "#849900");
        assert_eq!(palette.blue.to_hex(), "#268bd3");
        assert_eq!(palette.magenta.to_hex(), "#d23681");
        assert_eq!(palette.cyan.to_hex(), "#29a298");
        assert_eq!(
            palette.bright_foreground.map(|c| c.to_hex()),
            Some("#adb7b7".to_owned())
        );
        assert_eq!(
            palette.bright_cyan.map(|c| c.to_hex()),
            Some("#2aeddd".to_owned())
        );
        // `selection`, `muted`, `orange`, `lighter_background` and the `*_foreground`
        // variants are in the file but are not part of the palette this client keeps.
    }

    #[test]
    fn missing_keys_fall_back_to_the_defaults() {
        let defaults = Palette::default();
        let palette =
            Palette::from_toml_str("mode = \"light\"\n").expect("a partial palette parses");

        assert_eq!(palette.mode, super::Mode::Light);
        assert_eq!(palette.background, defaults.background);
        assert_eq!(palette.accent, defaults.accent);
    }

    #[test]
    fn bright_colours_are_optional() {
        let palette = Palette::from_toml_str("background = \"#001419\"\n").expect("parses");
        assert_eq!(palette.bright_blue, None);
        assert_eq!(palette.bright_magenta, None);
    }

    #[test]
    fn an_unknown_mode_does_not_break_the_parse() {
        let palette = Palette::from_toml_str("mode = \"auto\"\n").expect("parses");
        assert_eq!(palette.mode, super::Mode::Unknown);
    }

    #[test]
    fn a_bad_colour_is_a_parse_error() {
        let err = Palette::from_toml_str("accent = \"purple\"\n").unwrap_err();
        assert!(
            matches!(err, super::ThemeError::Parse(_)),
            "expected a parse error, got {err:?}"
        );
    }

    #[test]
    fn a_missing_file_is_an_error_but_leaves_a_usable_palette() {
        let dir = std::env::temp_dir().join(format!("nuncio-theme-{}", std::process::id()));
        let missing = dir.join("colors.toml");

        assert!(matches!(
            Palette::load(&missing),
            Err(super::ThemeError::Io { .. })
        ));
        assert_eq!(Palette::load_or_default(Some(&missing)), Palette::default());
        assert_eq!(Palette::load_or_default(None), Palette::default());

        let written = dir.join("real.toml");
        std::fs::create_dir_all(&dir).expect("temp dir is creatable");
        std::fs::write(&written, OMARCHY_COLORS).expect("temp palette is writable");
        assert_eq!(
            Palette::load_or_default(Some(&written)).background,
            Rgb::new(0x00, 0x14, 0x19)
        );
        std::fs::remove_dir_all(&dir).expect("temp dir is removable");
    }

    #[test]
    fn hex_colours_round_trip_and_reject_nonsense() {
        assert_eq!(Rgb::from_hex("#268bd3"), Some(Rgb::new(0x26, 0x8b, 0xd3)));
        assert_eq!(Rgb::from_hex("#268BD3"), Some(Rgb::new(0x26, 0x8b, 0xd3)));
        assert_eq!(Rgb::from_hex("268bd3"), None);
        assert_eq!(Rgb::from_hex("#268bd"), None);
        assert_eq!(Rgb::from_hex("#268bd3ff"), None);
        assert_eq!(Rgb::from_hex("#zzzzzz"), None);
        // Multi-byte input must not panic on a byte-index slice.
        assert_eq!(Rgb::from_hex("#ééé"), None);
    }

    #[test]
    fn the_palette_path_follows_the_xdg_base_directory_spec() {
        let state = OsStr::new("/state");
        let home = OsStr::new("/home/owner");
        assert_eq!(
            default_path_from(Some(state), Some(home)).as_deref(),
            Some(Path::new("/state/omarchy/current/theme/colors.toml"))
        );
        assert_eq!(
            default_path_from(None, Some(home)).as_deref(),
            Some(Path::new(
                "/home/owner/.local/state/omarchy/current/theme/colors.toml"
            ))
        );
        assert_eq!(
            default_path_from(Some(OsStr::new("")), Some(home)).as_deref(),
            Some(Path::new(
                "/home/owner/.local/state/omarchy/current/theme/colors.toml"
            ))
        );
        assert_eq!(default_path_from(None, None), None);
        assert_eq!(default_path_from(Some(OsStr::new("")), None), None);
    }
}

/// How long to wait after a filesystem event before re-reading the palette.
///
/// `omarchy-theme-set` replaces the theme directory in several steps, so one switch produces a
/// burst of events; the debounce collapses the burst into a single read.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Follows a live Omarchy theme switch.
///
/// The watcher deliberately does **not** watch `colors.toml` itself. `omarchy-theme-set` stages
/// `current/next-theme/`, then runs `rm -rf current/theme; mv current/next-theme current/theme`,
/// and rewrites `theme.name` — an inotify watch on a file inside `theme/` is dropped by the first
/// switch. So the watch is on the palette file's **parent directory** (`current/`), and every
/// event there causes `colors.toml` to be re-read.
pub struct ThemeWatcher {
    _watcher: notify::RecommendedWatcher,
    rx: std::sync::mpsc::Receiver<Palette>,
    last: std::sync::Arc<std::sync::Mutex<Option<Palette>>>,
}

impl ThemeWatcher {
    /// Start watching the directory that holds `colors_toml` (usually [`Palette::default_path`]).
    ///
    /// The watch root is that file's parent's parent — `~/.local/state/omarchy/current`.
    pub fn start(colors_toml: &Path) -> Result<Self, ThemeError> {
        Self::start_with(colors_toml, Palette::load)
    }

    /// [`ThemeWatcher::start`] with the palette reader passed in (tests make it fail).
    fn start_with<L>(colors_toml: &Path, load: L) -> Result<Self, ThemeError>
    where
        L: Fn(&Path) -> Result<Palette, ThemeError> + Send + 'static,
    {
        let theme_dir = colors_toml
            .parent()
            .ok_or_else(|| ThemeError::NoWatchRoot(colors_toml.to_owned()))?;
        let watch_root = theme_dir
            .parent()
            .ok_or_else(|| ThemeError::NoWatchRoot(colors_toml.to_owned()))?
            .to_owned();

        let (tx, rx) = std::sync::mpsc::channel();
        let last = std::sync::Arc::new(std::sync::Mutex::new(None::<Palette>));

        // The notify callback only signals; reading the palette happens on this crate's own
        // thread, so a reader that panics ends that thread (and `recv` sees the channel close)
        // instead of aborting the process inside notify's callback.
        let (events_tx, events_rx) = std::sync::mpsc::channel::<()>();
        let mut watcher =
            notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                if event.is_ok() {
                    let _ = events_tx.send(());
                }
            })
            .map_err(|source| ThemeError::Watch {
                path: watch_root.clone(),
                source,
            })?;

        let palette_path = colors_toml.to_owned();
        let seen = std::sync::Arc::clone(&last);
        // Ends when this `ThemeWatcher` (and with it the watcher's event sender) is dropped.
        std::thread::spawn(move || {
            while events_rx.recv().is_ok() {
                std::thread::sleep(DEBOUNCE);
                while events_rx.try_recv().is_ok() {}
                // Mid-switch the directory is briefly gone; the next event picks it up.
                let Ok(palette) = load(&palette_path) else {
                    continue;
                };
                let mut seen = seen.lock().expect("theme watcher mutex");
                if seen.as_ref() == Some(&palette) {
                    continue;
                }
                *seen = Some(palette.clone());
                drop(seen);
                if tx.send(palette).is_err() {
                    return;
                }
            }
        });

        watcher
            .watch(&watch_root, notify::RecursiveMode::NonRecursive)
            .map_err(|source| ThemeError::Watch {
                path: watch_root,
                source,
            })?;

        Ok(Self {
            _watcher: watcher,
            rx,
            last,
        })
    }

    /// Wait for the next palette. `None` means the watcher's thread has ended (nothing more will
    /// ever arrive), so a forwarding loop stops instead of spinning.
    pub fn recv(&self) -> Option<Palette> {
        self.rx.recv().ok()
    }

    /// Wait up to `timeout` for the next palette. `None` means nothing changed in time.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Palette> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// The most recent palette this watcher has emitted, if any.
    pub fn last(&self) -> Option<Palette> {
        self.last.lock().expect("theme watcher mutex").clone()
    }
}

#[cfg(test)]
mod watch_tests {
    use super::{Mode, Palette, Rgb, ThemeError, ThemeWatcher};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    fn unique_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nuncio-watch-{label}-{}-{stamp}",
            std::process::id()
        ))
    }

    fn write_palette(theme_dir: &Path, mode: &str, background: &str) {
        fs::create_dir_all(theme_dir).expect("create theme dir");
        fs::write(
            theme_dir.join("colors.toml"),
            format!(
                "mode = \"{mode}\"\nbackground = \"{background}\"\nforeground = \"#839395\"\n\
                 accent = \"#268bd3\"\n"
            ),
        )
        .expect("write palette");
    }

    /// The S5 acceptance test: a switch completes well inside 500 ms.
    #[test]
    fn a_theme_switch_is_noticed_within_500ms() {
        let root = unique_root("switch");
        let current = root.join("omarchy/current");
        let theme = current.join("theme");
        let next = current.join("next-theme");
        write_palette(&theme, "dark", "#001419");
        fs::write(current.join("theme.name"), "solarized-osaka\n").expect("write theme.name");

        let watcher = ThemeWatcher::start(&theme.join("colors.toml")).expect("watcher starts");
        // Let inotify settle first, so the measurement is detection and not setup.
        std::thread::sleep(Duration::from_millis(250));

        // Exactly what `omarchy-theme-set` does: stage, swap the whole directory, rewrite the name.
        write_palette(&next, "light", "#fdf6e3");
        let started = Instant::now();
        fs::remove_dir_all(&theme).expect("remove old theme");
        fs::rename(&next, &theme).expect("swap theme in");
        fs::write(current.join("theme.name"), "flexoki-light\n").expect("rewrite theme.name");

        let palette = watcher
            .recv_timeout(Duration::from_millis(500))
            .expect("a theme switch must be noticed within 500 ms");
        let elapsed = started.elapsed();

        assert_eq!(palette.mode, Mode::Light);
        assert_eq!(palette.background, Rgb::new(0xfd, 0xf6, 0xe3));
        assert!(
            elapsed < Duration::from_millis(500),
            "detected after {elapsed:?}"
        );
        println!("theme switch detected in {elapsed:?}");
        println!(
            "  mode={:?} background={}",
            palette.mode,
            palette.background.to_hex()
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recv_returns_when_the_watcher_thread_ends() {
        let root = unique_root("ended");
        let current = root.join("omarchy/current");
        let theme = current.join("theme");
        write_palette(&theme, "dark", "#001419");
        let watcher = ThemeWatcher::start_with(&theme.join("colors.toml"), |_| {
            panic!("palette reader crashed")
        })
        .expect("watcher starts");
        std::thread::sleep(Duration::from_millis(250));
        // The first event ends the watcher's thread.
        fs::write(current.join("theme.name"), "flexoki-light\n").expect("write theme.name");

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(watcher.recv());
        });
        let got = done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("recv must return once the watcher's thread has ended, not wait forever");
        assert_eq!(got, None);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_palette_path_without_a_parent_is_rejected() {
        let err = match ThemeWatcher::start(Path::new("colors.toml")) {
            Err(err) => err,
            Ok(_) => panic!("a path with no watch root must be rejected"),
        };
        assert!(matches!(err, ThemeError::NoWatchRoot(_)), "{err}");
    }

    #[test]
    fn the_same_palette_is_not_emitted_twice() {
        let root = unique_root("dedupe");
        let current = root.join("omarchy/current");
        let theme = current.join("theme");
        write_palette(&theme, "dark", "#001419");

        let watcher = ThemeWatcher::start(&theme.join("colors.toml")).expect("watcher starts");
        std::thread::sleep(Duration::from_millis(250));

        // Touch the palette with byte-identical content twice.
        let path = theme.join("colors.toml");
        for _ in 0..2 {
            let text = fs::read_to_string(&path).expect("read palette");
            fs::write(&path, text).expect("rewrite palette");
            std::thread::sleep(Duration::from_millis(300));
        }

        assert!(
            watcher.recv_timeout(Duration::from_millis(400)).is_none(),
            "an unchanged palette must not be re-emitted"
        );
        assert!(watcher.last().is_none(), "nothing emitted yet");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_default_palette_path_is_under_the_expected_tree() {
        let path = Palette::default_path().expect("a HOME is set in tests");
        assert!(
            path.ends_with("omarchy/current/theme/colors.toml"),
            "{path:?}"
        );
    }
}
