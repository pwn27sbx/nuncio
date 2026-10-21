//! The `nuncio-desktop` binary.
//!
//! Two modes:
//!
//! - normal: open the window (chat list, conversation, composer) on `nunciod` at
//!   `$XDG_RUNTIME_DIR/nuncio/core.sock`, reconnecting when it goes away, and follow the live
//!   Omarchy theme; show notifications for new messages (a click opens that chat), keep running
//!   in the background when the window is closed (`[app] background`) until Quit, and open the
//!   `whatsapp:` / `wa.me` links given as arguments or through `open`;
//! - `--probe`: the S5 acceptance check without a window on screen. It applies a palette, performs
//!   the same atomic theme swap `omarchy-theme-set` does, and prints the colour GTK resolves
//!   before and after plus how long the detection took. `--theme-dir` points it at a fixture tree
//!   so the check never touches the running theme.

use adw::prelude::*;
use gtk::gio;
use nuncio_client::{Backoff, Config, DEFAULT_READ_TIMEOUT, Palette, ThemeWatcher, socket_path};
use nuncio_desktop::controller::Controller;
use nuncio_desktop::lifecycle::Lifecycle;
use nuncio_desktop::present::shows_title_bar;
use nuncio_desktop::window::{Ui, WeakUi};
use nuncio_desktop::worker::Settings;
use nuncio_desktop::{ROOT_CLASS, apply_to_display, css_for, resolved_foreground};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "io.github.pwn27sbx.nuncio";
const CLIENT_NAME: &str = concat!("nuncio-desktop/", env!("CARGO_PKG_VERSION"));

struct Args {
    theme_dir: Option<PathBuf>,
    probe: bool,
    /// `whatsapp:` / `wa.me` links to open (as a browser hands them over).
    links: Vec<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        theme_dir: None,
        probe: false,
        links: Vec::new(),
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--probe" => args.probe = true,
            "--theme-dir" => args.theme_dir = argv.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!(
                    "usage: nuncio-desktop [--theme-dir DIR] [--probe] [LINK...]\n\n  \
                     --theme-dir DIR  directory holding colors.toml (default: Omarchy's)\n  \
                     --probe          run the S5 theme-switch check and exit\n  \
                     LINK             a whatsapp://send?phone=... or https://wa.me/... link: opens\n                   \
                     that chat with the link's text in the composer (never sent)"
                );
                std::process::exit(0);
            }
            other if !other.starts_with('-') => args.links.push(other.to_owned()),
            other => {
                eprintln!("nuncio-desktop: unexpected argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    args
}

/// The palette file to follow: the explicit `--theme-dir`, else Omarchy's own.
fn palette_path(args: &Args) -> PathBuf {
    match &args.theme_dir {
        Some(dir) => dir.join("colors.toml"),
        None => Palette::default_path().expect("HOME or XDG_STATE_HOME must be set"),
    }
}

fn main() -> glib::ExitCode {
    let args = parse_args();
    let palette_file = palette_path(&args);

    if args.probe {
        return probe(&palette_file);
    }

    // Read before any window, so a typo stops the app with its reason.
    let config = match Config::default_path().map(|path| Config::load(&path)) {
        Some(Ok(config)) => config,
        None => Config::default(),
        Some(Err(error)) => {
            eprintln!("nuncio-desktop: {error}");
            return glib::ExitCode::from(2);
        }
    };
    let title_bar = shows_title_bar(
        config.app.title_bar,
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    );

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // Links arrive as `open` (the command line's, or a desktop's D-Bus `Open`).
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let shown: Rc<RefCell<Option<WeakUi>>> = Rc::default();
    let lifecycle = Rc::new(RefCell::new(Lifecycle::new(&config.app)));
    let hold: Rc<RefCell<Option<gio::ApplicationHoldGuard>>> = Rc::default();

    // Everything is built at startup, before `activate`, `open` or an action (a notification
    // clicked at a cold start activates `app.open-chat` with no `activate`).
    app.connect_startup({
        let shown = Rc::clone(&shown);
        let lifecycle = Rc::clone(&lifecycle);
        let hold = Rc::clone(&hold);
        move |app| {
            let mut controller = Controller::new(client_prefix());
            controller.set_config(&config);
            let ui = Ui::new(Some(app), controller, local_offset_minutes(), title_bar);
            ui.set_lifecycle(Lifecycle::new(&config.app));
            if lifecycle.borrow().holds() {
                // Keep running with the window hidden; released by Quit.
                *hold.borrow_mut() = Some(app.hold());
            }

            let palette = Palette::load_or_default(Some(&palette_file));
            apply_to_display(&gtk::gdk::Display::default().expect("a display"), &palette);
            follow_theme(&palette_file);
            connect(&ui);

            app.add_action(&ui.open_chat_action({
                let lifecycle = Rc::clone(&lifecycle);
                move |ui| {
                    lifecycle.borrow_mut().shown();
                    ui.window().present();
                }
            }));
            let quit = gio::SimpleAction::new("quit", None);
            quit.connect_activate({
                let weak = ui.downgrade();
                let hold = Rc::clone(&hold);
                let app = app.downgrade();
                move |_, _| {
                    hold.borrow_mut().take();
                    if let Some(ui) = weak.upgrade() {
                        ui.quit();
                    }
                    if let Some(app) = app.upgrade() {
                        app.quit();
                    }
                }
            });
            app.add_action(&quit);
            app.set_accels_for_action("app.quit", &["<Control>q"]);
            *shown.borrow_mut() = Some(ui.downgrade());
        }
    });
    app.connect_activate({
        let shown = Rc::clone(&shown);
        let lifecycle = Rc::clone(&lifecycle);
        move |_| {
            // A second launch only brings the window back; `[app] start_hidden` keeps the first
            // one hidden.
            if let Some(ui) = shown.borrow().as_ref().and_then(WeakUi::upgrade)
                && lifecycle.borrow_mut().activate()
            {
                ui.window().present();
            }
        }
    });
    app.connect_open({
        let shown = Rc::clone(&shown);
        let lifecycle = Rc::clone(&lifecycle);
        move |_, files, _| {
            let Some(ui) = shown.borrow().as_ref().and_then(WeakUi::upgrade) else {
                return;
            };
            for file in files {
                ui.open_link(&file.uri());
            }
            lifecycle.borrow_mut().shown();
            ui.window().present();
        }
    });

    // GTK must not see this program's own options; the links go on as `open`.
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "nuncio-desktop".to_owned());
    app.run_with_args(&std::iter::once(argv0).chain(args.links).collect::<Vec<_>>())
}

/// Start the daemon worker on the socket `nunciod` listens on.
fn connect(ui: &Ui) {
    match socket_path() {
        Ok(path) => ui.start_worker(Settings {
            path,
            client: CLIENT_NAME.to_owned(),
            timeout: DEFAULT_READ_TIMEOUT,
            backoff: Backoff::default(),
        }),
        Err(error) => eprintln!("nuncio-desktop: cannot reach nunciod: {error}"),
    }
}

/// Follow live theme switches: the watcher owns its own thread, so hand events back to the main
/// loop before touching any widget.
fn follow_theme(palette_file: &Path) {
    match ThemeWatcher::start(palette_file) {
        Ok(watcher) => {
            let context = glib::MainContext::default();
            // `None` is only a quiet second: keep waiting (stopping there, as the M0 spike did,
            // stopped following the theme a second after the start).
            std::thread::spawn(move || {
                loop {
                    let Some(palette) = watcher.recv_timeout(Duration::from_secs(1)) else {
                        continue;
                    };
                    context.invoke(move || {
                        if let Some(display) = gtk::gdk::Display::default() {
                            apply_to_display(&display, &palette);
                        }
                    });
                }
            });
        }
        Err(err) => eprintln!("nuncio-desktop: not following the theme: {err}"),
    }
}

/// The prefix of this run's `client_id`s: it must differ between runs, so a new send never repeats
/// an old `client_id`.
fn client_prefix() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    format!("desktop-{}-{seconds}", std::process::id())
}

/// The local offset from UTC in minutes, read once at startup (as the terminal client does).
fn local_offset_minutes() -> i32 {
    glib::DateTime::now_local()
        .map(|now| i32::try_from(now.utc_offset().as_seconds() / 60).unwrap_or(0))
        .unwrap_or(0)
}

/// Write a palette the way Omarchy does, so the fixture tree looks like the real one.
fn write_palette(theme_dir: &Path, mode: &str, background: &str, foreground: &str) {
    std::fs::create_dir_all(theme_dir).expect("create theme dir");
    std::fs::write(
        theme_dir.join("colors.toml"),
        format!(
            "mode = \"{mode}\"\nbackground = \"{background}\"\nforeground = \"{foreground}\"\n\
             accent = \"#268bd3\"\nred = \"#db302d\"\n"
        ),
    )
    .expect("write palette");
}

/// S5's acceptance check, with no window on screen.
///
/// It carries an explicit colour in `colors.toml` and asks GTK what it resolved for a widget, so
/// the evidence is the style engine's answer and not the string we handed it.
fn probe(colors_toml: &Path) -> glib::ExitCode {
    let theme_dir = colors_toml
        .parent()
        .expect("colors.toml has a parent")
        .to_path_buf();
    let current = theme_dir
        .parent()
        .expect("theme dir has a parent")
        .to_path_buf();
    let next = current.join("next-theme");

    write_palette(&theme_dir, "dark", "#001419", "#839395");

    gtk::init().expect("GTK initialises");
    let display = gtk::gdk::Display::default().expect("a display");
    let window = gtk::Window::builder().title("nuncio probe").build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class(ROOT_CLASS);
    let label = gtk::Label::new(Some("probe"));
    root.append(&label);
    window.set_child(Some(&root));
    // Realize without presenting: the style engine resolves, but no window lands on the desktop.
    gtk::prelude::WidgetExt::realize(&window);

    let dark = Palette::load(colors_toml).expect("fixture palette loads");
    apply_to_display(&display, &dark);
    let before = resolved_foreground(&label);

    let watcher = match ThemeWatcher::start(colors_toml) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!("probe: cannot watch {colors_toml:?}: {err}");
            return glib::ExitCode::FAILURE;
        }
    };
    // Let inotify settle so the measurement is detection, not setup.
    std::thread::sleep(Duration::from_millis(250));

    // Exactly what `omarchy-theme-set` does: stage, swap the directory, rewrite the name.
    write_palette(&next, "light", "#fdf6e3", "#26363d");
    let started = Instant::now();
    std::fs::remove_dir_all(&theme_dir).expect("remove old theme");
    std::fs::rename(&next, &theme_dir).expect("swap theme in");
    std::fs::write(current.join("theme.name"), "probe-light\n").expect("rewrite theme.name");

    let Some(light) = watcher.recv_timeout(Duration::from_millis(500)) else {
        eprintln!("probe: FAIL — no theme switch detected within 500 ms");
        return glib::ExitCode::FAILURE;
    };
    let elapsed = started.elapsed();
    apply_to_display(&display, &light);
    let after = resolved_foreground(&label);

    println!("probe theme dir:  {}", theme_dir.display());
    println!(
        "applied before:   mode={:?} fg={}",
        dark.mode,
        dark.foreground.to_hex()
    );
    println!(
        "GTK resolved:     {}",
        before.map_or("(none)".to_string(), |c| c.to_hex())
    );
    println!(
        "applied after:    mode={:?} fg={}",
        light.mode,
        light.foreground.to_hex()
    );
    println!(
        "GTK resolved:     {}",
        after.map_or("(none)".to_string(), |c| c.to_hex())
    );
    println!("switch detected:  {elapsed:?}");
    println!("generated css:    {} bytes", css_for(&light).len());

    let flipped = before != after && after == Some(light.foreground);
    println!(
        "verdict:          {} ({elapsed:?}, budget 500 ms)",
        if flipped && elapsed < Duration::from_millis(500) {
            "PASS — GTK repainted in the new palette"
        } else if elapsed < Duration::from_millis(500) {
            "PARTIAL — switch detected in time, GTK colour did not visibly change"
        } else {
            "FAIL — too slow"
        }
    );

    glib::ExitCode::SUCCESS
}
