//! The plain-file configuration shared by nuncio-tui and nuncio-desktop (`docs/config.md`).
//!
//! The file is TOML at `$XDG_CONFIG_HOME/nuncio/config.toml` (else
//! `~/.config/nuncio/config.toml`). Every key is optional and has a default; a missing file is
//! all the defaults. A key this build does not know is an error naming it, so a typo is visible
//! instead of silently ignored.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: Ui,
    pub notifications: Notifications,
    pub app: App,
}

/// `[ui]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// How nuncio-tui draws pictures, as `NUNCIO_IMAGE_PROTOCOL` names it; that variable wins
    /// over the file. `None` (the default) asks the terminal.
    pub image_protocol: Option<ImageProtocol>,
}

/// A value of `[ui] image_protocol`: the same five `NUNCIO_IMAGE_PROTOCOL` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageProtocol {
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
    Off,
}

impl ImageProtocol {
    /// The spelling in the file and in `NUNCIO_IMAGE_PROTOCOL`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
            Self::Halfblocks => "halfblocks",
            Self::Off => "off",
        }
    }
}

/// `[notifications]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    /// Desktop notifications for new messages (default `true`).
    pub enabled: bool,
    /// Put the message text in the notification (default `false`: a notification says only
    /// `New message`, `Photo`, … so nothing private shows on a locked or shared screen).
    pub show_preview: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            show_preview: false,
        }
    }
}

/// `[app]` (nuncio-desktop).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct App {
    /// Closing the window keeps the app running in the background (default `true`).
    pub background: bool,
    /// Start with no window, in the background (default `false`).
    pub start_hidden: bool,
    /// Whether the window shows its title bar controls (default `auto`).
    pub title_bar: TitleBar,
}

impl Default for App {
    fn default() -> Self {
        Self {
            background: true,
            start_hidden: false,
            title_bar: TitleBar::Auto,
        }
    }
}

/// A value of `[app] title_bar`: `auto` shows no title bar (no window controls, no window title)
/// on a tiling desktop (Hyprland) and shows it elsewhere; `shown` and `hidden` force it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleBar {
    #[default]
    Auto,
    Shown,
    Hidden,
}

/// Why the configuration file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Bad TOML, a wrong type, or a key this build does not know (the message names it).
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl Config {
    /// The path below `$XDG_CONFIG_HOME` (or `~/.config`).
    pub const FILE: &'static str = "nuncio/config.toml";

    /// Parse the file's text.
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Read the file at `path`. A missing file is all the defaults, not an error.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::from_toml_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// `$XDG_CONFIG_HOME/nuncio/config.toml`, else `~/.config/nuncio/config.toml`; `None` with
    /// neither variable set.
    pub fn default_path() -> Option<PathBuf> {
        config_path_from(
            env::var_os("XDG_CONFIG_HOME").as_deref(),
            env::var_os("HOME").as_deref(),
        )
    }
}

/// The configuration path for a given `$XDG_CONFIG_HOME` and `$HOME`, split out so it is testable
/// without touching the process environment. An empty variable counts as unset, as the XDG base
/// directory spec says.
pub fn config_path_from(config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = match config_home.filter(|dir| !dir.is_empty()) {
        Some(config_home) => PathBuf::from(config_home),
        None => Path::new(home.filter(|dir| !dir.is_empty())?).join(".config"),
    };
    Some(base.join(Config::FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nuncio-config-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let config = Config::default();
        assert_eq!(config.ui.image_protocol, None);
        assert!(config.notifications.enabled);
        assert!(
            !config.notifications.show_preview,
            "no message text by default"
        );
        assert!(config.app.background);
        assert!(!config.app.start_hidden);
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(
            Config::from_toml_str("").expect("parses"),
            Config::default()
        );
    }

    #[test]
    fn a_missing_file_is_the_defaults_not_an_error() {
        let dir = temp_dir("missing");
        let config = Config::load(&dir.join("config.toml")).expect("missing is fine");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn every_key_is_read() {
        let config = Config::from_toml_str(
            "[ui]\nimage_protocol = \"sixel\"\n\
             [notifications]\nenabled = false\nshow_preview = true\n\
             [app]\nbackground = false\nstart_hidden = true\n",
        )
        .expect("parses");
        assert_eq!(config.ui.image_protocol, Some(ImageProtocol::Sixel));
        assert!(!config.notifications.enabled);
        assert!(config.notifications.show_preview);
        assert!(!config.app.background);
        assert!(config.app.start_hidden);
    }

    #[test]
    fn a_partial_section_keeps_the_other_defaults() {
        let config =
            Config::from_toml_str("[notifications]\nshow_preview = true\n").expect("parses");
        assert!(config.notifications.enabled, "enabled keeps its default");
        assert!(config.notifications.show_preview);
        assert!(config.app.background);
    }

    #[test]
    fn every_image_protocol_value_is_accepted_and_others_are_not() {
        for (text, want) in [
            ("kitty", ImageProtocol::Kitty),
            ("sixel", ImageProtocol::Sixel),
            ("iterm2", ImageProtocol::Iterm2),
            ("halfblocks", ImageProtocol::Halfblocks),
            ("off", ImageProtocol::Off),
        ] {
            let config = Config::from_toml_str(&format!("[ui]\nimage_protocol = \"{text}\"\n"))
                .expect("parses");
            assert_eq!(config.ui.image_protocol, Some(want));
            assert_eq!(want.as_str(), text);
        }
        assert!(Config::from_toml_str("[ui]\nimage_protocol = \"png\"\n").is_err());
    }

    #[test]
    fn an_unknown_key_is_an_error_naming_it() {
        for (text, key) in [
            ("[notifications]\nshow_previews = true\n", "show_previews"),
            ("[app]\nbackgroud = false\n", "backgroud"),
            ("[ui]\nimage = \"kitty\"\n", "image"),
            ("[window]\nwidth = 3\n", "window"),
            ("theme = \"dark\"\n", "theme"),
        ] {
            let err = Config::from_toml_str(text).expect_err(text);
            assert!(err.to_string().contains(key), "{err} names {key}");
        }
    }

    #[test]
    fn load_reports_the_path_and_the_bad_key() {
        let dir = temp_dir("bad");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[app]\nstart_hiden = true\n").expect("write");
        let err = Config::load(&path).expect_err("unknown key");
        let text = err.to_string();
        assert!(text.contains("start_hiden"), "{text}");
        assert!(text.contains(&path.display().to_string()), "{text}");
    }

    #[test]
    fn load_reads_a_real_file() {
        let dir = temp_dir("good");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[app]\nstart_hidden = true\n").expect("write");
        assert!(Config::load(&path).expect("loads").app.start_hidden);
    }

    #[test]
    fn the_title_bar_is_auto_unless_shown_or_hidden_is_chosen() {
        assert_eq!(Config::default().app.title_bar, TitleBar::Auto);
        for (text, expected) in [
            ("auto", TitleBar::Auto),
            ("shown", TitleBar::Shown),
            ("hidden", TitleBar::Hidden),
        ] {
            let config = Config::from_toml_str(&format!("[app]\ntitle_bar = \"{text}\"\n"))
                .expect("a valid value");
            assert_eq!(config.app.title_bar, expected);
        }
        assert!(Config::from_toml_str("[app]\ntitle_bar = \"none\"\n").is_err());
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        assert!(Config::from_toml_str("[notifications]\nenabled = \"yes\"\n").is_err());
    }

    #[test]
    fn the_path_follows_the_xdg_base_directory_spec() {
        assert_eq!(
            config_path_from(Some(OsStr::new("/cfg")), Some(OsStr::new("/home/owner"))),
            Some(PathBuf::from("/cfg/nuncio/config.toml"))
        );
        assert_eq!(
            config_path_from(Some(OsStr::new("")), Some(OsStr::new("/home/owner"))),
            Some(PathBuf::from("/home/owner/.config/nuncio/config.toml"))
        );
        assert_eq!(
            config_path_from(None, Some(OsStr::new("/home/owner"))),
            Some(PathBuf::from("/home/owner/.config/nuncio/config.toml"))
        );
        assert_eq!(config_path_from(None, None), None);
    }
}
