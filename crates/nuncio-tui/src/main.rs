//! `nuncio-tui` — the nuncio terminal client.
//!
//! Connects to `nunciod` over `$XDG_RUNTIME_DIR/nuncio/core.sock`, does the NDJSON
//! handshake, and renders the chat list. **If the daemon is not reachable it still runs**,
//! rendering `proto/fixtures/chats_response.json` instead and saying so on screen.
//!
//! It never links a WhatsApp device and never shows a QR code: M0's daemon is unlinked.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nuncio_client::{Palette, Snapshot, fetch, socket_path};
use nuncio_tui::{App, Source, fixture_snapshot};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// Announced in the `hello` frame, e.g. `nuncio-tui/0.1.0`.
const CLIENT_NAME: &str = concat!("nuncio-tui/", env!("CARGO_PKG_VERSION"));
/// How many chats to ask for (the wire contract allows 1..200).
const CHAT_LIMIT: u16 = 40;

const USAGE: &str = "\
usage: nuncio-tui [--image <png>]

  --image <png>   render a local PNG inline, next to the chat list (S9's ratatui-image
                  check; nothing in M0 serves images, so this points at any file)
  -h, --help      show this message
";

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("nuncio-tui: {message}");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // Printed before the terminal is taken over, so it is still readable.
    let palette = Palette::load_or_default(Palette::default_path().as_deref());
    let (snapshot, source) = load_snapshot();

    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("nuncio-tui: cannot use this terminal: {error}");
            return ExitCode::FAILURE;
        }
    };

    // The ratatui-image picker queries the terminal, so it must run after entering the
    // alternate screen and before reading events.
    let (image, image_notice) = match args.image.as_deref() {
        Some(path) => image_for(path),
        None => (None, None),
    };
    let mut app = App::new(snapshot, source, palette);
    if let Some(image) = image {
        app = app.with_image(image);
    }

    let outcome = event_loop(&mut terminal, &mut app);
    ratatui::restore();

    // Printed after the terminal is given back, so it does not corrupt the display.
    if let Some(notice) = image_notice {
        eprintln!("{notice}");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nuncio-tui: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The daemon's view, or the compiled-in fixture when the daemon is unreachable.
fn load_snapshot() -> (Snapshot, Source) {
    match socket_path().and_then(|path| fetch(&path, CLIENT_NAME, CHAT_LIMIT)) {
        Ok(snapshot) => (snapshot, Source::Daemon),
        Err(error) => {
            eprintln!("nuncio-tui: no daemon ({error}); showing fixture data");
            (fixture_snapshot(), Source::Fixture)
        }
    }
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
            KeyCode::Tab => app.toggle_pane(),
            _ => {}
        }
    }
}

/// Load `path` and prepare it for the terminal the client is running in.
///
/// Returns the image state plus a line to print once the terminal is given back.
fn image_for(path: &Path) -> (Option<StatefulProtocol>, Option<String>) {
    let image = match image::open(path) {
        Ok(image) => image,
        Err(error) => {
            return (
                None,
                Some(format!(
                    "nuncio-tui: cannot read {}: {error}",
                    path.display()
                )),
            );
        }
    };

    let picker = match Picker::from_query_stdio() {
        Ok(picker) => picker,
        Err(error) => {
            let picker = Picker::halfblocks();
            let notice = format!(
                "nuncio-tui: this terminal did not answer the image query ({error}); \
                 drawing {} with half blocks instead of the {:?} protocol",
                path.display(),
                picker.protocol_type()
            );
            return (Some(picker.new_resize_protocol(image)), Some(notice));
        }
    };

    let notice = format!(
        "nuncio-tui: drawing {} with the {:?} protocol",
        path.display(),
        picker.protocol_type()
    );
    (Some(picker.new_resize_protocol(image)), Some(notice))
}

/// The command line.
#[derive(Debug)]
struct Args {
    image: Option<PathBuf>,
}

impl Args {
    /// `Ok(None)` means help was asked for.
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut image = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "--image" => {
                    let path = args
                        .next()
                        .ok_or_else(|| "--image needs a PNG path".to_owned())?;
                    image = Some(PathBuf::from(path));
                }
                other => return Err(format!("unexpected argument {other:?}")),
            }
        }
        Ok(Some(Self { image }))
    }
}
