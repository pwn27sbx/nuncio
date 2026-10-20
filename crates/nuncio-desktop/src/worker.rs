//! The daemon connections, off the GTK main loop.
//!
//! Two threads per [`Worker`]: the request thread owns the request connection and the reconnect
//! schedule ([`Reconnect`]), and sends each request it is handed, one at a time, in order; an
//! event thread per connection life reads the subscribed connection. Neither touches the view:
//! everything they learn is posted as an [`Incoming`] (through the `post` callback, which the app
//! points at a channel into the main loop) and folded into the model there
//! ([`crate::controller::Controller::incoming`]), with the shared driver's rules.
//!
//! A loss on either connection drops both and schedules a reconnect on the shared capped backoff
//! (0.5 s doubling to 5 s); a request made meanwhile is answered [`Incoming::NotConnected`] and
//! never sent later by itself. A reconnect posts [`Incoming::Connected`], after which the main loop
//! resyncs.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use nuncio_client::driver::{self, Link};
use nuncio_client::{Backoff, ClientError, ClientFrame, Connection, DaemonFrame, Reconnect};

/// How long the event thread waits for an event before it checks whether its life ended.
const EVENT_WAIT: Duration = Duration::from_millis(250);

/// What the worker tells the main loop.
#[derive(Debug)]
pub enum Incoming {
    /// Both connections are up (the first time, or again after a loss).
    Connected,
    /// The answer to `request` (or why there is none). A loss in it already dropped the link.
    Reply {
        request: ClientFrame,
        result: Result<DaemonFrame, ClientError>,
    },
    /// `request` was not sent: the worker is reconnecting.
    NotConnected(ClientFrame),
    /// An event from the subscribed connection.
    Event(DaemonFrame),
    /// The event connection broke; both were dropped and the worker reconnects.
    Lost(ClientError),
    /// The main loop fell [`EVENT_BACKLOG`] events behind: later events were dropped until it
    /// caught up, so what they said must be asked again (a resync).
    Overflow,
}

/// How many events may wait for the main loop before more are dropped.
pub const EVENT_BACKLOG: usize = 1024;

/// Keeps the events waiting for the main loop bounded, so a stalled main loop under an event storm
/// cannot grow memory without end. Only events are counted and dropped: a reply answers a request
/// the main loop made (one each), and dropping one would leave a send "sending" for good.
#[derive(Debug)]
pub struct Backlog {
    limit: usize,
    waiting: AtomicUsize,
    /// Events were dropped and the `Overflow` saying so was posted, not yet seen.
    overflowed: AtomicBool,
}

impl Backlog {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            waiting: AtomicUsize::new(0),
            overflowed: AtomicBool::new(false),
        }
    }

    /// The worker's side: what to post for `incoming`. An event over the limit is dropped, and the
    /// first one dropped becomes one [`Incoming::Overflow`].
    pub fn admit(&self, incoming: Incoming) -> Option<Incoming> {
        if !matches!(incoming, Incoming::Event(_)) {
            return Some(incoming);
        }
        if self.waiting.load(Ordering::Acquire) < self.limit {
            self.waiting.fetch_add(1, Ordering::AcqRel);
            return Some(incoming);
        }
        (!self.overflowed.swap(true, Ordering::AcqRel)).then_some(Incoming::Overflow)
    }

    /// The main loop's side: `incoming` was taken off the channel.
    pub fn delivered(&self, incoming: &Incoming) {
        match incoming {
            Incoming::Event(_) => {
                self.waiting.fetch_sub(1, Ordering::AcqRel);
            }
            Incoming::Overflow => self.overflowed.store(false, Ordering::Release),
            _ => {}
        }
    }
}

/// What the request thread is handed.
enum Command {
    Request(ClientFrame),
    /// The event thread of life `life` lost its connection with this error.
    EventsLost(u64, ClientError),
    /// The handle was dropped: close both connections and end.
    Stop,
}

/// The handle the main loop keeps. Dropping it stops both threads: the request connection closes
/// at once, the event connection within [`EVENT_WAIT`], and nothing reconnects.
pub struct Worker {
    commands: mpsc::Sender<Command>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The request thread holds a sender of its own (for its event threads), so it never sees
        // the channel close: it is told.
        let _ = self.commands.send(Command::Stop);
    }
}

/// Where and how to connect.
#[derive(Debug, Clone)]
pub struct Settings {
    pub path: PathBuf,
    /// The name sent in `hello`.
    pub client: String,
    /// The bound on every read.
    pub timeout: Duration,
    pub backoff: Backoff,
}

impl Worker {
    /// Start the threads: the first connect is tried at once.
    pub fn spawn(settings: Settings, post: impl Fn(Incoming) + Send + Clone + 'static) -> Self {
        let (commands, inbox) = mpsc::channel();
        let back = commands.clone();
        thread::Builder::new()
            .name("nuncio-requests".to_owned())
            .spawn(move || run(settings, inbox, back, post))
            .expect("spawn the request thread");
        Self { commands }
    }

    /// Send `frame` (in order with the others); its answer comes back as [`Incoming::Reply`] or
    /// [`Incoming::NotConnected`].
    pub fn send(&self, frame: ClientFrame) {
        // The thread only ends when this handle is dropped.
        let _ = self.commands.send(Command::Request(frame));
    }
}

/// One connection life: the request connection, and the flag that ends its event thread.
struct Life {
    id: u64,
    requests: Connection,
    over: Arc<AtomicBool>,
}

impl Drop for Life {
    fn drop(&mut self) {
        self.over.store(true, Ordering::Relaxed);
    }
}

fn run(
    settings: Settings,
    inbox: mpsc::Receiver<Command>,
    back: mpsc::Sender<Command>,
    post: impl Fn(Incoming) + Send + Clone + 'static,
) {
    let mut reconnect = Reconnect::new(settings.backoff.clone());
    let mut lives = 0_u64;
    let mut life = connect(&settings, &mut lives, &back, &post);
    if life.is_none() {
        reconnect.lost(Instant::now());
    }
    loop {
        let command = match &life {
            Some(_) => match inbox.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            },
            None => {
                let wait = reconnect.wait(Instant::now()).unwrap_or(Duration::ZERO);
                match inbox.recv_timeout(wait) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        };
        match command {
            Some(Command::Request(frame)) => match life.as_mut() {
                Some(current) => {
                    let result = driver::request_on(&mut current.requests, &frame);
                    if let Err(error) = &result
                        && driver::is_loss(error)
                    {
                        life = None;
                        reconnect.lost(Instant::now());
                    }
                    post(Incoming::Reply {
                        request: frame,
                        result,
                    });
                }
                None => post(Incoming::NotConnected(frame)),
            },
            // Only the current life's loss counts: an older one was already dropped.
            Some(Command::EventsLost(id, error))
                if life.as_ref().is_some_and(|current| current.id == id) =>
            {
                life = None;
                reconnect.lost(Instant::now());
                post(Incoming::Lost(error));
            }
            // Dropping the life closes the request connection and ends its event thread.
            Some(Command::Stop) => return,
            Some(Command::EventsLost(..)) | None => {}
        }
        if life.is_none() && reconnect.due(Instant::now()) {
            life = connect(&settings, &mut lives, &back, &post);
            match life {
                Some(_) => reconnect.succeeded(),
                None => reconnect.failed(Instant::now()),
            }
        }
    }
}

/// Open both connections and start this life's event thread; `None` when the daemon did not
/// answer.
fn connect(
    settings: &Settings,
    lives: &mut u64,
    back: &mpsc::Sender<Command>,
    post: &(impl Fn(Incoming) + Send + Clone + 'static),
) -> Option<Life> {
    let Link { requests, events } =
        Link::open(&settings.path, &settings.client, settings.timeout).ok()?;
    *lives += 1;
    let id = *lives;
    let over = Arc::new(AtomicBool::new(false));
    let (flag, back, events_post) = (Arc::clone(&over), back.clone(), post.clone());
    thread::Builder::new()
        .name("nuncio-events".to_owned())
        .spawn(move || read_events(events, id, &flag, &back, &events_post))
        .ok()?;
    post(Incoming::Connected);
    Some(Life { id, requests, over })
}

/// Read events until the connection breaks or its life is over.
fn read_events(
    mut events: Connection,
    id: u64,
    over: &AtomicBool,
    back: &mpsc::Sender<Command>,
    post: &impl Fn(Incoming),
) {
    while !over.load(Ordering::Relaxed) {
        match events.next_event(EVENT_WAIT) {
            Ok(Some(frame)) if !over.load(Ordering::Relaxed) => post(Incoming::Event(frame)),
            Ok(_) => {}
            Err(error) => {
                let _ = back.send(Command::EventsLost(id, error));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
