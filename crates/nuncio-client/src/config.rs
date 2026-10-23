//! The plain-file configuration shared by nuncio-tui and nuncio-desktop (`docs/config.md`).
//!
//! The file is TOML at `$XDG_CONFIG_HOME/nuncio/config.toml` (else
//! `~/.config/nuncio/config.toml`). Every key is optional and has a default; a missing file is
//! all the defaults. A key this build does not know is an error naming it, so a typo is visible
//! instead of silently ignored.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: Ui,
    pub notifications: Notifications,
    pub app: App,
}

/// `[ui]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// How nuncio-tui draws pictures, as `NUNCIO_IMAGE_PROTOCOL` names it; that variable wins
    /// over the file. `None` (the default) asks the terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_protocol: Option<ImageProtocol>,
}

/// A value of `[ui] image_protocol`: the same five `NUNCIO_IMAGE_PROTOCOL` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleBar {
    #[default]
    Auto,
    Shown,
    Hidden,
}

/// Why the configuration file was not written ([`Config::save`]). The file on disk is unchanged.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// The file there now cannot be read back as a configuration (a key this build does not know,
    /// a wrong type, bad TOML, or it cannot be read): writing would destroy what the user wrote.
    #[error("not overwriting it: {0}")]
    Invalid(ConfigError),
    /// The configuration could not be turned into TOML: nothing was written.
    #[error("cannot serialise the configuration: {0}")]
    Serialize(toml::ser::Error),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// The first lines of a file nuncio writes.
pub const WRITTEN_HEADER: &str = "# nuncio configuration (see docs/config.md).\n\
     # Written by nuncio-desktop's settings: comments and blank lines in this file are not\n\
     # preserved when it writes it again.\n";

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

    /// The file's text: [`WRITTEN_HEADER`] and every known key (`image_protocol` only when one is
    /// chosen). [`Config::from_toml_str`] reads it back as the same configuration.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        let body = toml::to_string(self)?;
        Ok(format!("{WRITTEN_HEADER}\n{body}"))
    }

    /// Write the configuration to `path`, atomically: a temporary file in the same directory
    /// (mode 0600) renamed over it, so a failure leaves the old file whole. A missing directory is
    /// created with mode 0700. A file there that does not load ([`Config::load`] fails: an unknown
    /// key, a wrong type, bad TOML) is never overwritten. A symlink is followed: its target is
    /// rewritten and the link stays. After the rename the directory holding the file (the
    /// link's target's, for a symlink) is flushed too, best effort: a directory that cannot be
    /// synced does not fail a save that has already happened.
    pub fn save(&self, path: &Path) -> Result<(), SaveError> {
        Self::write_text(path, self.to_toml_string(), &mut sync_dir)
    }

    /// [`Config::save`] of an already serialised configuration: a serialisation error is
    /// returned before anything on disk is touched. `sync_dir` flushes the directory that
    /// received the rename ([`sync_dir`] in a real save; a test counts the calls).
    fn write_text(
        path: &Path,
        text: Result<String, toml::ser::Error>,
        sync_dir: &mut dyn FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<(), SaveError> {
        Self::load(path).map_err(SaveError::Invalid)?;
        let text = text.map_err(SaveError::Serialize)?;
        let write_error = |path: &Path| {
            let path = path.to_owned();
            move |source| SaveError::Write { path, source }
        };
        let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let target = if is_link {
            std::fs::canonicalize(path).map_err(write_error(path))?
        } else {
            path.to_owned()
        };
        let dir = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
            _ => PathBuf::from("."),
        };
        if !dir.exists() {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(write_error(&dir))?;
        }
        let name = target
            .file_name()
            .map_or_else(|| "config.toml".into(), OsStr::to_string_lossy);
        let (temp, file) = create_temp(&dir, &name).map_err(write_error(&dir))?;
        // From here the temporary file is this save's own: only it is removed on a failure.
        let written = fill(file, text.as_bytes()).and_then(|()| std::fs::rename(&temp, &target));
        if let Err(source) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(SaveError::Write {
                path: target,
                source,
            });
        }
        // The rename is done and the file is whole; flushing the directory only makes the rename
        // itself durable across a crash. Best effort: a filesystem or platform that cannot open or
        // fsync a directory leaves a save that already succeeded, so its error is not returned.
        let _ = sync_dir(&dir);
        Ok(())
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

/// The next temporary file number of this process.
static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The name of temporary file number `count` beside the configuration file `name`.
fn temp_name(name: &str, count: u64) -> String {
    format!(".{name}.tmp-{}-{count}", std::process::id())
}

/// Create a new temporary file (mode 0600) beside the configuration, named after it, this
/// process and a counter. A name that exists already (another save's, or one left by an earlier
/// process with the same id) is never opened or removed: the next name is tried.
fn create_temp(dir: &Path, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::Ordering;
    let mut last = None;
    for _ in 0..100 {
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(temp_name(name, count));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temporary file name")))
}

/// Write `bytes` into the new temporary `file` with mode exactly 0600, flushed to the disk.
fn fill(mut file: std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    // The umask can only take bits away; set it exactly.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flush `dir`'s entries to the disk, so a rename into it survives a crash.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
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

    /// A directory this test created under the system temp dir, removed (with what the test put
    /// in it) when the guard is dropped.
    struct TempDir(PathBuf);

    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for TempDir {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            // A test may have left it read-only.
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(label: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "nuncio-config-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir)
    }

    #[test]
    fn the_temp_dir_helper_leaves_nothing_behind() {
        let path;
        {
            let dir = temp_dir("guard");
            path = dir.to_path_buf();
            std::fs::write(dir.join("config.toml"), "").expect("write");
            std::fs::create_dir_all(dir.join("nuncio")).expect("subdir");
        }
        assert!(!path.exists(), "{} is left behind", path.display());
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
    fn every_kind_of_config() -> Vec<Config> {
        let mut changed = Config::default();
        changed.ui.image_protocol = Some(ImageProtocol::Halfblocks);
        changed.notifications.enabled = false;
        changed.notifications.show_preview = true;
        changed.app.background = false;
        changed.app.start_hidden = true;
        changed.app.title_bar = TitleBar::Hidden;
        let mut shown = Config::default();
        shown.app.title_bar = TitleBar::Shown;
        shown.ui.image_protocol = Some(ImageProtocol::Off);
        vec![Config::default(), changed, shown]
    }

    #[test]
    fn a_saved_config_loads_back_the_same() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("config.toml");
        for config in every_kind_of_config() {
            config.save(&path).expect("saves");
            assert_eq!(Config::load(&path).expect("loads"), config);
            assert_eq!(
                Config::from_toml_str(&config.to_toml_string().expect("serialises"))
                    .expect("parses"),
                config
            );
        }
    }

    #[test]
    fn the_written_text_says_comments_are_not_kept_and_has_only_known_keys() {
        let mut config = Config::default();
        config.ui.image_protocol = Some(ImageProtocol::Kitty);
        let text = config.to_toml_string().expect("serialises");
        assert!(text.starts_with(WRITTEN_HEADER), "{text}");
        assert!(text.contains("comments"), "{text}");
        for key in [
            "[ui]",
            "image_protocol = \"kitty\"",
            "[notifications]",
            "enabled = true",
            "show_preview = false",
            "[app]",
            "background = true",
            "start_hidden = false",
            "title_bar = \"auto\"",
        ] {
            assert!(text.contains(key), "{key} missing from:\n{text}");
        }
        assert!(
            !Config::default()
                .to_toml_string()
                .expect("serialises")
                .contains("image_protocol"),
            "an unset protocol is left out (it means: ask the terminal)"
        );
    }

    #[test]
    fn saving_creates_the_directory_0700_and_the_file_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("modes");
        let path = dir.join("nuncio").join("config.toml");
        Config::default().save(&path).expect("saves");
        let mode = |p: &Path| std::fs::metadata(p).expect("exists").permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir.join("nuncio")), 0o700);
        // Rewriting an existing 0644 file leaves it 0600 too.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        Config::default().save(&path).expect("saves again");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn a_file_that_does_not_load_is_never_overwritten() {
        let dir = temp_dir("refuse");
        let path = dir.join("config.toml");
        for text in [
            "# mine\n[notifications]\nshow_previews = true\n",
            "[app]\nbackground = \"yes\"\n",
            "[app\n",
        ] {
            std::fs::write(&path, text).expect("write");
            let err = Config::default().save(&path).expect_err(text);
            assert!(matches!(err, SaveError::Invalid(_)), "{err}");
            assert!(err.to_string().contains("not overwriting"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).expect("read"), text);
        }
        let names: Vec<_> = std::fs::read_dir(&dir).expect("dir").collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
    }

    #[test]
    fn a_serialisation_error_writes_nothing() {
        // `toml::to_string` cannot fail on these plain structs, so the error path is driven
        // through the seam `save` uses, handing it the failed serialisation directly.
        let failed = || Err(<toml::ser::Error as serde::ser::Error>::custom("boom"));
        let dir = temp_dir("serialise");
        let path = dir.join("config.toml");
        let mine = "[app]\nstart_hidden = true\n";
        std::fs::write(&path, mine).expect("write");
        let err = Config::write_text(&path, failed(), &mut sync_dir).expect_err("nothing to write");
        assert!(matches!(err, SaveError::Serialize(_)), "{err}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), mine);
        let names: Vec<_> = std::fs::read_dir(&dir).expect("dir").collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
        // Nor is a missing file or its directory created.
        let fresh = dir.join("nuncio").join("config.toml");
        Config::write_text(&fresh, failed(), &mut sync_dir).expect_err("nothing to write");
        assert!(!dir.join("nuncio").exists());
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_leaves_no_temporary_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("readonly");
        let path = dir.join("config.toml");
        std::fs::write(&path, "[app]\nstart_hidden = true\n").expect("write");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        let result = Config::default().save(&path);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let err = result.expect_err("a read-only directory cannot take the new file");
        assert!(matches!(err, SaveError::Write { .. }), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "[app]\nstart_hidden = true\n"
        );
        assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 1);
    }

    #[test]
    fn concurrent_saves_all_succeed_and_leave_one_whole_file() {
        let dir = temp_dir("concurrent");
        let path = dir.join("config.toml");
        let configs = every_kind_of_config();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                let config = configs[i % configs.len()].clone();
                std::thread::spawn(move || {
                    (0..25)
                        .map(|_| config.save(&path).map_err(|e| e.to_string()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for thread in threads {
            for result in thread.join().expect("thread") {
                assert_eq!(result, Ok(()));
            }
        }
        let saved = Config::load(&path).expect("one whole file");
        assert!(configs.contains(&saved));
        let names: Vec<_> = std::fs::read_dir(&*dir)
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["config.toml"], "no temporary file left");
    }

    #[test]
    fn a_symlink_to_nothing_is_an_error_and_creates_nothing() {
        let dir = temp_dir("dangling");
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink(dir.join("gone").join("config.toml"), &link).expect("symlink");
        let err = Config::default().save(&link).expect_err("no target");
        assert!(matches!(err, SaveError::Write { .. }), "{err}");
        assert!(!dir.join("gone").exists(), "nothing is created");
        assert_eq!(
            std::fs::read_dir(&*dir).expect("dir").count(),
            1,
            "only the link"
        );
    }

    #[test]
    fn a_temporary_name_already_taken_is_skipped_and_never_removed() {
        let dir = temp_dir("taken");
        let path = dir.join("config.toml");
        // The next names this process would use, taken by someone else.
        let next = TEMP_COUNTER.load(std::sync::atomic::Ordering::Relaxed);
        let taken: Vec<PathBuf> = (next..next + 50)
            .map(|count| dir.join(temp_name("config.toml", count)))
            .collect();
        for file in &taken {
            std::fs::write(file, "someone else's").expect("write");
        }
        Config::default()
            .save(&path)
            .expect("saves under a free name");
        assert_eq!(Config::load(&path).expect("loads"), Config::default());
        for file in &taken {
            assert_eq!(
                std::fs::read_to_string(file).expect("still there"),
                "someone else's"
            );
        }
    }

    #[test]
    fn saving_through_a_symlink_rewrites_its_target_and_keeps_the_link() {
        let dir = temp_dir("symlink");
        let target = dir.join("dotfiles-config.toml");
        std::fs::write(&target, "").expect("write");
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let mut config = Config::default();
        config.app.start_hidden = true;
        config.save(&link).expect("saves");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink(),
            "the link stays a link"
        );
        assert!(Config::load(&target).expect("loads").app.start_hidden);
    }

    #[test]
    fn a_save_syncs_the_directory_of_the_resolved_target_once() {
        let dir = temp_dir("dirsync");
        let real = dir.join("dotfiles");
        std::fs::create_dir(&real).expect("dir");
        let target = real.join("config.toml");
        std::fs::write(&target, "").expect("write");
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let mut synced = Vec::new();
        let mut config = Config::default();
        config.app.start_hidden = true;
        Config::write_text(&link, config.to_toml_string(), &mut |dir: &Path| {
            synced.push(dir.to_owned());
            Ok(())
        })
        .expect("saves");
        let resolved = std::fs::canonicalize(&real).expect("canonical");
        assert_eq!(synced, vec![resolved]);
        assert!(Config::load(&link).expect("loads").app.start_hidden);
    }

    #[test]
    fn a_directory_that_cannot_be_synced_still_saves() {
        let dir = temp_dir("dirsync-fails");
        let path = dir.join("config.toml");
        let mut calls = 0;
        let mut config = Config::default();
        config.app.start_hidden = true;
        Config::write_text(&path, config.to_toml_string(), &mut |_: &Path| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        })
        .expect("an unsupported directory sync is not a failed save");
        assert_eq!(calls, 1);
        assert_eq!(Config::load(&path).expect("loads"), config);
    }

    #[test]
    fn the_real_directory_sync_works_on_a_directory() {
        let dir = temp_dir("dirsync-real");
        sync_dir(&dir).expect("a directory can be synced here");
    }
}
