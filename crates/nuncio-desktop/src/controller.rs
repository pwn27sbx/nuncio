//! The app's state and what each user action or daemon answer does to it, without GTK: the
//! shared [`Model`] holds every rule, the shared driver ([`nuncio_client::driver`]) folds the
//! worker's answers in, and what is left to do outside (send a frame to the worker, open a file)
//! comes back as [`Command`]s. The window calls these methods and draws
//! [`crate::present`]'s view of the model afterwards.

use std::collections::HashSet;
use std::path::PathBuf;

use nuncio_client::driver::{self, Outcome};
use nuncio_client::link::{LinkError, parse_link};
use nuncio_client::model::Effect;
use nuncio_client::notify::{Notification, ViewState, notification_for};
use nuncio_client::{ClientFrame, Config, DaemonFrame, Model, Session, Snapshot};

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

/// The application action a notification's `default` action names, with the chat JID as its
/// string target: it shows the window and opens that chat.
pub const OPEN_CHAT_ACTION: &str = "app.open-chat";

/// The id a chat's notification is shown under: one per chat, so a newer message replaces the
/// notification of an older one in the same chat, and opening the chat withdraws it.
pub fn notification_id(jid: &str) -> String {
    format!("chat:{jid}")
}

/// A desktop notification to show or take back (the window hands it to the `GApplication`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alert {
    /// Show this notification under its chat's [`notification_id`].
    Show(Notification),
    /// Withdraw the notification of this chat: it was opened or read.
    Withdraw(String),
}

/// What "New message" says when the number typed cannot open a chat: the link's own rule
/// ([`LinkError::BadPhone`]), said of a typed number. It never repeats the number.
pub const BAD_NUMBER: &str = "the phone number is not 7 to 15 digits";

/// The `whatsapp://send?phone=<digits>` link, with no text, that a phone number typed into "New
/// message" stands for. The number is judged by the link's own rules ([`parse_link`]): every
/// character typed goes into the link percent-encoded, so `+`, spaces, dashes, dots and
/// parentheses are ignored as a link's are, 7 to 15 digits must remain, and nothing typed can
/// add a `text` or anything else to the link.
pub fn number_link(written: &str) -> Result<String, &'static str> {
    let encoded: String = written
        .trim()
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    let target = parse_link(&format!("whatsapp://send?phone={encoded}")).map_err(|_| BAD_NUMBER)?;
    let digits = target
        .jid
        .strip_suffix("@s.whatsapp.net")
        .ok_or(BAD_NUMBER)?;
    Ok(format!("whatsapp://send?phone={digits}"))
}

/// The model and what the window needs to drive it.
#[derive(Debug, Clone)]
pub struct Controller {
    model: Model,
    /// Whether the worker ever connected: the first connect says nothing, a later one says
    /// `reconnected to nunciod`.
    ever_connected: bool,
    config: Config,
    /// What the window shows, for [`notification_for`], and the messages already notified.
    view: ViewState,
    /// The chats with a notification shown and not withdrawn yet.
    notifying: HashSet<String>,
    alerts: Vec<Alert>,
    /// Whether a chat list has arrived: a chat named before that (a notification clicked or a
    /// link opened at a cold start) is opened once it has.
    chats_loaded: bool,
    /// The chat (and the link's draft) waiting for the first chat list.
    waiting: Option<(String, Option<String>)>,
    /// The draft was set here (a link), not typed: the composer widget must show it.
    draft_changed: bool,
    /// Pictures whose full file was asked for again ([`Controller::open_media`]), by chat and
    /// id: one that could not be shown may be decoded once more.
    picture_retries: Vec<(String, String)>,
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
                stop_reason: None,
            },
            client_prefix,
        );
        model.set_connected(false);
        // Not seen until the window says it is active.
        model.set_seen(false);
        Self::build(model, false, false)
    }

    /// A controller over an existing model (tests, the smoke test).
    pub fn with_model(model: Model) -> Self {
        Self::build(model, true, true)
    }

    fn build(model: Model, ever_connected: bool, chats_loaded: bool) -> Self {
        Self {
            model,
            ever_connected,
            config: Config::default(),
            view: ViewState::default(),
            notifying: HashSet::new(),
            alerts: Vec::new(),
            chats_loaded,
            waiting: None,
            draft_changed: false,
            picture_retries: Vec::new(),
        }
    }

    /// Follow `config` (which messages notify and what their notifications say).
    pub fn set_config(&mut self, config: &Config) {
        self.config = config.clone();
    }

    /// The configuration followed now.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Follow a configuration changed while running (the settings window): which messages notify
    /// and what their notifications say change at once. Returns the new `[app] title_bar` choice
    /// when it changed, for the window to redraw its header bars.
    pub fn reconfigure(&mut self, config: &Config) -> Option<nuncio_client::TitleBar> {
        let title_bar =
            (config.app.title_bar != self.config.app.title_bar).then_some(config.app.title_bar);
        self.config = config.clone();
        title_bar
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
                let listed = matches!(result, Ok(DaemonFrame::Chats { .. }));
                if let Ok(DaemonFrame::MarkedRead { jid, .. }) = &result {
                    self.withdraw(jid);
                }
                let mut commands = match driver::settle(&mut self.model, &request, result, CLIENT) {
                    Outcome::Done => Vec::new(),
                    Outcome::Effect(effect) => commands(Some(effect)),
                    Outcome::Lost(error) => {
                        driver::lost(&mut self.model, &error);
                        Vec::new()
                    }
                };
                if listed {
                    self.chats_loaded = true;
                    if let Some((jid, draft)) = self.waiting.take() {
                        commands.extend(self.open_now(&jid, draft));
                    }
                }
                commands
            }
            // A send from a life that has ended: it marks its own send, and its loss (if any) was
            // the old link's, already shown.
            Incoming::LateReply { request, result } => {
                match driver::settle(&mut self.model, &request, result, CLIENT) {
                    Outcome::Effect(effect) => commands(Some(effect)),
                    Outcome::Done | Outcome::Lost(_) => Vec::new(),
                }
            }
            Incoming::NotConnected(frame) => {
                driver::not_connected(&mut self.model, &frame, CLIENT);
                Vec::new()
            }
            Incoming::Event(frame) => {
                self.notify(&frame);
                commands(self.model.apply(&frame))
            }
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
        let frame = self.model.open_selected();
        self.withdraw_selected();
        send(frame)
    }

    /// Open the chat `jid` names from outside the list, as a click on its notification does
    /// ([`OPEN_CHAT_ACTION`]): selected, opened and marked read as [`Controller::open_chat`] does;
    /// a chat the list does not have gets an empty conversation of its own
    /// ([`Model::open_jid`]). Before the first chat list arrives (a cold start), it waits for it.
    pub fn open_jid(&mut self, jid: &str) -> Vec<Command> {
        self.open_or_wait(jid, None)
    }

    /// Open a `whatsapp:` / `wa.me` link ([`parse_link`]): its chat as [`Controller::open_jid`]
    /// does, and its text, if any, in the composer in place of the draft. Nothing is ever sent:
    /// only the user's own send does. A group invite or an invalid link only says why.
    pub fn open_link(&mut self, link: &str) -> Vec<Command> {
        match parse_link(link) {
            Ok(target) => self.open_or_wait(&target.jid, target.draft),
            Err(LinkError::GroupInvite) => {
                self.model.set_notice(LinkError::GroupInvite.to_string());
                Vec::new()
            }
            Err(error) => {
                self.model
                    .set_notice(format!("cannot open the link: {error}"));
                Vec::new()
            }
        }
    }

    /// "New message": open the chat of the phone number `written` as its link with no text does
    /// ([`number_link`], then [`Controller::open_link`]), so nothing is ever sent, and always
    /// with an empty composer (S8): a draft typed in another chat is cleared first, so a double
    /// Enter can never send it to the new number. An invalid number opens nothing, clears
    /// nothing and leaves no notice: the error ([`BAD_NUMBER`]) is for the dialog that asked,
    /// which stays open.
    pub fn new_message(&mut self, written: &str) -> Result<Vec<Command>, &'static str> {
        let link = number_link(written)?;
        if !self.model.composer().is_empty() {
            self.model.composer_mut().set_text("");
            self.draft_changed = true;
        }
        Ok(self.open_link(&link))
    }

    fn open_or_wait(&mut self, jid: &str, draft: Option<String>) -> Vec<Command> {
        if self.chats_loaded {
            self.open_now(jid, draft)
        } else {
            self.waiting = Some((jid.to_owned(), draft));
            Vec::new()
        }
    }

    fn open_now(&mut self, jid: &str, draft: Option<String>) -> Vec<Command> {
        let before = self.model.selected_jid().map(str::to_owned);
        let frame = self.model.open_jid(jid);
        if self.model.selected_jid() != before.as_deref() {
            self.model.composer_mut().cancel_reply();
        }
        if let Some(draft) = draft {
            self.model.composer_mut().set_text(&draft);
            self.draft_changed = true;
        }
        self.withdraw_selected();
        send(frame)
    }

    /// The notifications to show or withdraw since the last call, oldest first.
    pub fn take_alerts(&mut self) -> Vec<Alert> {
        std::mem::take(&mut self.alerts)
    }

    /// Whether the draft was replaced here (a link) since the last call.
    pub fn take_draft_changed(&mut self) -> bool {
        std::mem::take(&mut self.draft_changed)
    }

    /// The notification an event deserves ([`notification_for`], seen from what the window shows
    /// now), and the withdrawal of a chat read elsewhere.
    fn notify(&mut self, frame: &DaemonFrame) {
        if let DaemonFrame::EventUnread {
            jid,
            chat_unread: 0,
            ..
        } = frame
        {
            self.withdraw(jid);
        }
        self.view.focused_chat = self
            .model
            .is_open()
            .then(|| self.model.selected_jid().map(str::to_owned))
            .flatten();
        self.view.window_visible = self.model.is_seen();
        let chats = self.model.chat_list().chats();
        if let Some(notification) = notification_for(frame, chats, &self.view, &self.config) {
            self.view.notified.record(&notification);
            self.notifying.insert(notification.chat_jid.clone());
            self.alerts.push(Alert::Show(notification));
        }
    }

    fn withdraw_selected(&mut self) {
        if let Some(jid) = self.model.selected_jid().map(str::to_owned) {
            self.withdraw(&jid);
        }
    }

    fn withdraw(&mut self, jid: &str) {
        if self.notifying.remove(jid) {
            self.alerts.push(Alert::Withdraw(jid.to_owned()));
        }
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

    /// Open media message `index` (a click on its picture, Enter, `o`, or Open in its menu): a
    /// picture's full file is fetched to show it in its bubble ([`crate::pictures`]); any other
    /// media file is fetched to open it outside the app. Only this asks for a download.
    pub fn open_media(&mut self, index: usize) -> Vec<Command> {
        let commands = self.download(index, false);
        if let [Command::Send(ClientFrame::DownloadMedia { jid, id, .. })] = commands.as_slice()
            && self
                .model
                .selected_message()
                .is_some_and(|m| nuncio_client::model::is_picture(&m.kind))
        {
            self.picture_retries.push((jid.clone(), id.clone()));
        }
        commands
    }

    /// The pictures asked for again since the last call (the window lets a failed full picture
    /// be decoded once more), oldest first.
    pub fn take_picture_retries(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.picture_retries)
    }

    /// Fetch media message `index`'s file to open it outside the app, a picture too (Shift+O, or
    /// Open externally in its menu).
    pub fn open_externally(&mut self, index: usize) -> Vec<Command> {
        self.download(index, true)
    }

    fn download(&mut self, index: usize, external: bool) -> Vec<Command> {
        if !self.model.select_message(index) {
            return Vec::new();
        }
        send(
            self.model
                .download_selected(true, external)
                .map(|download| download.frame()),
        )
    }

    /// A full picture fetched to show could not be shown: the notice says why (its label stays).
    pub fn picture_failed(&mut self, error: &str) {
        self.model
            .set_notice(format!("cannot show the picture: {error}"));
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
