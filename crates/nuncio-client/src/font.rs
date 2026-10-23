//! Following the Omarchy font.
//!
//! Omarchy has one font setting, the system **monospace** font, and fontconfig is its source of
//! truth (Omarchy's `bin/omarchy-font-set` and `bin/omarchy-font-current`):
//!
//! - the package-owned default (`default/fontconfig/conf.avail/50-omarchy.conf`) maps the
//!   `monospace` family to `JetBrainsMono Nerd Font`;
//! - `omarchy-font-set <name>` rewrites `~/.config/fontconfig/fonts.conf` with a `<match>` that
//!   prepends `<name>` to `monospace`, which wins over the default;
//! - `omarchy-font-current` asks fontconfig what `monospace` resolves to:
//!   `fc-match monospace -f '%{family}\n' | head -n1 | cut -d, -f1`.
//!
//! This module asks fontconfig the same question ([`current_font`], parsed by
//! [`parse_fc_match`]) and follows a font switch live ([`FontWatcher`]) by watching the
//! directory `omarchy-font-set` writes to. It also holds the small process runner ([`Run`],
//! [`run_captured`]) the desktop app shares for its other one-shot desktop queries (`hyprctl`):
//! one place that bounds how long a desktop helper may take.

use std::env;
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::Watcher as _;

/// The program and arguments `omarchy-font-current` runs.
pub const FC_MATCH: (&str, [&str; 3]) = ("fc-match", ["monospace", "-f", "%{family}\n"]);

/// The longest family name accepted; anything longer is not a font name.
const MAX_FAMILY_LEN: usize = 256;

/// At most this much of a helper's output is read.
const MAX_OUTPUT: u64 = 64 * 1024;

/// How long to wait after a filesystem event before asking fontconfig again: `omarchy-font-set`
/// creates the directory and then writes the file, so one switch is a burst of events.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Why a desktop helper gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0} is not installed")]
    NotFound(String),
    #[error("{program} did not answer within {timeout:?}")]
    TimedOut { program: String, timeout: Duration },
    #[error("{program} failed: {status}")]
    Failed { program: String, status: ExitStatus },
    #[error("cannot run {program}: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

impl RunError {
    /// The helper was running but too slow (a cold fontconfig cache at login, say): worth asking
    /// again with more time, unlike a missing or failing helper.
    pub fn timed_out(&self) -> bool {
        matches!(self, Self::TimedOut { .. })
    }
}

/// Runs a desktop helper and returns its standard output. Tests use a fake.
pub trait Run: Send + Sync {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, RunError>;
}

/// The real runner: [`run_captured`].
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Run for System {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, RunError> {
        run_captured(program, args, timeout)
    }
}

/// Run `program` with `args`, no stdin and stderr discarded, and return its standard output
/// (at most 64 KiB) if it exits successfully within `timeout`. A helper still running at the
/// deadline is killed, so a hung helper never holds the caller longer than `timeout`.
pub fn run_captured(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, RunError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound(program.to_owned()),
            _ => RunError::Io {
                program: program.to_owned(),
                source,
            },
        })?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(MAX_OUTPUT).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    loop {
        let waited = child.try_wait().map_err(|source| RunError::Io {
            program: program.to_owned(),
            source,
        })?;
        match waited {
            Some(status) if status.success() => return Ok(reader.join().unwrap_or_default()),
            Some(status) => {
                return Err(RunError::Failed {
                    program: program.to_owned(),
                    status,
                });
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut {
                    program: program.to_owned(),
                    timeout,
                });
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// The family in `fc-match monospace -f '%{family}\n'` output: the first line's first
/// comma-separated entry, trimmed, as `omarchy-font-current` takes it.
///
/// `None` for empty output, an empty first entry, and anything that is not a plausible family
/// name (control characters, over 256 bytes), so garbage never reaches the stylesheet.
pub fn parse_fc_match(stdout: &str) -> Option<String> {
    let family = stdout.lines().next()?.split(',').next()?.trim();
    if family.is_empty() || family.len() > MAX_FAMILY_LEN || family.chars().any(char::is_control) {
        return None;
    }
    Some(family.to_owned())
}

/// The family fontconfig resolves `monospace` to — Omarchy's current font — asked through
/// `runner` with `timeout`. `Ok(None)` when fontconfig answered nothing usable.
pub fn current_font(runner: &dyn Run, timeout: Duration) -> Result<Option<String>, RunError> {
    let (program, args) = FC_MATCH;
    let out = runner.run(program, &args, timeout)?;
    Ok(std::str::from_utf8(&out).ok().and_then(parse_fc_match))
}

/// `~/.config/fontconfig`, honouring `XDG_CONFIG_HOME`: where `omarchy-font-set` writes
/// `fonts.conf`.
pub fn fontconfig_dir() -> Option<PathBuf> {
    fontconfig_dir_from(
        env::var_os("XDG_CONFIG_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

/// [`fontconfig_dir`] for a given `$XDG_CONFIG_HOME` and `$HOME` (an empty value counts as
/// unset).
pub fn fontconfig_dir_from(config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = match config_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(home.filter(|dir| !dir.is_empty())?).join(".config"),
    };
    Some(base.join("fontconfig"))
}

/// Why the font cannot be followed.
#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("cannot watch {path}: {source}")]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
    #[error("{0} has no parent directory to watch for a font switch")]
    NoWatchRoot(PathBuf),
}

/// Follows a live Omarchy font switch.
///
/// It watches the fontconfig directory (`~/.config/fontconfig`) and its parent: a fresh Omarchy
/// has no such directory until the first `omarchy-font-set` creates it, and the parent's watch
/// sees it appear (the new directory is then watched too). Every event that touches the
/// fontconfig directory re-asks `query` (normally [`current_font`]) after a short debounce, and a
/// family different from the last one is sent.
pub struct FontWatcher {
    _watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    events: mpsc::Sender<()>,
    rx: mpsc::Receiver<Option<String>>,
}

impl FontWatcher {
    /// Start watching `fontconfig_dir`. `initial` is the family already applied, so only a
    /// change is sent.
    pub fn start<Q>(
        fontconfig_dir: &Path,
        initial: Option<String>,
        query: Q,
    ) -> Result<Self, FontError>
    where
        Q: Fn() -> Option<String> + Send + 'static,
    {
        let parent = fontconfig_dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| FontError::NoWatchRoot(fontconfig_dir.to_owned()))?
            .to_owned();
        let dir = fontconfig_dir.to_owned();

        let (events_tx, events_rx) = mpsc::channel::<()>();
        let events = events_tx.clone();
        // Events name the canonical path on some platforms (macOS reports `/private/var/...`
        // for `/var/...`), so match both spellings of the directory.
        let mut relevant = vec![dir.clone()];
        if let (Ok(canonical), Some(name)) = (parent.canonicalize(), dir.file_name()) {
            relevant.push(canonical.join(name));
        }
        let watcher =
            notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                let Ok(event) = event else { return };
                let touches = |path: &PathBuf| relevant.iter().any(|dir| path.starts_with(dir));
                if event.paths.iter().any(touches) {
                    let _ = events_tx.send(());
                }
            })
            .map_err(|source| FontError::Watch {
                path: parent.clone(),
                source,
            })?;
        let watcher = Arc::new(Mutex::new(watcher));
        {
            let mut guard = watcher.lock().expect("font watcher mutex");
            guard
                .watch(&parent, notify::RecursiveMode::NonRecursive)
                .map_err(|source| FontError::Watch {
                    path: parent.clone(),
                    source,
                })?;
        }
        let armed = dir.is_dir() && arm(&watcher, &dir);

        let (tx, rx) = mpsc::channel();
        let weak = Arc::downgrade(&watcher);
        std::thread::spawn(move || {
            let mut armed = armed;
            let mut last = initial;
            // Ends when this `FontWatcher` (with the watcher and both event senders) is dropped.
            while events_rx.recv().is_ok() {
                std::thread::sleep(DEBOUNCE);
                while events_rx.try_recv().is_ok() {}
                if !armed && dir.is_dir() {
                    let Some(watcher) = weak.upgrade() else {
                        return;
                    };
                    armed = arm(&watcher, &dir);
                }
                let family = query();
                if family == last {
                    continue;
                }
                last = family.clone();
                if tx.send(family).is_err() {
                    return;
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            events,
            rx,
        })
    }

    /// Ask fontconfig again now, as a filesystem event would (after the same debounce); a
    /// family different from the last one is sent. For a start-up query that timed out.
    pub fn ask_again(&self) {
        let _ = self.events.send(());
    }

    /// Wait for the next family. `None` means the watcher's thread has ended (nothing more will
    /// ever arrive), so a forwarding loop stops instead of spinning.
    pub fn recv(&self) -> Option<Option<String>> {
        self.rx.recv().ok()
    }

    /// Wait up to `timeout` for the next family. `None` means nothing changed in time;
    /// `Some(None)` means the font is no longer known (keep the toolkit's default).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Option<String>> {
        self.rx.recv_timeout(timeout).ok()
    }
}

/// Watch the fontconfig directory itself; `false` when it cannot be (it is retried on the next
/// event).
fn arm(watcher: &Mutex<notify::RecommendedWatcher>, dir: &Path) -> bool {
    watcher
        .lock()
        .expect("font watcher mutex")
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        FC_MATCH, FontError, FontWatcher, Run, RunError, current_font, fontconfig_dir_from,
        parse_fc_match, run_captured,
    };
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant, SystemTime};

    /// What `fc-match monospace -f '%{family}\n'` prints on a stock Omarchy: the family and its
    /// alias, comma-separated (as `omarchy-font-current`'s comment shows).
    const OMARCHY_DEFAULT: &str = "JetBrainsMono Nerd Font,JetBrainsMono NF\n";

    #[test]
    fn the_first_family_of_the_real_output_is_the_font() {
        assert_eq!(
            parse_fc_match(OMARCHY_DEFAULT).as_deref(),
            Some("JetBrainsMono Nerd Font")
        );
        assert_eq!(
            parse_fc_match("CaskaydiaMono Nerd Font\n").as_deref(),
            Some("CaskaydiaMono Nerd Font")
        );
        assert_eq!(
            parse_fc_match("  Iosevka Nerd Font  \nDejaVu Sans Mono\n").as_deref(),
            Some("Iosevka Nerd Font")
        );
    }

    #[test]
    fn missing_empty_and_garbage_output_is_no_font() {
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("\n"), None);
        assert_eq!(parse_fc_match(",JetBrainsMono NF\n"), None);
        assert_eq!(parse_fc_match("\u{1b}[31mFont\n"), None);
        assert_eq!(parse_fc_match("Mono\u{0}Font\n"), None);
        assert_eq!(parse_fc_match(&"x".repeat(300)), None);
    }

    struct Fake {
        answer: fn() -> Result<Vec<u8>, RunError>,
        calls: AtomicUsize,
    }

    impl Run for Fake {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<Vec<u8>, RunError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!((program, args), (FC_MATCH.0, &FC_MATCH.1[..]));
            (self.answer)()
        }
    }

    #[test]
    fn the_current_font_is_what_fontconfig_resolves_monospace_to() {
        let fake = Fake {
            answer: || Ok(OMARCHY_DEFAULT.as_bytes().to_vec()),
            calls: AtomicUsize::new(0),
        };
        let font = current_font(&fake, Duration::from_millis(200)).expect("answered");
        assert_eq!(font.as_deref(), Some("JetBrainsMono Nerd Font"));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        let garbage = Fake {
            answer: || Ok(vec![0xff, 0xfe, b'\n']),
            calls: AtomicUsize::new(0),
        };
        assert_eq!(
            current_font(&garbage, Duration::from_millis(200)).expect("answered"),
            None
        );

        let missing = Fake {
            answer: || Err(RunError::NotFound("fc-match".to_owned())),
            calls: AtomicUsize::new(0),
        };
        assert!(matches!(
            current_font(&missing, Duration::from_millis(200)),
            Err(RunError::NotFound(_))
        ));
    }

    #[test]
    fn the_fontconfig_directory_follows_the_xdg_base_directory_spec() {
        let home = OsStr::new("/home/owner");
        assert_eq!(
            fontconfig_dir_from(Some(OsStr::new("/cfg")), Some(home)).as_deref(),
            Some(Path::new("/cfg/fontconfig"))
        );
        assert_eq!(
            fontconfig_dir_from(Some(OsStr::new("")), Some(home)).as_deref(),
            Some(Path::new("/home/owner/.config/fontconfig"))
        );
        assert_eq!(fontconfig_dir_from(None, None), None);
    }

    #[test]
    fn a_helper_is_run_and_its_output_returned() {
        let out = run_captured(
            "sh",
            &["-c", "printf 'Mono Font\\n'"],
            Duration::from_secs(5),
        )
        .expect("sh runs");
        assert_eq!(out, b"Mono Font\n");
        assert!(matches!(
            run_captured("sh", &["-c", "exit 3"], Duration::from_secs(5)),
            Err(RunError::Failed { .. })
        ));
        assert!(matches!(
            run_captured("nuncio-no-such-helper", &[], Duration::from_millis(200)),
            Err(RunError::NotFound(_))
        ));
    }

    #[test]
    fn a_hung_helper_is_killed_at_the_deadline() {
        let started = Instant::now();
        let result = run_captured("sleep", &["5"], Duration::from_millis(100));
        assert!(
            matches!(result, Err(RunError::TimedOut { .. })),
            "{result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    fn unique_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nuncio-font-{label}-{}-{stamp}",
            std::process::id()
        ))
    }

    /// A stand-in for fontconfig: the "resolved" family is the first line of `fonts.conf`.
    fn fake_fc_match(
        dir: &Path,
        calls: &Arc<AtomicUsize>,
    ) -> impl Fn() -> Option<String> + 'static {
        let file = dir.join("fonts.conf");
        let calls = Arc::clone(calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            fs::read_to_string(&file)
                .ok()
                .and_then(|text| parse_fc_match(&text))
        }
    }

    fn wait_for(watcher: &FontWatcher, want: Option<&str>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(got) = watcher.recv_timeout(Duration::from_millis(100)) {
                assert_eq!(got.as_deref(), want);
                return;
            }
        }
        panic!("no font change noticed; wanted {want:?}");
    }

    #[test]
    fn a_font_switch_is_noticed() {
        let root = unique_root("switch");
        let dir = root.join("fontconfig");
        fs::create_dir_all(&dir).expect("create fontconfig dir");
        fs::write(dir.join("fonts.conf"), "JetBrainsMono Nerd Font\n").expect("write");
        let calls = Arc::new(AtomicUsize::new(0));

        let watcher = FontWatcher::start(
            &dir,
            Some("JetBrainsMono Nerd Font".to_owned()),
            fake_fc_match(&dir, &calls),
        )
        .expect("watcher starts");
        std::thread::sleep(Duration::from_millis(250));

        fs::write(dir.join("fonts.conf"), "CaskaydiaMono Nerd Font\n").expect("rewrite");
        wait_for(&watcher, Some("CaskaydiaMono Nerd Font"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_first_font_switch_on_a_fresh_omarchy_is_noticed() {
        let root = unique_root("fresh");
        fs::create_dir_all(&root).expect("create config home");
        let dir = root.join("fontconfig");
        let calls = Arc::new(AtomicUsize::new(0));

        let watcher = FontWatcher::start(&dir, None, fake_fc_match(&dir, &calls))
            .expect("watcher starts without the directory");
        std::thread::sleep(Duration::from_millis(250));

        // What `omarchy-font-set` does: `mkdir -p`, then write the file.
        fs::create_dir_all(&dir).expect("create fontconfig dir");
        fs::write(dir.join("fonts.conf"), "Iosevka Nerd Font\n").expect("write");
        wait_for(&watcher, Some("Iosevka Nerd Font"));

        // And the directory, now watched, carries the next switch too.
        std::thread::sleep(Duration::from_millis(250));
        fs::write(dir.join("fonts.conf"), "Fira Code\n").expect("rewrite");
        wait_for(&watcher, Some("Fira Code"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unchanged_font_and_unrelated_files_are_not_sent() {
        let root = unique_root("quiet");
        let dir = root.join("fontconfig");
        fs::create_dir_all(&dir).expect("create fontconfig dir");
        fs::write(dir.join("fonts.conf"), "Fira Code\n").expect("write");
        let calls = Arc::new(AtomicUsize::new(0));

        let watcher = FontWatcher::start(
            &dir,
            Some("Fira Code".to_owned()),
            fake_fc_match(&dir, &calls),
        )
        .expect("watcher starts");
        std::thread::sleep(Duration::from_millis(250));

        // Another program's config next to fontconfig: fontconfig is not even asked.
        fs::write(root.join("other.conf"), "x").expect("write sibling");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "fontconfig asked for nothing"
        );

        // The same family rewritten: asked, but not sent.
        fs::write(dir.join("fonts.conf"), "Fira Code\n").expect("rewrite");
        assert!(
            watcher.recv_timeout(Duration::from_millis(800)).is_none(),
            "an unchanged font must not be re-sent"
        );
        assert!(calls.load(Ordering::SeqCst) >= 1, "fontconfig was asked");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_a_slow_fontconfig_is_worth_asking_again() {
        let slow = Fake {
            answer: || {
                Err(RunError::TimedOut {
                    program: "fc-match".to_owned(),
                    timeout: Duration::from_millis(200),
                })
            },
            calls: AtomicUsize::new(0),
        };
        let error = current_font(&slow, Duration::from_millis(200)).expect_err("timed out");
        assert!(error.timed_out(), "{error}");

        let missing = Fake {
            answer: || Err(RunError::NotFound("fc-match".to_owned())),
            calls: AtomicUsize::new(0),
        };
        let error = current_font(&missing, Duration::from_millis(200)).expect_err("missing");
        assert!(!error.timed_out(), "{error}");
    }

    #[test]
    fn asking_again_sends_the_font_without_a_filesystem_event() {
        let root = unique_root("again");
        fs::create_dir_all(&root).expect("create config home");
        let dir = root.join("fontconfig");
        let fake = Arc::new(Fake {
            answer: || Ok(OMARCHY_DEFAULT.as_bytes().to_vec()),
            calls: AtomicUsize::new(0),
        });

        let query = {
            let fake = Arc::clone(&fake);
            move || current_font(&*fake, Duration::from_secs(2)).ok().flatten()
        };
        let watcher = FontWatcher::start(&dir, None, query).expect("watcher starts");
        watcher.ask_again();
        wait_for(&watcher, Some("JetBrainsMono Nerd Font"));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recv_returns_when_the_watcher_thread_ends() {
        let root = unique_root("ended");
        fs::create_dir_all(&root).expect("create config home");
        let dir = root.join("fontconfig");
        let watcher = FontWatcher::start(&dir, None, || panic!("fontconfig query crashed"))
            .expect("watcher starts");
        // The first query ends the watcher's thread.
        watcher.ask_again();
        std::thread::sleep(Duration::from_millis(250));
        fs::create_dir_all(&dir).expect("create fontconfig dir");

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
    fn a_directory_without_a_parent_is_rejected() {
        let err = match FontWatcher::start(Path::new("fontconfig"), None, || None) {
            Err(err) => err,
            Ok(_) => panic!("a directory with no parent must be rejected"),
        };
        assert!(matches!(err, FontError::NoWatchRoot(_)), "{err}");
    }
}
