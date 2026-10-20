//! The app's state and what each user action or daemon answer does to it, without GTK: the
//! shared [`Model`] holds every rule, the shared driver ([`nuncio_client::driver`]) folds the
//! worker's answers in, and what is left to do outside (send a frame to the worker, open a file)
//! comes back as [`Command`]s. The window calls these methods and draws
//! [`crate::present`]'s view of the model afterwards.

use std::path::PathBuf;

use nuncio_client::driver::{self, Outcome};
use nuncio_client::model::Effect;
use nuncio_client::{ClientFrame, Model, Session, Snapshot};

use crate::worker::Incoming;

/// How the desktop app names itself in the driver's notices.
const CLIENT: driver::Client = driver::Client {
    program: "nuncio-desktop",
    retry_hint: "choose Retry on the message once reconnected",
};

/// What the window must do after an action or an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Hand this frame to the worker.
    Send(ClientFrame),
    /// Open this downloaded media file with the desktop's opener.
    Open(PathBuf),
}

/// The model and what the window needs to drive it.
#[derive(Debug, Clone)]
pub struct Controller {
    model: Model,
    /// Whether the worker ever connected: the first connect says nothing, a later one says
    /// `reconnected to nunciod`.
    ever_connected: bool,
}

fn commands(effect: Option<Effect>) -> Vec<Command> {
    match effect {
        Some(Effect::Request(frame)) => vec![Command::Send(frame)],
        Some(Effect::Open(path)) => vec![Command::Open(path)],
        None => Vec::new(),
    }
}

fn send(frame: Option<ClientFrame>) -> Vec<Command> {
    frame.map(Command::Send).into_iter().collect()
}

impl Controller {
    /// Nothing loaded and not connected yet: the worker's first [`Incoming::Connected`] asks for
    /// the status and the chats. Sends get `client_id`s `<prefix>-1`, `<prefix>-2`, ...
    pub fn new(client_prefix: impl Into<String>) -> Self {
        let mut model = Model::new(
            Snapshot {
                daemon: String::new(),
                linked: false,
                session: Session::Unknown,
                unread: 0,
                chats: Vec::new(),
            },
            client_prefix,
        );
        model.set_connected(false);
        // Not seen until the window says it is active.
        model.set_seen(false);
        Self {
            model,
            ever_connected: false,
        }
    }

    /// A controller over an existing model (tests, the smoke test).
    pub fn with_model(model: Model) -> Self {
        Self {
            model,
            ever_connected: true,
        }
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Fold what the worker posted in.
    pub fn incoming(&mut self, incoming: Incoming) -> Vec<Command> {
        match incoming {
            Incoming::Connected => {
                let frames = if self.ever_connected {
                    driver::reconnected(&mut self.model)
                } else {
                    self.ever_connected = true;
                    self.model.set_connected(true);
                    self.model.resync()
                };
                frames.into_iter().map(Command::Send).collect()
            }
            Incoming::Reply { request, result } => {
                match driver::settle(&mut self.model, &request, result, CLIENT) {
                    Outcome::Done => Vec::new(),
                    Outcome::Effect(effect) => commands(Some(effect)),
                    Outcome::Lost(error) => {
                        driver::lost(&mut self.model, &error);
                        Vec::new()
                    }
                }
            }
            Incoming::NotConnected(frame) => {
                driver::not_connected(&mut self.model, &frame, CLIENT);
                Vec::new()
            }
            Incoming::Event(frame) => commands(self.model.apply(&frame)),
            Incoming::Lost(error) => {
                driver::lost(&mut self.model, &error);
                Vec::new()
            }
            // Events were dropped: ask again for what they would have said.
            Incoming::Overflow => {
                self.model
                    .set_notice("too many updates at once; reloading the chats");
                self.model.resync().into_iter().map(Command::Send).collect()
            }
        }
    }

    /// The user opened the chat at list position `index` (a click, or Enter on its row): it is
    /// selected, its conversation is in use, its messages are asked for, and once they are shown
    /// the model marks it read. Moving through the list without opening marks nothing read.
    pub fn open_chat(&mut self, index: usize) -> Vec<Command> {
        if index >= self.model.chat_list().chats().len() {
            return Vec::new();
        }
        let other = self.model.chat_list().highlighted() != Some(index);
        self.model.select_chat(index);
        if other {
            self.model.composer_mut().cancel_reply();
        }
        send(self.model.open_selected())
    }

    /// The window became active and shown (`true`), or inactive, hidden or minimized: only a
    /// seen window marks a new message in the open chat read.
    pub fn set_seen(&mut self, seen: bool) -> Vec<Command> {
        commands(self.model.set_seen(seen))
    }

    /// The user went back to the chat list (the narrow layout's back button).
    pub fn back_to_list(&mut self) {
        self.model.set_open(false);
    }

    /// The composer's text widget changed: the draft is now `text`.
    pub fn set_draft(&mut self, text: &str) {
        if self.model.composer().text() != text {
            self.model.composer_mut().set_text(text);
        }
    }

    /// Send the draft (see [`Model::send`]); `true` when it was sent, so the text widget empties.
    pub fn send(&mut self) -> (bool, Vec<Command>) {
        let frame = self.model.send();
        (frame.is_some(), send(frame))
    }

    pub fn cancel_reply(&mut self) {
        self.model.composer_mut().cancel_reply();
    }

    /// The loaded message at `index` has the focus.
    pub fn select_message(&mut self, index: usize) -> bool {
        self.model.select_message(index)
    }

    /// Reply to message `index`; false (the notice says why) when it cannot be replied to.
    pub fn reply(&mut self, index: usize) -> bool {
        self.model.select_message(index) && self.model.reply_to_selected()
    }

    /// React to message `index` with `emoji` (empty removes mine).
    pub fn react(&mut self, index: usize, emoji: &str) -> Vec<Command> {
        if !self.model.select_message(index) {
            return Vec::new();
        }
        send(self.model.react_selected(emoji))
    }

    /// One more attempt of my send `index` that ended not sent; never automatic.
    pub fn retry(&mut self, index: usize) -> Vec<Command> {
        if !self.model.select_message(index) {
            return Vec::new();
        }
        send(self.model.retry_selected())
    }

    /// Fetch media message `index`'s file to open it outside the app (no inline pictures here).
    pub fn open_media(&mut self, index: usize) -> Vec<Command> {
        if !self.model.select_message(index) {
            return Vec::new();
        }
        send(
            self.model
                .download_selected(false, true)
                .map(|download| download.frame()),
        )
    }

    /// Search the open chat for `query` (a blank one asks nothing).
    pub fn search(&mut self, query: &str) -> Vec<Command> {
        if !self.model.open_search() {
            return Vec::new();
        }
        query.chars().for_each(|c| self.model.search_type(c));
        send(self.model.submit_search())
    }

    /// Close the search and select its result `index` in the conversation; its position there,
    /// or `None` (the notice says it is older than the loaded messages).
    pub fn jump_to_result(&mut self, index: usize) -> Option<usize> {
        self.model.set_result_selected(Some(index));
        self.model
            .jump_to_result()
            .then(|| self.model.transcript().cursor())
            .flatten()
    }

    pub fn close_search(&mut self) {
        self.model.close_search();
    }

    /// A notice to show once (a toast), if a new one came.
    pub fn take_notice(&mut self) -> Option<String> {
        self.model.take_notice()
    }

    /// Opening a downloaded file failed.
    pub fn open_failed(&mut self, error: &std::io::Error) {
        self.model
            .set_notice(format!("cannot open the media: {error}"));
    }

    /// Whether the view lost its place (another chat, or a send): scroll to the newest.
    pub fn take_view_reset(&mut self) -> bool {
        self.model.take_view_reset()
    }
}

#[cfg(test)]
mod tests;
