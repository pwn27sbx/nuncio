//! Sends and retries, off the caller's loop.
//!
//! A `send` (or `retry`) reply can take up to nunciod's own send timeout (20 s) and is awaited up
//! to [`SEND_WAIT`], far longer than an ordinary read. So a client never waits for it on its UI
//! loop or on its request connection: it hands the frame to an [`Outbox`], whose thread sends it
//! on a connection of its own (opened at the first send, with `hello`) and posts the answer back
//! ([`Posted`]). Meanwhile the message shows as sending (the model's sending mark) and every other
//! request goes on as before.
//!
//! Frames go out one at a time, in the order pushed, each exactly once. A reply that does not come
//! within the wait is [`ClientError::SendUnanswered`] (not confirmed, F3), and like any other loss
//! it ends the outbox's connection: every frame still waiting is posted [`Posted::NotConnected`],
//! never sent later by itself. Dropping the outbox (the client lost its daemon link) does the
//! same to the frames still waiting, while the one already written is still answered: its reply
//! is applied by its `client_id`, so it can only ever change its own message.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::conn::{ClientError, Connection};
use crate::driver::{self, SEND_WAIT};
use crate::wire::{ClientFrame, DaemonFrame};

/// What the outbox thread posts back, once per frame pushed.
// One value per send, moved once through a channel: boxing the reply would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Posted {
    /// The answer to `request` (or why there is none, a loss included).
    Reply {
        request: ClientFrame,
        result: Result<DaemonFrame, ClientError>,
    },
    /// `request` was never written: the outbox was dropped, or its connection was lost, before
    /// its turn.
    NotConnected(ClientFrame),
}

/// Where the outbox connects, and how long it waits.
#[derive(Debug, Clone)]
pub struct Settings {
    pub path: PathBuf,
    /// The name sent in `hello`.
    pub client: String,
    /// The bound on every other read (the handshake).
    pub timeout: Duration,
    /// How long one send or retry reply is awaited ([`SEND_WAIT`]; tests shorten it).
    pub send_wait: Duration,
}

impl Settings {
    pub fn new(path: &Path, client: &str, timeout: Duration) -> Self {
        Self {
            path: path.to_owned(),
            client: client.to_owned(),
            timeout,
            send_wait: SEND_WAIT,
        }
    }
}

/// The handle to one outbox thread. Dropping it ends the thread once the frame in flight is
/// answered; the frames still waiting are posted [`Posted::NotConnected`].
pub struct Outbox {
    frames: mpsc::Sender<ClientFrame>,
    over: Arc<AtomicBool>,
}

impl Drop for Outbox {
    fn drop(&mut self) {
        self.over.store(true, Ordering::Release);
    }
}

impl Outbox {
    /// Start the thread; nothing connects until the first [`Outbox::push`].
    pub fn start(settings: Settings, mut post: impl FnMut(Posted) + Send + 'static) -> Self {
        let (frames, inbox) = mpsc::channel::<ClientFrame>();
        let over = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&over);
        thread::Builder::new()
            .name("nuncio-outbox".to_owned())
            .spawn(move || run(&settings, &inbox, &flag, &mut post))
            .expect("spawn the outbox thread");
        Self { frames, over }
    }

    /// Send `frame` after every frame pushed before it; its answer is posted.
    pub fn push(&self, frame: ClientFrame) {
        // The thread only ends once this handle is dropped.
        let _ = self.frames.send(frame);
    }
}

fn run(
    settings: &Settings,
    inbox: &mpsc::Receiver<ClientFrame>,
    over: &AtomicBool,
    post: &mut impl FnMut(Posted),
) {
    let mut connection: Option<Connection> = None;
    let mut broken = false;
    while let Ok(frame) = inbox.recv() {
        if broken || over.load(Ordering::Acquire) {
            post(Posted::NotConnected(frame));
            continue;
        }
        let requests = match connection.as_mut() {
            Some(requests) => requests,
            None => match open(settings) {
                Ok(opened) => connection.insert(opened),
                Err(error) => {
                    broken = true;
                    post(Posted::Reply {
                        request: frame,
                        result: Err(error),
                    });
                    continue;
                }
            },
        };
        let result = requests.request_within(&frame, settings.send_wait);
        if let Err(error) = &result
            && driver::is_loss(error)
        {
            broken = true;
            connection = None;
        }
        post(Posted::Reply {
            request: frame,
            result,
        });
    }
}

fn open(settings: &Settings) -> Result<Connection, ClientError> {
    let mut connection = Connection::connect_with_timeout(&settings.path, settings.timeout)?;
    connection.handshake(&settings.client)?;
    Ok(connection)
}

#[cfg(test)]
mod tests;
