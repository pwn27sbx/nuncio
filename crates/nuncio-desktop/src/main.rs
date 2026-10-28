//! The `nuncio-desktop` binary.
//!
//! Two modes:
//!
//! - normal: open the window (chat list, conversation, composer) on `nunciod` at
//!   `$XDG_RUNTIME_DIR/nuncio/core.sock`, reconnecting when it goes away, and follow the live
//!   Omarchy theme; show notifications for new messages (a click opens that chat), keep running
//!   in the background when the window is closed (`[app] background`) until Quit, and open the
//!   `whatsapp:` / `wa.me` links given as arguments or through `open`; a command palette (F1,
//!   Ctrl+K) and a settings window that writes `config.toml`;
//! - `--probe`: the S5 acceptance check without a window on screen. It applies a palette, performs
//!   the same atomic theme swap `omarchy-theme-set` does, and prints the colour GTK resolves
//!   before and after plus how long the detection took. `--theme-dir` points it at a fixture tree
//!   so the check never touches the running theme.

use adw::prelude::*;
use gtk::gio;
use nuncio_client::driver::SEND_WAIT;
use nuncio_client::font::{self, FontWatcher, System};
use nuncio_client::{Backoff, Config, DEFAULT_READ_TIMEOUT, Palette, ThemeWatcher, socket_path};
use nuncio_desktop::args::{self, APP_ID, Args, Parsed};
use nuncio_desktop::controller::Controller;
use nuncio_desktop::hyprland;
use nuncio_desktop::lifecycle::Lifecycle;
use nuncio_desktop::window::{Ui, WeakUi, title_bar_here};
use nuncio_desktop::worker::Settings;
use nuncio_desktop::{
    Look, ROOT_CLASS, apply_look, apply_to_display, css_for, omarchy_here, resolved_foreground,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLIENT_NAME: &str = concat!("nuncio-desktop/", env!("CARGO_PKG_VERSION"));

fn parse_args() -> Args {
    match args::parse(std::env::args().skip(1)) {
        Parsed::Run(args) => args,
        Parsed::Help => {
            println!("{}", args::USAGE);
            std::process::exit(0);
        }
        Parsed::Unexpected(other) => {
            eprintln!(
                "nuncio-desktop: unexpected argument {other:?}\n\n{}",
                args::USAGE
            );
            std::process::exit(2);
        }
    }
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

    // The daemon comes back after a logout without the user ever typing `systemctl`: enabling a
    // unit of the *user* manager needs no root, and the call is best effort.
    nuncio_client::bus::enable_daemon_unit();

    // Read before any window, so a typo stops the app with its reason.
    let config = match Config::default_path().map(|path| Config::load(&path)) {
        Some(Ok(config)) => config,
        None => Config::default(),
        Some(Err(error)) => {
            eprintln!("nuncio-desktop: {error}");
            return glib::ExitCode::from(2);
        }
    };
    let title_bar = title_bar_here(config.app.title_bar);

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // Links arrive as `open` (the command line's, or a desktop's D-Bus `Open`).
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let shown: Rc<RefCell<Option<WeakUi>>> = Rc::default();
    let lifecycle = Rc::new(RefCell::new(Lifecycle::new(&config.app)));
    let hold: Rc<RefCell<Option<gio::ApplicationHoldGuard>>> = Rc::default();

    // Everything is built at startup, before `activate`, `open` or an action (a notification
    // clicked at a cold start activates `app.open-chat` with no `activate`). Startup runs only in
    // the primary instance, so a second launch that just forwards to it skips the desktop queries.
    app.connect_startup({
        let shown = Rc::clone(&shown);
        let lifecycle = Rc::clone(&lifecycle);
        let hold = Rc::clone(&hold);
        move |app| {
            let mut controller = Controller::new(client_prefix());
            controller.set_config(&config);
            let ui = Ui::new(Some(app), controller, local_offset_minutes(), title_bar);
            ui.set_lifecycle(Lifecycle::new(&config.app));
            // The settings window (Ctrl+, or the command palette) edits this file.
            ui.set_config_path(Config::default_path());
            if lifecycle.borrow().holds() {
                // Keep running with the window hidden; released by Quit.
                *hold.borrow_mut() = Some(app.hold());
            }

            let look = desktop_look();
            let palette = Palette::load_or_default(Some(&palette_file));
            let display = gtk::gdk::Display::default().expect("a display");
            apply_look(&display, |current| *current = look.look());
            follow_animations(&display);
            apply_to_display(&display, &palette);
            follow_theme(&palette_file);
            if look.follows_font {
                follow_font(look.font_family.clone(), look.font_late);
            }
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

    // GTK must not see this program's own options; its `--gapplication-*` ones and the links
    // (as `open`) go on.
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "nuncio-desktop".to_owned());
    app.run_with_args(
        &std::iter::once(argv0)
            .chain(args.passed_on)
            .collect::<Vec<_>>(),
    )
}

/// Start the daemon worker on the socket `nunciod` listens on.
fn connect(ui: &Ui) {
    match socket_path() {
        Ok(path) => ui.start_worker(Settings {
            path,
            client: CLIENT_NAME.to_owned(),
            timeout: DEFAULT_READ_TIMEOUT,
            send_wait: SEND_WAIT,
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
            // `recv` blocks until a switch and ends when the watcher's thread has, so this
            // thread stops with it instead of spinning on a closed channel.
            std::thread::spawn(move || {
                while let Some(palette) = watcher.recv() {
                    context.invoke(move || {
                        if let Some(display) = gtk::gdk::Display::default() {
                            apply_to_display(&display, &palette);
                        }
                    });
                }
                eprintln!("nuncio-desktop: stopped following the theme");
            });
        }
        Err(err) => eprintln!("nuncio-desktop: not following the theme: {err}"),
    }
}

/// How long startup waits for fontconfig on the main loop. When it is slower (a cold cache at
/// login), the font watcher asks once more in the background with [`FONT_ON_SWITCH`].
const FONT_AT_START: Duration = Duration::from_millis(200);
/// How long a background query (a font switch, or that second start-up try) waits for
/// fontconfig, off the main loop.
const FONT_ON_SWITCH: Duration = Duration::from_secs(2);

/// The desktop's look at start: Omarchy's font (only where Omarchy is installed) and Hyprland's
/// rounding (only under Hyprland). Each query is bounded to ~200 ms; a failure is logged once and
/// leaves libadwaita's font or square corners, except a slow fontconfig, which is asked again in
/// the background.
struct StartLook {
    follows_font: bool,
    font_family: Option<String>,
    /// fontconfig did not answer within [`FONT_AT_START`]: ask again in the background.
    font_late: bool,
    corner_radius: Option<u32>,
}

impl StartLook {
    fn look(&self) -> Look {
        Look {
            font_family: self.font_family.clone(),
            corner_radius: self.corner_radius,
            ..Look::default()
        }
    }
}

/// Turn the app's transitions on or off with the desktop's `gtk-enable-animations` (GNOME's
/// `enable-animations` gsetting, which GTK reads), now and on each change (design plan §4.4).
fn follow_animations(display: &gtk::gdk::Display) {
    let settings = gtk::Settings::for_display(display);
    let on = settings.is_gtk_enable_animations();
    apply_look(display, |look| look.animations = on);
    let display = display.clone();
    settings.connect_gtk_enable_animations_notify(move |settings| {
        let on = settings.is_gtk_enable_animations();
        apply_look(&display, |look| look.animations = on);
    });
}

fn desktop_look() -> StartLook {
    let follows_font = omarchy_here(Palette::default_path().as_deref());
    let (font_family, font_late) = if follows_font {
        match font::current_font(&System, FONT_AT_START) {
            Ok(family) => (family, false),
            Err(error) if error.timed_out() => {
                eprintln!("nuncio-desktop: {error}; asking again in the background");
                (None, true)
            }
            Err(error) => {
                eprintln!("nuncio-desktop: using the default font: {error}");
                (None, false)
            }
        }
    } else {
        (None, false)
    };
    let corner_radius = hyprland::rounding(
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
        &System,
    )
    .unwrap_or_else(|error| {
        eprintln!("nuncio-desktop: using the default window corners: {error}");
        None
    });
    StartLook {
        follows_font,
        font_family,
        font_late,
        corner_radius,
    }
}

/// Follow live Omarchy font switches (`omarchy-font-set` rewrites `~/.config/fontconfig`), as
/// [`follow_theme`] follows the palette. `ask_now` asks fontconfig once more right away (the
/// start-up query timed out) and applies the font when it arrives.
fn follow_font(initial: Option<String>, ask_now: bool) {
    let Some(dir) = font::fontconfig_dir() else {
        return;
    };
    let query = || font::current_font(&System, FONT_ON_SWITCH).ok().flatten();
    match FontWatcher::start(&dir, initial, query) {
        Ok(watcher) => {
            if ask_now {
                watcher.ask_again();
            }
            let context = glib::MainContext::default();
            // `recv` blocks until a change and ends when the watcher's thread has, so this
            // thread stops with it instead of spinning on a closed channel.
            std::thread::spawn(move || {
                while let Some(family) = watcher.recv() {
                    context.invoke(move || {
                        if let Some(display) = gtk::gdk::Display::default() {
                            apply_look(&display, |look| look.font_family = family);
                        }
                    });
                }
                eprintln!("nuncio-desktop: stopped following the font");
            });
        }
        Err(err) => eprintln!("nuncio-desktop: not following the font: {err}"),
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
    println!(
        "generated css:    {} bytes",
        css_for(&light, &Look::default()).len()
    );

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
