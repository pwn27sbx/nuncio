//! The `nuncio-desktop` binary.
//!
//! Two modes:
//!
//! - normal: open the window and follow the live Omarchy theme;
//! - `--probe`: the S5 acceptance check without a window on screen. It applies a palette, performs
//!   the same atomic theme swap `omarchy-theme-set` does, and prints the colour GTK resolves
//!   before and after plus how long the detection took. `--theme-dir` points it at a fixture tree
//!   so the check never touches the running theme.

use gtk::prelude::*;
use nuncio_client::{Palette, ThemeWatcher};
use nuncio_desktop::{ROOT_CLASS, apply_to_display, css_for, resolved_foreground};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const APP_ID: &str = "io.github.pwn27sbx.nuncio";

struct Args {
    theme_dir: Option<PathBuf>,
    probe: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        theme_dir: None,
        probe: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--probe" => args.probe = true,
            "--theme-dir" => args.theme_dir = argv.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!(
                    "usage: nuncio-desktop [--theme-dir DIR] [--probe]\n\n  \
                     --theme-dir DIR  directory holding colors.toml (default: Omarchy's)\n  \
                     --probe          run the S5 theme-switch check and exit"
                );
                std::process::exit(0);
            }
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

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("nuncio")
            .default_width(860)
            .default_height(560)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class(ROOT_CLASS);
        root.set_margin_top(12);
        root.set_margin_bottom(12);
        root.set_margin_start(12);
        root.set_margin_end(12);
        let heading = gtk::Label::new(Some("nuncio"));
        heading.add_css_class("nuncio-accent");
        let note = gtk::Label::new(Some("M0 spike: the window follows the live Omarchy theme."));
        note.add_css_class("nuncio-muted");
        root.append(&heading);
        root.append(&note);
        window.set_child(Some(&root));

        let palette = Palette::load_or_default(Some(&palette_file));
        apply_to_display(&gtk::gdk::Display::default().expect("a display"), &palette);

        // Follow live theme switches: the watcher owns its own thread, so hand events back to the
        // main loop before touching any widget.
        match ThemeWatcher::start(&palette_file) {
            Ok(watcher) => {
                let context = glib::MainContext::default();
                std::thread::spawn(move || {
                    while let Some(palette) = watcher.recv_timeout(Duration::from_secs(1)) {
                        let context = context.clone();
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

        window.present();
    });

    app.run()
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
