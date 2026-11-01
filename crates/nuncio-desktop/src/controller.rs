//! The app's state and what each user action or daemon answer does to it, without GTK: the
//! shared [`Model`] holds every rule, the shared driver ([`nuncio_client::driver`]) folds the
//! worker's answers in, and what is left to do outside (send a frame to the worker, open a file)
//! comes back as [`Command`]s. The window calls these methods and draws
//! [`crate::present`]'s view of the model afterwards.

use std::cell::{Ref, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use nuncio_client::driver::{self, Outcome};
use nuncio_client::link::{LinkError, parse_link};
use nuncio_client::model::Effect;
use nuncio_client::notify::{Notification, ViewState, notification_for};
use nuncio_client::wire::MessageState;
use nuncio_client::{
    ClientError, ClientFrame, Config, DaemonFrame, LinkFlow, LinkQr, LinkState, LinkUpdate, Model,
    PROTOCOL_VERSION, Session, Snapshot,
};

use crate::unlink::{self, Unsent};
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
    /// Ask the session bus for the daemon: pressing Link with none serving starts it (F6).
    StartDaemon,
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

/// The link screen's state while the app has no device (S4/S6/S8, D3): the consent before
/// anything is sent, the flow's own states, and the two refusals the first press can meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkScreen {
    /// The first screen: the consent text and the one Link button. Nothing has been asked of the
    /// daemon, so no code exists and no keyring has been touched.
    Consent,
    /// Link was pressed and `link_start` went out: the daemon is opening the store and the
    /// keyring, and has published no code yet.
    Starting,
    /// The daemon published a code (a later one replaces it): the screen draws it.
    Waiting,
    /// `link_state: failed`: the daemon's one sentence, and a new code can be asked for.
    Failed,
    /// The daemon refused to start a flow: one sentence, and one Try again.
    Refused(LinkRefusal),
}

/// The answer to a wipe this app sent, until the window closed the confirmation on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The daemon confirmed it: the device is unlinked and nuncio's data is deleted, so the app
    /// ends on the link screen with one line (S4, S11).
    Unlinked,
    /// The daemon answered without confirming it (WhatsApp did not, or the deletion failed): it
    /// said why, nothing is claimed, and Settings stays where it was.
    Unconfirmed,
    /// The connection broke before the answer, because the daemon that held the wipe went away:
    /// whether it happened is the next daemon's word (S11), so nothing is said about the device,
    /// but the dialogs go (F5) and the cut is not reported as an error (F6).
    Lost,
}

/// Why a `link_start` was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The daemon could not read the keyring (locked, or no Secret Service at all): D3's sentence
    /// names both cases and the remedy.
    Keyring,
    /// Any other refusal, as the daemon worded it (a flow already running, a device linked).
    Other(String),
}

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
    /// The draft the composer widget shows. When the model's draft differs (another chat
    /// opened, which keeps its own draft, or a link's text), the widget must show the model's.
    shown_draft: String,
    /// Pictures whose full file was asked for again ([`Controller::open_media`]), by chat and
    /// id: one that could not be shown may be decoded once more.
    picture_retries: Vec<(String, String)>,
    /// The link screen while the app has no device, and what the daemon published for it. Shared
    /// between clones of one controller (a flow is pairing material, and nothing duplicates it).
    link: Rc<RefCell<LinkFlow>>,
    link_screen: LinkScreen,
    /// A flow paired and the screen is gone, before the session's own frames say `linked`.
    link_linked: bool,
    /// A `link_cancel` the worker could not deliver (the link was down): sent on the next
    /// connect, so a flow the user stopped leaves nothing behind (S7).
    link_cancel_pending: bool,
    /// Whether this app sent the `link_start` of the flow the screen is showing. The daemon
    /// publishes `link_qr`/`link_state` to every subscriber, so a flow another client started
    /// (`nuncio link --qr` in a terminal) is on this screen too; leaving this window must not
    /// cancel that one.
    link_started_here: bool,
    /// A `link_start` the daemon never received (the link was down): sent on the next connect, so
    /// pressing Link with no daemon serving still pairs once the bus has started one (F6).
    link_start_pending: bool,
    /// A `wipe` this app sent has no answer yet, or its answer was lost with the daemon that held
    /// it: a cut while it is set is the unlink's own, never an error.
    unlink_pending: bool,
    /// A `wipe` the app sent is in flight: a second one is never sent (S11).
    unlink_running: bool,
    /// The daemon answered the app's `wipe` (S11): the device is gone and so is nuncio's data, so
    /// the first screen belongs on the screen even while the daemon that replaces it starts.
    wiped: bool,
    /// The answer to the app's wipe, until the window closed the confirmation on it.
    unlink_settled: Option<Settled>,
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
            shown_draft: String::new(),
            picture_retries: Vec::new(),
            link: Rc::new(RefCell::new(LinkFlow::new())),
            link_screen: LinkScreen::Consent,
            link_linked: false,
            link_cancel_pending: false,
            link_started_here: false,
            link_start_pending: false,
            unlink_pending: false,
            unlink_running: false,
            wiped: false,
            unlink_settled: None,
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
                let mut commands: Vec<Command> = frames.into_iter().map(Command::Send).collect();
                // A cancel made while the link was down reaches the daemon now, before anything
                // else, so the flow it stopped leaves nothing behind (S7).
                if std::mem::take(&mut self.link_cancel_pending) {
                    commands.insert(0, Command::Send(ClientFrame::link_cancel()));
                }
                // A Link pressed while nothing served goes out now: the daemon is there, so the
                // flow the press asked for starts (F6).
                if std::mem::take(&mut self.link_start_pending) {
                    commands.push(Command::Send(ClientFrame::link_start(true)));
                }
                // A new daemon is the word on the device the lost wipe left open (S11): from here
                // its status decides, not the lost connection.
                self.unlink_pending = false;
                commands
            }
            Incoming::Reply { request, result } => {
                // The confirmation's own `wipe` (S11): the model has nothing to say about it.
                if matches!(request, ClientFrame::Wipe { .. }) {
                    return self.wipe_reply(result);
                }
                if matches!(
                    request,
                    ClientFrame::LinkStart { .. } | ClientFrame::LinkCancel { .. }
                ) {
                    return self.link_reply(&request, result);
                }
                // The daemon's own word on the device decides whether the link screen is still
                // gone: after a logout (or a daemon restarted without a device) it comes back.
                if let Ok(DaemonFrame::Status { session, .. }) = &result
                    && *session == Session::Unlinked
                {
                    self.link_linked = false;
                }
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
                // The wipe's answer, on a connection that ended since: the daemon's word stands.
                if matches!(request, ClientFrame::Wipe { .. }) {
                    return self.wipe_reply(result);
                }
                match driver::settle(&mut self.model, &request, result, CLIENT) {
                    Outcome::Effect(effect) => commands(Some(effect)),
                    Outcome::Done | Outcome::Lost(_) => Vec::new(),
                }
            }
            Incoming::NotConnected(frame) => {
                if matches!(frame, ClientFrame::LinkStart { .. }) {
                    // Nothing reached the daemon: no flow was started.
                    *self.link.borrow_mut() = LinkFlow::new();
                    self.link_screen = LinkScreen::Consent;
                    self.link_started_here = false;
                    self.link_start_pending = false;
                }
                if matches!(frame, ClientFrame::Wipe { .. }) {
                    // Nothing reached the daemon, so nothing was deleted, and the confirmation
                    // can be pressed again.
                    self.unlink_running = false;
                    self.unlink_pending = false;
                }
                driver::not_connected(&mut self.model, &frame, CLIENT);
                Vec::new()
            }
            Incoming::Event(frame) => {
                self.notify(&frame);
                if matches!(
                    &frame,
                    DaemonFrame::EventConnection {
                        session: Session::Unlinked,
                        ..
                    }
                ) {
                    self.link_linked = false;
                }
                let mut link = self.fold_link(&frame);
                // A paired device handed over by the daemon (this app's flow, or a link another
                // client made) is served at once: ask for the chats, so the list appears without
                // a restart (S4).
                if !self.link_linked
                    && self.model.status().session != Session::Linked
                    && matches!(
                        &frame,
                        DaemonFrame::EventConnection {
                            session: Session::Linked,
                            ..
                        }
                    )
                {
                    self.link_linked = true;
                    self.wiped = false;
                    link.extend(self.model.resync().into_iter().map(Command::Send));
                }
                link.extend(commands(self.model.apply(&frame)));
                link
            }
            Incoming::Lost(error) => {
                // Whether a wipe the connection carried happened is the daemon's word on the next
                // connection, never this client's: nothing is claimed here (S11).
                self.lost(&error);
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

    /// The link screen, while the app has no device: the daemon says `unlinked` and no flow has
    /// paired yet (S4). `None` at any other time, and the panes show instead. A wipe this app
    /// confirmed leaves no device and no data, so the screen stays up across the reconnect to the
    /// daemon that replaces the wiped one, instead of flashing empty panes (S11).
    pub fn link_screen(&self) -> Option<&LinkScreen> {
        let unlinked = self.model.status().session == Session::Unlinked
            && (self.model.connected() || self.wiped);
        (unlinked && !self.link_linked).then_some(&self.link_screen)
    }

    /// The code the link screen draws now; `None` before the first one, and once the flow ended.
    pub fn link_qr(&self) -> Option<LinkQr> {
        self.flow().qr().cloned()
    }

    /// The daemon's failure sentence, only while the flow is in its failed state: every state
    /// frame refreshes it, so an earlier one is never shown (T6 review, L13b).
    pub fn link_reason(&self) -> Option<String> {
        let flow = self.flow();
        (flow.phase() == Some(LinkState::Failed))
            .then(|| flow.reason().map(str::to_owned))
            .flatten()
    }

    fn flow(&self) -> Ref<'_, LinkFlow> {
        self.link.borrow()
    }

    /// The link screen's one button: Link (the consent, S6), Try again after a refusal (which
    /// asks the daemon for its state again first, D3), or a new code after a failed flow. With no
    /// daemon serving (the one that held the unlink stopped with it), the press is what asks the
    /// session bus for one, and the start waits for that connection (F6).
    pub fn start_link(&mut self) -> Vec<Command> {
        // The button press is the consent: this is the only `link_start` carrying `consent:
        // true`, so nothing links before it.
        let probe = matches!(self.link_screen, LinkScreen::Refused(_));
        let connected = self.model.connected();
        *self.link.borrow_mut() = LinkFlow::new();
        self.link_linked = false;
        self.link_started_here = true;
        self.link_screen = LinkScreen::Starting;
        let mut commands: Vec<Command> = Vec::new();
        if connected {
            if probe {
                commands.push(Command::Send(ClientFrame::status()));
            }
            commands.push(Command::Send(ClientFrame::link_start(true)));
        } else {
            // Nothing to talk to: the daemon is started first, and the flow the press asked for
            // goes out on the connection that follows.
            self.link_start_pending = true;
            commands.push(Command::StartDaemon);
        }
        commands
    }

    /// Esc or Cancel: stop a flow the app started and go back to the consent screen; the daemon
    /// answers once what the flow created is gone (S7). A cancel that cannot be delivered now is
    /// sent on the next connect. A press that never reached the daemon sent nothing, so there is
    /// nothing to cancel and no flow to start later.
    pub fn cancel_link(&mut self) -> Vec<Command> {
        let running = self.link_started_here
            && matches!(self.link_screen, LinkScreen::Starting | LinkScreen::Waiting);
        let unsent = std::mem::take(&mut self.link_start_pending);
        *self.link.borrow_mut() = LinkFlow::new();
        self.link_screen = LinkScreen::Consent;
        self.link_started_here = false;
        if !running || unsent {
            return Vec::new();
        }
        self.link_cancel_pending = true;
        vec![Command::Send(ClientFrame::link_cancel())]
    }

    /// What a wipe would destroy that the user has not sent yet: the draft of every chat that has
    /// one, and the sends the daemon still holds queued. The confirmation says how much before
    /// asking for the word (S11).
    pub fn unsent_work(&self) -> Unsent {
        let mut jids: Vec<&str> = self
            .model
            .chat_list()
            .chats()
            .iter()
            .map(|chat| chat.jid.as_str())
            .collect();
        // The chat whose conversation is open may have vanished from the list and kept its draft.
        if let Some(jid) = self.model.selected_jid()
            && !jids.contains(&jid)
        {
            jids.push(jid);
        }
        let queued = jids
            .into_iter()
            .filter_map(|jid| self.model.transcript().conversation(jid))
            .flatten()
            .filter(|message| message.from_me && message.state == Some(MessageState::Queued))
            .count();
        // Drafts live per chat in the model, which shows only the selected one: a copy is asked
        // chat by chat through the public selection, so nothing of the model is duplicated here.
        let mut probe = self.model.clone();
        let shown = probe.selected_jid().map(str::to_owned);
        let mut drafts = usize::from(!probe.composer().text().is_empty());
        for (index, chat) in self.model.chat_list().chats().iter().enumerate() {
            if shown.as_deref() == Some(chat.jid.as_str()) {
                continue;
            }
            probe.select_chat(index);
            drafts += usize::from(!probe.composer().text().is_empty());
        }
        Unsent { drafts, queued }
    }

    /// The line the confirmation shows before the field, when a wipe would destroy unsent work.
    pub fn unlink_warning(&self) -> Option<String> {
        self.unsent_work().sentence()
    }

    /// The confirmation's one button: it sends the daemon's `wipe` with `unlink: true` once (S11),
    /// and only when `typed` is exactly [`unlink::WORD`]. There is no new frame: this is the
    /// daemon's existing logout path (`proto/wire.md`, the wipe frame).
    pub fn confirm_unlink(&mut self, typed: &str) -> Vec<Command> {
        if self.unlink_running || !self.model.status().linked || typed != unlink::WORD {
            return Vec::new();
        }
        self.unlink_running = true;
        self.unlink_pending = true;
        vec![Command::Send(ClientFrame::wipe(true))]
    }

    /// Whether a `wipe` is in flight: the confirmation keeps its button disabled until it lands.
    pub fn unlink_running(&self) -> bool {
        self.unlink_running
    }

    /// The answer to a wipe the user confirmed, until the window closed the confirmation on it.
    pub fn take_unlink_settled(&mut self) -> Option<Settled> {
        self.unlink_settled.take()
    }

    /// The daemon's answer to the confirmation (S11): the device is unlinked and the data is
    /// gone (the app ends on the link screen, S4), WhatsApp did not confirm it, or nothing was
    /// deleted and the daemon said why.
    fn wipe_reply(&mut self, result: Result<DaemonFrame, ClientError>) -> Vec<Command> {
        self.unlink_running = false;
        match result {
            Ok(DaemonFrame::Wiped { unlinked: true, .. }) => {
                self.unlink_pending = false;
                self.unlink_settled = Some(Settled::Unlinked);
                // The daemon deleted its data, its socket and exited: this app is unlinked, and
                // the first screen belongs here without a restart. What the daemon that replaces
                // it reports says the same.
                self.wiped = true;
                self.link_linked = false;
                self.link_screen = LinkScreen::Consent;
                self.link_started_here = false;
                *self.link.borrow_mut() = LinkFlow::new();
                let daemon = self.model.status().daemon.clone();
                self.model.apply(&DaemonFrame::Status {
                    v: PROTOCOL_VERSION,
                    daemon,
                    linked: false,
                    session: Session::Unlinked,
                    unread: 0,
                    stop_reason: None,
                });
                self.model.set_notice(unlink::UNLINKED.to_owned());
                Vec::new()
            }
            Ok(DaemonFrame::Wiped {
                unlinked: false, ..
            }) => {
                self.unlink_pending = false;
                self.unlink_settled = Some(Settled::Unconfirmed);
                self.model.set_notice(unlink::UNCONFIRMED.to_owned());
                Vec::new()
            }
            // The daemon refused the wipe (a logout WhatsApp did not confirm, a deletion that
            // failed): its own sentence, and nothing changes here.
            Err(ClientError::Daemon { message, .. }) => {
                self.unlink_pending = false;
                self.unlink_settled = Some(Settled::Unconfirmed);
                self.model.set_notice(message);
                Vec::new()
            }
            // An answer to a wipe that says something else: nothing is claimed for it.
            Ok(other) => {
                self.unlink_pending = false;
                self.unlink_settled = Some(Settled::Unconfirmed);
                self.model
                    .set_notice(format!("nunciod answered `{}` to the unlink", other.op()));
                Vec::new()
            }
            // The connection broke before the answer: whether the wipe happened is the daemon's
            // word on the next connection, never this client's.
            Err(error) => {
                self.lost(&error);
                Vec::new()
            }
        }
    }

    /// The connection broke with `error`. The cut an unlink causes is expected, because the
    /// daemon that held the device stops with it: it is not painted as an error (F6). While the
    /// wipe still has no answer, the confirmation and Settings close instead of waiting for one
    /// (F5) and the app shows its first screen while nothing serves it. Nothing is said about the
    /// device (S11): the status of the daemon that replaces it replaces the state set here.
    fn lost(&mut self, error: &ClientError) {
        self.unlink_running = false;
        if !self.unlink_pending && !self.wiped {
            driver::lost(&mut self.model, error);
            return;
        }
        self.model.set_connected(false);
        if !self.unlink_pending {
            // Already answered: no dialog is waiting for it any more.
            return;
        }
        self.unlink_settled = Some(Settled::Lost);
        self.wiped = true;
        self.link_linked = false;
        self.link_screen = LinkScreen::Consent;
        self.link_started_here = false;
        *self.link.borrow_mut() = LinkFlow::new();
        let daemon = self.model.status().daemon.clone();
        self.model.apply(&DaemonFrame::Status {
            v: PROTOCOL_VERSION,
            daemon,
            linked: false,
            session: Session::Unlinked,
            unread: 0,
            stop_reason: None,
        });
    }

    /// Fold one daemon frame into the link screen; returns the frames the change asks for.
    fn fold_link(&mut self, frame: &DaemonFrame) -> Vec<Command> {
        let update = self.link.borrow_mut().apply(frame);
        match update {
            Some(LinkUpdate::Qr) => {
                if !self.link_linked {
                    self.link_screen = LinkScreen::Waiting;
                }
                Vec::new()
            }
            Some(LinkUpdate::State(LinkState::Waiting)) => {
                if !self.link_linked {
                    self.link_screen = LinkScreen::Waiting;
                }
                Vec::new()
            }
            Some(LinkUpdate::State(LinkState::Failed)) => {
                // The daemon ended the flow and removed what it created, whoever started it.
                self.link_started_here = false;
                self.link_screen = LinkScreen::Failed;
                Vec::new()
            }
            Some(LinkUpdate::State(LinkState::Linked)) => {
                // The daemon serves the paired device now: the chat list appears without a
                // restart. A clean first screen is left for a later logout.
                self.link_linked = true;
                self.wiped = false;
                self.link_started_here = false;
                *self.link.borrow_mut() = LinkFlow::new();
                self.link_screen = LinkScreen::Consent;
                self.model.resync().into_iter().map(Command::Send).collect()
            }
            Some(LinkUpdate::State(LinkState::Unknown)) | None => Vec::new(),
        }
    }

    /// The answer to a link frame the screen words itself (the model has nothing to say about
    /// linking, and its notices would repeat the daemon's whole error).
    fn link_reply(
        &mut self,
        request: &ClientFrame,
        result: Result<DaemonFrame, ClientError>,
    ) -> Vec<Command> {
        match result {
            // The daemon may publish the first code before it answers `link_started` (the two
            // connections are separate): a code on the screen is not taken away by the answer.
            Ok(DaemonFrame::LinkStarted { .. }) => {
                if self.flow().qr().is_none() {
                    self.link_screen = LinkScreen::Starting;
                }
                Vec::new()
            }
            Ok(DaemonFrame::LinkCancelled { .. }) => {
                *self.link.borrow_mut() = LinkFlow::new();
                self.link_screen = LinkScreen::Consent;
                self.link_cancel_pending = false;
                self.link_started_here = false;
                Vec::new()
            }
            Ok(_) => Vec::new(),
            // A cancel the daemon could not finish removing everything for is never reported as
            // clean (S7).
            Err(ClientError::Daemon { message, .. })
                if matches!(request, ClientFrame::LinkCancel { .. }) =>
            {
                *self.link.borrow_mut() = LinkFlow::new();
                self.link_screen = LinkScreen::Consent;
                self.link_cancel_pending = false;
                self.link_started_here = false;
                self.model.set_notice(format!(
                    "nuncio could not remove everything linking created: {message}"
                ));
                Vec::new()
            }
            Err(ClientError::Daemon { message, .. }) => {
                *self.link.borrow_mut() = LinkFlow::new();
                // The daemon refused the start: no flow of ours runs.
                self.link_started_here = false;
                self.link_screen = if message.to_lowercase().contains("keyring") {
                    LinkScreen::Refused(LinkRefusal::Keyring)
                } else {
                    LinkScreen::Refused(LinkRefusal::Other(message))
                };
                Vec::new()
            }
            // The connection broke before the answer: the worker reconnects, and the daemon's own
            // frames arrive on the new subscription if the flow started.
            Err(error) => {
                driver::lost(&mut self.model, &error);
                Vec::new()
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
        // Each chat keeps its own draft and reply ([`Model::composer`]): opening another chat
        // shows its own, and the one left stays for its chat.
        self.model.select_chat(index);
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
    /// does, and its text, if any, in that chat's composer in place of its draft (without text,
    /// the chat shows its own draft, never another chat's). Nothing is ever sent:
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
    /// ([`number_link`], then [`Controller::open_link`]), so nothing is ever sent. The composer
    /// shows that chat's own draft (S8): empty for a new number, so a double Enter can never
    /// send a draft typed in another chat, which stays saved for that chat. An invalid number
    /// opens nothing and leaves no notice: the error ([`BAD_NUMBER`]) is for the dialog that
    /// asked, which stays open.
    pub fn new_message(&mut self, written: &str) -> Result<Vec<Command>, &'static str> {
        let link = number_link(written)?;
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
        let frame = self.model.open_jid(jid);
        if let Some(draft) = draft {
            self.model.composer_mut().set_text(&draft);
        }
        self.withdraw_selected();
        send(frame)
    }

    /// The notifications to show or withdraw since the last call, oldest first.
    pub fn take_alerts(&mut self) -> Vec<Alert> {
        std::mem::take(&mut self.alerts)
    }

    /// Whether the model's draft differs from the one the composer widget shows (another chat
    /// opened with its own draft, or a link's text): the widget must show the model's. Once
    /// asked, the widget is taken to show it.
    pub fn take_draft_changed(&mut self) -> bool {
        let draft = self.model.composer().text();
        if self.shown_draft == draft {
            return false;
        }
        draft.clone_into(&mut self.shown_draft);
        true
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
        text.clone_into(&mut self.shown_draft);
        if self.model.composer().text() != text {
            self.model.composer_mut().set_text(text);
        }
    }

    /// Send the draft (see [`Model::send`]); `true` when it was sent, so the text widget empties.
    pub fn send(&mut self) -> (bool, Vec<Command>) {
        let frame = self.model.send();
        if frame.is_some() {
            self.shown_draft.clear();
        }
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

#[cfg(test)]
mod unlink_tests {
    use super::*;
    use crate::unlink::{UNCONFIRMED, UNLINKED, Unsent, WORD};
    use nuncio_client::{Chat, PROTOCOL_VERSION};

    const ANA: &str = "15551230001@s.whatsapp.net";
    const BEN: &str = "15551230002@s.whatsapp.net";

    fn chat(jid: &str, name: &str) -> Chat {
        Chat {
            jid: jid.to_owned(),
            name: name.to_owned(),
            unread: 0,
            archived: false,
            pinned: false,
            muted: false,
            aliases: Vec::new(),
            phone: None,
            last_message: None,
        }
    }

    /// A controller over a linked device with Ana and Ben listed.
    fn linked() -> Controller {
        let model = Model::new(
            Snapshot {
                daemon: "nunciod/test".to_owned(),
                linked: true,
                session: Session::Linked,
                unread: 0,
                chats: vec![chat(ANA, "Ana"), chat(BEN, "Ben")],
                stop_reason: None,
            },
            "d",
        );
        Controller::with_model(model)
    }

    /// The daemon's answer to the one `wipe` the confirmation sends.
    fn wiped(unlinked: bool) -> Incoming {
        Incoming::Reply {
            request: ClientFrame::wipe(true),
            result: Ok(DaemonFrame::Wiped {
                v: PROTOCOL_VERSION,
                unlinked,
            }),
        }
    }

    #[test]
    fn nothing_unlinks_before_the_exact_word_is_typed() {
        let mut c = linked();
        for wrong in [
            "", " ", "unlin", "unlinked", "Unlink", "UNLINK", "unlink ", " unlink",
        ] {
            assert!(
                c.confirm_unlink(wrong).is_empty(),
                "`{wrong}` is not the word"
            );
            assert!(!c.unlink_running(), "`{wrong}` sent nothing");
        }
        assert_eq!(
            c.confirm_unlink(WORD),
            [Command::Send(ClientFrame::wipe(true))],
            "the exact word sends the daemon's own wipe, once"
        );
        assert!(c.unlink_running());
    }

    #[test]
    fn a_second_wipe_never_goes_out_while_one_runs() {
        let mut c = linked();
        assert_eq!(c.confirm_unlink(WORD).len(), 1);
        assert!(c.confirm_unlink(WORD).is_empty(), "a repeat confirm");
        assert!(
            c.confirm_unlink(WORD).is_empty(),
            "a double click is one wipe"
        );
        c.incoming(wiped(false));
        assert!(!c.unlink_running(), "answered: a new attempt is possible");
        assert_eq!(c.confirm_unlink(WORD).len(), 1);
    }

    #[test]
    fn an_unlinked_app_does_not_unlink() {
        let model = Model::new(
            Snapshot {
                daemon: "nunciod/test".to_owned(),
                linked: false,
                session: Session::Unlinked,
                unread: 0,
                chats: Vec::new(),
                stop_reason: None,
            },
            "d",
        );
        let mut c = Controller::with_model(model);
        assert!(c.confirm_unlink(WORD).is_empty());
    }

    #[test]
    fn the_daemons_unlinked_answer_ends_on_the_link_screen_with_one_notice() {
        let mut c = linked();
        assert_eq!(c.confirm_unlink(WORD).len(), 1);
        c.incoming(wiped(true));
        assert_eq!(
            c.link_screen(),
            Some(&LinkScreen::Consent),
            "the app ends on the first screen, not on empty panes"
        );
        assert_eq!(c.take_notice().as_deref(), Some(UNLINKED));
        assert_eq!(c.model().status().session, Session::Unlinked);
        assert!(!c.model().status().linked, "the device is gone");
        assert!(!c.unlink_running());
        assert_eq!(c.take_unlink_settled(), Some(Settled::Unlinked));
        assert_eq!(c.take_unlink_settled(), None, "said once");
    }

    #[test]
    fn an_unconfirmed_unlink_is_said_and_claims_nothing() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(wiped(false));
        assert_eq!(c.take_notice().as_deref(), Some(UNCONFIRMED));
        assert_eq!(
            c.link_screen(),
            None,
            "the device stays linked: the panes stay"
        );
        assert_eq!(c.model().status().session, Session::Linked);
        assert_eq!(c.take_unlink_settled(), Some(Settled::Unconfirmed));
    }

    #[test]
    fn a_refused_wipe_shows_the_daemons_message_and_changes_nothing() {
        let message =
            "could not delete all of nuncio's local data; run nuncio purge --confirm to finish";
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(Incoming::Reply {
            request: ClientFrame::wipe(true),
            result: Err(ClientError::Daemon {
                code: "internal".to_owned(),
                message: message.to_owned(),
            }),
        });
        assert_eq!(c.take_notice().as_deref(), Some(message));
        assert_eq!(c.model().status().session, Session::Linked);
        assert!(c.model().status().linked);
        assert!(c.link_screen().is_none());
        assert_eq!(c.take_unlink_settled(), Some(Settled::Unconfirmed));
        assert_eq!(c.confirm_unlink(WORD).len(), 1, "the user may try again");
    }

    #[test]
    fn a_wipe_that_never_left_the_app_claims_nothing_and_can_be_redone() {
        let mut c = linked();
        let commands = c.confirm_unlink(WORD);
        let [Command::Send(frame)] = commands.as_slice() else {
            panic!("one wipe");
        };
        let frame = frame.clone();
        c.incoming(Incoming::NotConnected(frame));
        assert!(!c.unlink_running());
        assert_eq!(c.take_unlink_settled(), None);
        assert_eq!(
            c.model().notice(),
            Some("not connected to nunciod; reconnecting…")
        );
        assert_eq!(c.confirm_unlink(WORD).len(), 1, "nothing happened");
    }

    #[test]
    fn a_wipe_whose_daemon_dies_closes_its_dialogs_without_an_error() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(Incoming::Lost(ClientError::Closed));
        assert!(!c.unlink_running(), "nothing is in flight any more");
        assert_eq!(
            c.take_unlink_settled(),
            Some(Settled::Lost),
            "the confirmation and Settings go even though the answer never came (F5)"
        );
        assert_eq!(
            c.take_notice(),
            None,
            "the cut our own unlink causes is not painted as an error (F6)"
        );
        assert!(
            !c.model().connected(),
            "the rail still says the app is reconnecting"
        );
        assert_eq!(
            c.link_screen(),
            Some(&LinkScreen::Consent),
            "the first screen belongs here while the daemon that held the wipe is gone"
        );
        assert_eq!(c.model().status().session, Session::Unlinked);
    }

    #[test]
    fn a_wipe_whose_answer_never_comes_is_the_same_expected_cut() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        // The daemon stops with the wipe and the read hits its bound: F6's own line, which the
        // app no longer paints as an error.
        c.incoming(Incoming::Reply {
            request: ClientFrame::wipe(true),
            result: Err(ClientError::Timeout(std::time::Duration::from_secs(5))),
        });
        assert_eq!(c.take_unlink_settled(), Some(Settled::Lost));
        assert_eq!(c.take_notice(), None, "not painted as an error (F6)");
        assert_eq!(c.link_screen(), Some(&LinkScreen::Consent));
    }

    #[test]
    fn the_cut_after_a_confirmed_unlink_is_not_an_error_either() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(wiped(true));
        assert_eq!(c.take_notice().as_deref(), Some(UNLINKED));
        // The daemon exits right after its answer, as the wipe does: the cut is expected.
        c.incoming(Incoming::Lost(ClientError::Closed));
        assert_eq!(c.take_notice(), None, "not painted as an error (F6)");
        assert_eq!(c.link_screen(), Some(&LinkScreen::Consent));
    }

    #[test]
    fn the_next_daemons_word_replaces_the_first_screen_the_lost_wipe_showed() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(Incoming::Lost(ClientError::Closed));
        // The daemon that replaces it still has the device, because the wipe never ran: its own
        // status says so, and the panes come back. Nothing was claimed.
        c.incoming(Incoming::Reply {
            request: ClientFrame::status(),
            result: Ok(DaemonFrame::Status {
                v: PROTOCOL_VERSION,
                daemon: "nunciod/test".to_owned(),
                linked: true,
                session: Session::Linked,
                unread: 0,
                stop_reason: None,
            }),
        });
        assert_eq!(c.link_screen(), None, "the device is still linked");
        assert!(c.model().status().linked);
    }

    #[test]
    fn pressing_link_with_no_daemon_asks_for_one_and_starts_the_flow_once_it_answers() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(Incoming::Lost(ClientError::Closed));
        assert_eq!(
            c.start_link(),
            [Command::StartDaemon],
            "the press is what asks the bus for the daemon our unlink stopped (F6)"
        );
        assert_eq!(c.link_screen(), Some(&LinkScreen::Starting));
        let commands = c.incoming(Incoming::Connected);
        assert!(
            commands.contains(&Command::Send(ClientFrame::link_start(true))),
            "the flow starts as soon as the daemon answers: {commands:?}"
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command, Command::Send(ClientFrame::LinkStart { .. })))
                .count(),
            1,
            "one start, and only once: {commands:?}"
        );
        let again = c.incoming(Incoming::Connected);
        assert!(
            !again.contains(&Command::Send(ClientFrame::link_start(true))),
            "a later reconnect does not start a second flow: {again:?}"
        );
    }

    #[test]
    fn a_press_that_never_reached_the_daemon_is_cancelled_without_a_frame() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(Incoming::Lost(ClientError::Closed));
        assert_eq!(c.start_link(), [Command::StartDaemon]);
        assert!(
            c.cancel_link().is_empty(),
            "there is no flow to stop, so nothing is sent"
        );
        let commands = c.incoming(Incoming::Connected);
        assert!(
            !commands.contains(&Command::Send(ClientFrame::link_start(true))),
            "a cancelled press never pairs the device behind the user: {commands:?}"
        );
        assert_eq!(c.link_screen(), Some(&LinkScreen::Consent));
    }

    #[test]
    fn the_link_screen_stays_up_while_the_wiped_daemon_restarts() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(wiped(true));
        c.take_notice();
        // The wiped daemon exits and its socket goes, so the worker reconnects on the backoff.
        c.incoming(Incoming::Lost(ClientError::Closed));
        assert_eq!(
            c.link_screen(),
            Some(&LinkScreen::Consent),
            "the app does not flash empty panes while the daemon restarts"
        );
        c.incoming(Incoming::Connected);
        assert_eq!(c.link_screen(), Some(&LinkScreen::Consent));
    }

    #[test]
    fn a_linked_device_brings_the_panes_back_after_a_wipe() {
        let mut c = linked();
        c.confirm_unlink(WORD);
        c.incoming(wiped(true));
        c.take_notice();
        c.incoming(Incoming::Event(DaemonFrame::EventConnection {
            v: PROTOCOL_VERSION,
            session: Session::Linked,
            stop_reason: None,
        }));
        assert_eq!(c.link_screen(), None, "another device is linked now");
    }

    #[test]
    fn the_count_holds_every_chats_draft_and_queued_send() {
        let mut c = linked();
        assert_eq!(c.unsent_work(), Unsent::default());
        assert_eq!(c.unlink_warning(), None);

        c.open_chat(0);
        c.set_draft("for Ana");
        c.open_chat(1);
        c.set_draft("for Ben");
        assert_eq!(
            c.unsent_work(),
            Unsent {
                drafts: 2,
                queued: 0
            },
            "the draft of a chat the user left is counted too"
        );

        let (sent, commands) = c.send();
        assert!(sent);
        let [Command::Send(frame)] = commands.as_slice() else {
            panic!("one send");
        };
        let ClientFrame::Send { client_id, .. } = frame else {
            panic!("a send");
        };
        c.incoming(Incoming::Reply {
            request: frame.clone(),
            result: Ok(DaemonFrame::Sent {
                v: PROTOCOL_VERSION,
                client_id: client_id.clone(),
                state: nuncio_client::SendState::Queued,
                id: None,
                timestamp: None,
            }),
        });
        assert_eq!(
            c.unsent_work(),
            Unsent {
                drafts: 1,
                queued: 1
            }
        );
        assert_eq!(
            c.unlink_warning().as_deref(),
            Some("1 unsent draft and 1 message waiting to send will be deleted with it.")
        );
        assert!(
            c.unsent_work()
                .sentence()
                .is_some_and(|line| line.contains("queued") || line.contains("waiting to send")),
            "the queued send is named as what it is"
        );
    }
}
