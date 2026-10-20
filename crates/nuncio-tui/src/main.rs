//! `nuncio-tui` — the nuncio terminal client.
//!
//! Connects to `nunciod` over `$XDG_RUNTIME_DIR/nuncio/core.sock` (one request connection and
//! one subscribed event connection), renders the chat list and conversations, sends one message
//! at a time from the (multi-line) composer, and follows the daemon's events and the Omarchy theme. **If the
//! daemon is not reachable it still runs**, rendering `proto/fixtures/chats_response.json`
//! instead and saying so on screen; sends are then shown as not sent. A connection lost later (a
//! daemon restart) is retried on a capped backoff while the header says `reconnecting…`.
//!
//! It never links a WhatsApp device and never shows a QR code. The keys are in `README.md`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nuncio_client::{
    ClientFrame, DaemonFrame, PROTOCOL_VERSION, Palette, Snapshot, ThemeWatcher, fetch, socket_path,
};
use nuncio_tui::{Action, App, Daemon, Source, fixture_snapshot, open_externally};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// Announced in the `hello` frame, e.g. `nuncio-tui/0.1.0`.
const CLIENT_NAME: &str = concat!("nuncio-tui/", env!("CARGO_PKG_VERSION"));
/// How many chats to ask for (the wire contract allows 1..200).
const CHAT_LIMIT: u16 = 200;
/// How long one loop iteration waits for a key before it drains events and redraws.
const TICK: Duration = Duration::from_millis(100);

const USAGE: &str = "\
usage: nuncio-tui [--image <png>]

  Keys: j/k move or scroll, Enter open, i write, Enter send, Alt-Enter or Ctrl-J new line,
  Left/Right/Home/End move in the draft, Esc back, g/G oldest/newest, PgUp/PgDn page,
  K/J select a message, r reply, e react, R retry, o open media, / search, Tab pane,
  ? help, q quit.

  --image <png>   render a local PNG inline, next to the chat list (S9's ratatui-image
                  check; nothing in M0 serves images, so this points at any file)
  -h, --help      show this message
";

fn main() -> ExitCode {
    // Read before any thread starts: on Unix `time` only reads the local offset while the
    // process is single-threaded (the theme watcher and the opener start threads).
    let local_offset = time::UtcOffset::current_local_offset();
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
    let palette_path = Palette::default_path();
    let palette = Palette::load_or_default(palette_path.as_deref());
    let theme = palette_path
        .as_deref()
        .and_then(|path| ThemeWatcher::start(path).ok());
    let (snapshot, source) = load_snapshot();
    // The daemon answered `fetch`: if this connect fails anyway, the driver keeps retrying.
    let daemon = match source {
        Source::Daemon => socket_path().ok().map(|path| {
            Daemon::connect(&path, CLIENT_NAME).unwrap_or_else(|error| {
                eprintln!("nuncio-tui: no daemon connection yet ({error}); reconnecting");
                Daemon::disconnected(&path, CLIENT_NAME)
            })
        }),
        Source::Fixture => None,
    };

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
    let mut app = App::new(snapshot, source, palette)
        .with_client_prefix(client_prefix())
        .with_utc_offset(local_offset.unwrap_or(time::UtcOffset::UTC));
    if local_offset.is_err() {
        app.set_notice("the local time zone is unknown; times are UTC");
    }
    if let Some(image) = image {
        app = app.with_image(image);
    }
    if daemon.as_ref().is_some_and(|daemon| !daemon.is_connected()) {
        app.set_connected(false);
    }

    let mut io = Io { daemon, theme };
    let outcome = event_loop(&mut terminal, &mut app, &mut io);
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

/// What the loop talks to besides the terminal.
struct Io {
    daemon: Option<Daemon>,
    theme: Option<ThemeWatcher>,
}

impl Io {
    /// Carry out one action; `false` means quit.
    fn act(&mut self, app: &mut App, action: Action) -> bool {
        let frame = match action {
            Action::Quit => return false,
            Action::Request(frame) => frame,
            Action::Open(path) => {
                if let Err(error) = open_externally(&path) {
                    app.set_notice(format!("cannot open the media: {error}"));
                }
                return true;
            }
        };
        match self.daemon.as_mut() {
            // A lost connection is shown and reconnected by the driver itself.
            Some(daemon) => {
                let _ = daemon.perform(app, &frame);
            }
            // With no daemon (fixture data) a send cannot happen, and it says so.
            None => {
                if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } =
                    frame
                {
                    app.apply(&DaemonFrame::SendFailed {
                        v: PROTOCOL_VERSION,
                        client_id,
                        message: "no daemon".to_owned(),
                    });
                }
            }
        }
        true
    }

    /// Everything that happened since the last tick: daemon events and theme switches.
    fn tick(&mut self, app: &mut App) {
        // Also reconnects when the connection was lost and the backoff is due.
        if let Some(daemon) = self.daemon.as_mut() {
            let _ = daemon.drain_events(app);
        }
        if let Some(palette) = self
            .theme
            .as_ref()
            .and_then(|theme| theme.recv_timeout(Duration::ZERO))
        {
            app.set_palette(palette);
        }
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    io: &mut Io,
) -> std::io::Result<()> {
    loop {
        io.tick(app);
        terminal.draw(|frame| app.draw(frame))?;
        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let Some(action) = app.handle_key(key)
            && !io.act(app, action)
        {
            return Ok(());
        }
    }
}

/// A `client_id` prefix unique to this run, so a send never repeats an earlier run's id.
fn client_prefix() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    format!("tui-{}-{seconds}", std::process::id())
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
