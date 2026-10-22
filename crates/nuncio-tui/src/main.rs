//! `nuncio-tui` — the nuncio terminal client.
//!
//! Connects to `nunciod` over `$XDG_RUNTIME_DIR/nuncio/core.sock` (one request connection, one
//! subscribed event connection, and one for sends, whose answers are awaited off the loop),
//! renders the chat list and conversations, sends one message at a time from the (multi-line)
//! composer, and follows the daemon's events and the Omarchy theme. **If the
//! daemon is not reachable it still runs**, rendering `proto/fixtures/chats_response.json`
//! instead and saying so on screen; sends are then shown as not sent. A connection lost later (a
//! daemon restart) is retried on a capped backoff while the header says `reconnecting…`.
//!
//! It never links a WhatsApp device and never shows a QR code. The keys are in `README.md`.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nuncio_client::{
    ClientFrame, Config, DaemonFrame, ImageProtocol, PROTOCOL_VERSION, Palette, Snapshot,
    ThemeWatcher, fetch, socket_path,
};
use nuncio_tui::images::{self, Graphics, Plan, ProtocolChoice};
use nuncio_tui::{Action, App, Daemon, Source, fixture_snapshot, open_externally};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_image::FontSize;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

/// Announced in the `hello` frame, e.g. `nuncio-tui/0.1.0`.
const CLIENT_NAME: &str = concat!("nuncio-tui/", env!("CARGO_PKG_VERSION"));
/// How many chats to ask for (the wire contract allows 1..200).
const CHAT_LIMIT: u16 = 200;
/// How long one loop iteration waits for a key before it drains events and redraws.
const TICK: Duration = Duration::from_millis(100);
/// How long the terminal has to answer the picture query.
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a late answer to the query may take before the TUI asks for a key instead.
const LATE_ANSWER: Duration = Duration::from_millis(500);
/// How long the rest of a late answer may take to arrive before it is thrown away.
const LATE_TAIL: Duration = Duration::from_millis(100);

const USAGE: &str = "\
usage: nuncio-tui [--image <png>]

  Keys: Tab switches between the chat list and the conversation; j/k or the arrows move in
  the focused pane (chats, or the selected message); Enter opens a chat ready to write,
  Enter sends, Alt-Enter or Ctrl-J new line, Esc composer -> conversation -> chats;
  r reply, e react, R retry, o show a picture in full (other media, and every media with
  pictures off, open externally), O open media externally, / search, ? help, q quit
  (Ctrl-C anywhere).
  Mouse: click a chat to open it, click a message to select it, the wheel scrolls the pane
  under the pointer; hold Shift to select text.

  Pictures are drawn in the conversation from the thumbnail in each message; nothing is
  downloaded until o. The protocol is asked from the terminal once at startup (kitty, sixel,
  iTerm2, else half blocks; sixel or half blocks under zellij). NUNCIO_IMAGE_PROTOCOL=kitty,
  sixel, iterm2, halfblocks or off chooses it without asking, and so does image_protocol under
  [ui] in ~/.config/nuncio/config.toml (the variable wins; see docs/config.md).

  --image <png>   render a local PNG inline, next to the chat list (S9's ratatui-image
                  check; this points at any file)
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
    let config = match Config::default_path().map(|path| Config::load(&path)) {
        Some(Ok(config)) => config,
        None => Config::default(),
        Some(Err(error)) => {
            eprintln!("nuncio-tui: {error}");
            return ExitCode::from(2);
        }
    };
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
    // The picture protocol is asked from the terminal once, after entering the alternate screen
    // and before mouse reports are turned on or any event is read.
    let (graphics, picture_notice) = choose_graphics(&mut terminal, config.ui.image_protocol);

    // Clicks and the wheel. A panic turns it off before ratatui's own hook restores the rest,
    // so the shell is not left receiving mouse reports.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        hook(info);
    }));

    let (image, image_notice) = match args.image.as_deref() {
        Some(path) => image_for(
            path,
            graphics
                .as_ref()
                .map_or(&Picker::halfblocks(), Graphics::picker),
        ),
        None => (None, None),
    };
    let mut app = App::new(snapshot, source, palette)
        .with_client_prefix(client_prefix())
        .with_utc_offset(local_offset.unwrap_or(time::UtcOffset::UTC));
    if let Some(graphics) = graphics {
        app = app.with_graphics(graphics);
    }
    app.set_notice(picture_notice);
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
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
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
        let action = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            _ => None,
        };
        // A download waits for the daemon's answer: show `Downloading…` before it does.
        if let Some(Action::Request(ClientFrame::DownloadMedia { .. })) = &action {
            terminal.draw(|frame| app.draw(frame))?;
        }
        if let Some(action) = action
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

/// Choose how pictures are drawn, once: [`images::plan_with_config`] from `NUNCIO_IMAGE_PROTOCOL`,
/// the config file's `[ui] image_protocol`, `ZELLIJ`
/// and whether stdin and stdout are a terminal, then either that choice or ratatui-image's query.
/// Returns the protocol (`None` for `off`) and the footer's one-time line naming it.
fn choose_graphics(
    terminal: &mut ratatui::DefaultTerminal,
    configured: Option<ImageProtocol>,
) -> (Option<Graphics>, String) {
    let value = std::env::var(images::OVERRIDE).ok();
    let zellij = std::env::var_os("ZELLIJ").is_some();
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let (plan, warning) = images::plan_with_config(value.as_deref(), configured, zellij, tty);
    let (graphics, notice) = match plan {
        Plan::Fixed { choice, why } => fixed_graphics(choice, why),
        Plan::Query { zellij } => queried_graphics(terminal, zellij),
    };
    match warning {
        Some(warning) => (graphics, format!("{warning}; {notice}")),
        None => (graphics, notice),
    }
}

/// The protocol named in `NUNCIO_IMAGE_PROTOCOL` (or half blocks when the terminal cannot be
/// asked), with the cell size from the terminal's window size, without querying anything.
fn fixed_graphics(choice: ProtocolChoice, why: &str) -> (Option<Graphics>, String) {
    let protocol = match choice {
        ProtocolChoice::Off => return (None, format!("pictures: off ({why})")),
        ProtocolChoice::Kitty => ProtocolType::Kitty,
        ProtocolChoice::Sixel => ProtocolType::Sixel,
        ProtocolChoice::Iterm2 => ProtocolType::Iterm2,
        ProtocolChoice::Halfblocks => ProtocolType::Halfblocks,
    };
    let picker = if protocol == ProtocolType::Halfblocks {
        Picker::halfblocks()
    } else {
        // ratatui-image 11 builds a picker from a cell size only through this deprecated call.
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(cell_size());
        picker.set_protocol_type(protocol);
        picker
    };
    let graphics = Graphics::new(picker, why);
    let notice = graphics.notice();
    (Some(graphics), notice)
}

/// The terminal's cell size in pixels from its window size, or 10x20 when it reports none.
fn cell_size() -> FontSize {
    match ratatui::crossterm::terminal::window_size() {
        Ok(size) if size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0 => {
            FontSize::new(size.width / size.columns, size.height / size.rows)
        }
        _ => FontSize::new(10, 20),
    }
}

/// Ask the terminal which protocol it speaks. ratatui-image 11.1 reads the answer on a thread it
/// never stops when the answer does not come in time (issue #202); that thread would take the
/// next input. So when the query ran out its timeout without an answer, the TUI uses half blocks
/// and makes stdin safe before any event is read ([`guard_stdin`]).
fn queried_graphics(
    terminal: &mut ratatui::DefaultTerminal,
    zellij: bool,
) -> (Option<Graphics>, String) {
    let started = Instant::now();
    let result = Picker::from_query_stdio_with_options(QueryStdioOptions {
        timeout: QUERY_TIMEOUT,
        ..QueryStdioOptions::default()
    });
    let answered = result
        .as_ref()
        .is_ok_and(|picker| !picker.capabilities().is_empty());
    if images::query_left_stdin_unsafe(started.elapsed(), QUERY_TIMEOUT, answered) {
        guard_stdin(terminal);
        let graphics = Graphics::new(
            Picker::halfblocks(),
            format!(
                "the terminal did not answer; {} chooses another",
                images::OVERRIDE
            ),
        );
        let notice = graphics.notice();
        return (Some(graphics), notice);
    }
    let Ok(mut picker) = result else {
        let graphics = Graphics::new(Picker::halfblocks(), "the terminal query failed");
        let notice = graphics.notice();
        return (Some(graphics), notice);
    };
    let how = if zellij {
        let sixel = picker.capabilities().contains(&Capability::Sixel);
        picker.set_protocol_type(images::zellij_protocol(picker.protocol_type(), sixel));
        "zellij"
    } else {
        "detected"
    };
    let graphics = Graphics::new(picker, how);
    let notice = graphics.notice();
    (Some(graphics), notice)
}

/// Make stdin safe after a query that timed out: ask the terminal for a status report once more
/// (its answer ends the read ratatui-image's thread is blocked in, and that thread then stops),
/// and take Rust's stdin lock for good, so that thread can never read again. The event loop starts
/// only once the lock is taken; when no answer comes, the screen asks for a key, which that last
/// read takes. Whatever is left of the answer is then thrown away.
fn guard_stdin(terminal: &mut ratatui::DefaultTerminal) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b[5n");
    let _ = stdout.flush();
    let taken = images::hold_stdin();
    if taken.recv_timeout(LATE_ANSWER).is_ok() {
        drop_late_answer();
        return;
    }
    let text = format!(
        "The terminal did not answer nuncio's picture query.\n\nPress any key to continue \
         (set {}=halfblocks, or another protocol, to skip the query).",
        images::OVERRIDE
    );
    let _ = terminal.draw(|frame| {
        frame.render_widget(
            Paragraph::new(text.as_str()).wrap(Wrap { trim: false }),
            frame.area(),
        );
    });
    let _ = taken.recv();
    drop_late_answer();
}

/// What is left of the terminal's late answer (its tail can arrive after the read that took its
/// head) is thrown away before the first key is read.
fn drop_late_answer() {
    std::thread::sleep(LATE_TAIL);
    images::discard_pending_input();
}

/// Load `path` and prepare it for the terminal the client is running in, with `picker`.
///
/// Returns the image state plus a line to print once the terminal is given back.
fn image_for(path: &Path, picker: &Picker) -> (Option<StatefulProtocol>, Option<String>) {
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
