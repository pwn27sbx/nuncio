//! The settings window's rules, without GTK: it edits the plain configuration file
//! (`docs/config.md`) and writes it back with [`Config::save`] after each change.
//!
//! - The file is read when the window opens and again before each change, which changes only its
//!   own key there (a hand edit to another key meanwhile is kept). A file that does not load (a key this build does not
//!   know, a wrong type, bad TOML) is shown as the problem and never overwritten: every change is
//!   refused until the user fixes or removes it.
//! - A change that cannot be written (a read-only directory, a full disk) keeps the old file and
//!   the old value, and says why.
//! - Notifications and the title bar change at once ([`Controller::reconfigure`]); the background
//!   and start-hidden choices take effect at the next start; the picture protocol is nuncio-tui's.
//! - Writing keeps only the known keys, so the file's comments are not kept (the written file says
//!   so in its first lines).
//!
//! [`Controller::reconfigure`]: crate::controller::Controller::reconfigure

use std::path::{Path, PathBuf};

use nuncio_client::{Config, ImageProtocol, SaveError, TitleBar};

/// One setting changed in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// `[notifications] enabled`
    Notifications(bool),
    /// `[notifications] show_preview`
    ShowPreview(bool),
    /// `[app] background`
    Background(bool),
    /// `[app] start_hidden`
    StartHidden(bool),
    /// `[app] title_bar`
    TitleBar(TitleBar),
    /// `[ui] image_protocol` (`None`: ask the terminal)
    ImageProtocol(Option<ImageProtocol>),
}

/// When a change is followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Takes {
    /// At once, by this app.
    Now,
    /// At this app's next start.
    NextStart,
    /// Only by nuncio-tui, at its next start.
    Tui,
}

impl Change {
    /// When this change is followed.
    pub fn takes(self) -> Takes {
        match self {
            Self::Notifications(_) | Self::ShowPreview(_) | Self::TitleBar(_) => Takes::Now,
            Self::Background(_) | Self::StartHidden(_) => Takes::NextStart,
            Self::ImageProtocol(_) => Takes::Tui,
        }
    }

    /// `config` with this change made.
    pub fn applied_to(self, config: &Config) -> Config {
        let mut config = config.clone();
        match self {
            Self::Notifications(on) => config.notifications.enabled = on,
            Self::ShowPreview(on) => config.notifications.show_preview = on,
            Self::Background(on) => config.app.background = on,
            Self::StartHidden(on) => config.app.start_hidden = on,
            Self::TitleBar(choice) => config.app.title_bar = choice,
            Self::ImageProtocol(protocol) => config.ui.image_protocol = protocol,
        }
        config
    }
}

/// The title bar choices, in the order the window lists them.
pub const TITLE_BARS: [(TitleBar, &str); 3] = [
    (TitleBar::Auto, "Automatic (hidden on Hyprland)"),
    (TitleBar::Shown, "Shown"),
    (TitleBar::Hidden, "Hidden"),
];

/// The picture protocols, in the order the window lists them.
pub const IMAGE_PROTOCOLS: [(Option<ImageProtocol>, &str); 6] = [
    (None, "Ask the terminal"),
    (Some(ImageProtocol::Kitty), "kitty"),
    (Some(ImageProtocol::Sixel), "sixel"),
    (Some(ImageProtocol::Iterm2), "iterm2"),
    (Some(ImageProtocol::Halfblocks), "halfblocks"),
    (Some(ImageProtocol::Off), "off"),
];

/// The settings being edited: the file's path and what it holds.
#[derive(Debug, Clone)]
pub struct Preferences {
    path: Option<PathBuf>,
    config: Config,
    /// Why the file cannot be edited (it does not load, or there is no path).
    problem: Option<String>,
}

impl Preferences {
    /// Read the file at `path` (`None`: neither `XDG_CONFIG_HOME` nor `HOME` is set). A missing
    /// file is the defaults, written on the first change.
    pub fn load(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self {
                path: None,
                config: Config::default(),
                problem: Some(
                    "there is no configuration file to edit: set HOME or XDG_CONFIG_HOME"
                        .to_owned(),
                ),
            };
        };
        match Config::load(&path) {
            Ok(config) => Self {
                path: Some(path),
                config,
                problem: None,
            },
            Err(error) => Self {
                path: Some(path),
                config: Config::default(),
                problem: Some(format!(
                    "{error}. Fix or remove the file: the settings will not overwrite it."
                )),
            },
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Why nothing can be changed, when nothing can.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Make `change` and write the file: the file is read again first and only this one key is
    /// changed in it, so a valid hand edit to another key made while the window was open is
    /// kept (and shown from now on). `Ok(true)` when the configuration changed (and is saved),
    /// `Ok(false)` when it already was so (nothing written). `Err` says why nothing
    /// changed: the file cannot be edited, or writing failed (the old file and the old value
    /// stay).
    pub fn change(&mut self, change: Change) -> Result<bool, String> {
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        let Some(path) = &self.path else {
            return Err("there is no configuration file to edit".to_owned());
        };
        // The file as it is now, so a hand edit to another key since the window opened is kept:
        // only this change is made to it.
        let fresh = Config::load(path).map_err(|error| SaveError::Invalid(error).to_string())?;
        let changed = change.applied_to(&fresh);
        if changed == fresh {
            let differs = fresh != self.config;
            self.config = fresh;
            return Ok(differs);
        }
        changed
            .save(path)
            .map_err(|error| format!("settings not saved: {error}"))?;
        self.config = changed;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
