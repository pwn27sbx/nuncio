//! What a client does with its daemon connections, apart from how it waits on them: the
//! request connection and the subscribed event connection opened together ([`Link`]), how the
//! answer to one request (or its failure) is folded into the view ([`settle`]), what a request
//! made while disconnected becomes ([`not_connected`]), and what a loss and a reconnect say
//! ([`lost`], [`reconnected`]). The terminal client drives it from its own loop; the desktop app
//! from a worker thread, folding the outcomes in on its main loop. Both follow the same rules:
//!
//! - a send (or retry) the daemon answers `not_confirmed` is shown as not confirmed (it may have
//!   been delivered); one it refuses with any other `error` is not sent; every `error` becomes
//!   the notice;
//! - a broken connection (closed, timed out, malformed) marks a send it carried not sent, and the
//!   client reconnects ([`crate::Reconnect`]) and resyncs; but a send (or retry) written in full
//!   whose reply timed out ([`ClientError::SendUnanswered`]) is shown as not confirmed, since the
//!   daemon may already have handed it to WhatsApp (a retry reuses its WhatsApp id);
//! - while disconnected nothing is sent: a send is shown as not sent and is never retried by
//!   itself;
//! - a reply whose op this build does not know (a newer daemon) becomes the notice, and a send it
//!   answered is shown as not confirmed.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::conn::{ClientError, Connection};
use crate::model::{Effect, Model};
use crate::wire::{ClientFrame, DaemonFrame, ErrorCode, PROTOCOL_VERSION};

/// The notice's words for a request made while the client is reconnecting.
pub const NOT_CONNECTED: &str = "not connected to nunciod";
/// How long a `download_media` reply may take (the Go client's bound too).
pub const MEDIA_WAIT: Duration = Duration::from_secs(90);

/// How a client names itself in the driver's notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client {
    /// Its program name, in the notice about a reply too new for it.
    pub program: &'static str,
    /// How the user retries a send that was not sent while disconnected (`R retries it once
    /// reconnected` in the terminal client).
    pub retry_hint: &'static str,
}

/// What the driver folds outcomes into: the shared [`Model`], or a client's own view that wraps
/// it (the terminal client's `App`).
pub trait Host {
    /// Fold one daemon frame in ([`Model::apply`]).
    fn apply(&mut self, frame: &DaemonFrame) -> Option<Effect>;
    /// Fold the reply to `request` in ([`Model::apply_reply`]).
    fn apply_reply(&mut self, request: &ClientFrame, reply: &DaemonFrame) -> Option<Effect>;
    /// `frame` failed with `message` ([`Model::request_failed`]).
    fn request_failed(&mut self, frame: &ClientFrame, message: String);
    /// My send `client_id` was not confirmed ([`Model::send_not_confirmed`]).
    fn send_not_confirmed(&mut self, client_id: &str);
    /// The connection went down or came back ([`Model::set_connected`]).
    fn set_connected(&mut self, connected: bool);
    /// A one-line notice for the user.
    fn set_notice(&mut self, notice: String);
    /// What to ask again after a reconnect ([`Model::resync`]).
    fn resync(&mut self) -> Vec<ClientFrame>;
}

impl Host for Model {
    fn apply(&mut self, frame: &DaemonFrame) -> Option<Effect> {
        Model::apply(self, frame)
    }

    fn apply_reply(&mut self, request: &ClientFrame, reply: &DaemonFrame) -> Option<Effect> {
        Model::apply_reply(self, request, reply)
    }

    fn request_failed(&mut self, frame: &ClientFrame, message: String) {
        Model::request_failed(self, frame, message);
    }

    fn send_not_confirmed(&mut self, client_id: &str) {
        Model::send_not_confirmed(self, client_id);
    }

    fn set_connected(&mut self, connected: bool) {
        Model::set_connected(self, connected);
    }

    fn set_notice(&mut self, notice: String) {
        Model::set_notice(self, notice);
    }

    fn resync(&mut self) -> Vec<ClientFrame> {
        Model::resync(self)
    }
}

/// The request connection and the subscribed event connection, opened together.
pub struct Link {
    pub requests: Connection,
    pub events: Connection,
}

impl Link {
    /// Connect and handshake the request connection, then a second connection that subscribes;
    /// every read is bounded by `timeout`.
    pub fn open(path: &Path, client: &str, timeout: Duration) -> Result<Self, ClientError> {
        let mut requests = Connection::connect_with_timeout(path, timeout)?;
        requests.handshake(client)?;
        let mut events = Connection::connect_with_timeout(path, timeout)?;
        events.handshake(client)?;
        events.subscribe()?;
        Ok(Self { requests, events })
    }
}

/// Send `frame` on `requests` and read its reply, a `download_media` allowed [`MEDIA_WAIT`].
pub fn request_on(
    requests: &mut Connection,
    frame: &ClientFrame,
) -> Result<DaemonFrame, ClientError> {
    match frame {
        ClientFrame::DownloadMedia { .. } => requests.request_within(frame, MEDIA_WAIT),
        _ => requests.request(frame),
    }
}

/// What the caller still has to do once a request's answer was folded in.
#[derive(Debug)]
pub enum Outcome {
    /// Nothing more.
    Done,
    /// What the reply asks for: another request, or a file to open.
    Effect(Effect),
    /// The connection broke: drop it, call [`lost`], and reconnect on the backoff.
    Lost(ClientError),
}

/// Whether `error` broke the connection it came from (closed, timed out, malformed), so both
/// connections are dropped and the client reconnects. An `error` frame from the daemon does not:
/// the connection stays usable.
pub fn is_loss(error: &ClientError) -> bool {
    !matches!(error, ClientError::Daemon { .. })
}

/// A send (or retry) `frame` that did not reach WhatsApp is shown as not sent.
fn failed_send(host: &mut impl Host, frame: &ClientFrame, message: String) {
    if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } = frame {
        host.apply(&DaemonFrame::SendFailed {
            v: PROTOCOL_VERSION,
            client_id: client_id.clone(),
            message,
        });
    }
}

/// Fold the answer to `frame` in (see the module docs); `client` names itself in the notice about
/// a reply too new for it.
pub fn settle(
    host: &mut impl Host,
    frame: &ClientFrame,
    result: Result<DaemonFrame, ClientError>,
    client: Client,
) -> Outcome {
    let program = client.program;
    match result {
        // A newer daemon's reply this build cannot read: the request's outcome is unknown, so a
        // send (or retry) is shown as not confirmed (a retry reuses its WhatsApp id).
        Ok(DaemonFrame::Unknown { op }) => {
            if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } =
                frame
            {
                host.send_not_confirmed(client_id);
            }
            host.request_failed(
                frame,
                format!("daemon: unknown reply `{op}`; this {program} may be too old"),
            );
            Outcome::Done
        }
        // The reply to this very request: a different chat in it is the one the daemon served
        // (ADR 0001), which `apply_reply` adopts.
        Ok(reply) => match host.apply_reply(frame, &reply) {
            Some(effect) => Outcome::Effect(effect),
            None => Outcome::Done,
        },
        Err(ClientError::Daemon { code, message }) => {
            match frame {
                ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. }
                    if code == ErrorCode::NotConfirmed.as_str() =>
                {
                    host.send_not_confirmed(client_id)
                }
                _ => failed_send(host, frame, message.clone()),
            }
            host.request_failed(frame, format!("daemon: {code}: {message}"));
            Outcome::Done
        }
        // Written in full and not answered in time: it may be with WhatsApp already.
        Err(error @ ClientError::SendUnanswered(_)) => {
            if let ClientFrame::Send { client_id, .. } | ClientFrame::Retry { client_id, .. } =
                frame
            {
                host.send_not_confirmed(client_id);
            }
            Outcome::Lost(error)
        }
        Err(error) => {
            failed_send(host, frame, error.to_string());
            Outcome::Lost(error)
        }
    }
}

/// `frame` was asked for while disconnected: nothing is sent, a send (or retry) is shown as not
/// sent, the notice says so, and nothing retries it later.
pub fn not_connected(host: &mut impl Host, frame: &ClientFrame, client: Client) {
    failed_send(host, frame, NOT_CONNECTED.to_owned());
    let notice = match frame {
        ClientFrame::Send { .. } | ClientFrame::Retry { .. } => {
            format!("{NOT_CONNECTED}: not sent ({})", client.retry_hint)
        }
        _ => format!("{NOT_CONNECTED}; reconnecting…"),
    };
    host.request_failed(frame, notice);
}

/// Both connections were lost with `error`: the view says it is reconnecting.
pub fn lost(host: &mut impl Host, error: &ClientError) {
    host.set_connected(false);
    host.set_notice(format!("daemon connection lost: {error}; reconnecting…"));
}

/// Both connections are back: the view says so, and the frames to send again, in order, are
/// returned (stop at the first loss).
pub fn reconnected(host: &mut impl Host) -> Vec<ClientFrame> {
    host.set_connected(true);
    host.set_notice("reconnected to nunciod".to_owned());
    host.resync()
}

/// Open a downloaded media file with the desktop's opener: `open` on macOS, `xdg-open`
/// elsewhere, detached. It is reaped on a thread so it never lingers as a zombie.
pub fn open_externally(path: &Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::Snapshot;
    use crate::wire::{Chat, MessageState, SendState, Session};

    const ANA: &str = "15551230001@s.whatsapp.net";
    const X: Client = Client {
        program: "nuncio-desktop",
        retry_hint: "Retry once reconnected",
    };

    fn ana() -> Model {
        let chat: Chat = serde_json::from_value(serde_json::json!({
            "jid": ANA, "name": "Ana", "unread": 0, "archived": false, "pinned": false,
            "muted": false,
        }))
        .expect("a chat");
        Model::new(
            Snapshot {
                daemon: "test".to_owned(),
                linked: true,
                session: Session::Linked,
                unread: 0,
                chats: vec![chat],
                stop_reason: None,
            },
            "t",
        )
    }

    /// A model with one send of mine made, and that send's frame.
    fn sent(text: &str) -> (Model, ClientFrame) {
        let mut model = ana();
        text.chars().for_each(|c| model.composer_mut().insert(c));
        let frame = model.send().expect("a send");
        (model, frame)
    }

    fn state(model: &Model) -> (Option<MessageState>, bool, bool) {
        let message = &model.loaded()[0];
        (
            message.state.clone(),
            model.transcript().is_sending(message),
            model.transcript().is_unconfirmed(message),
        )
    }

    #[test]
    fn only_a_broken_connection_is_a_loss() {
        assert!(is_loss(&ClientError::Closed));
        assert!(is_loss(&ClientError::Timeout(Duration::from_secs(5))));
        assert!(!is_loss(&ClientError::Daemon {
            code: "internal".to_owned(),
            message: "oops".to_owned(),
        }));
    }

    #[test]
    fn a_not_confirmed_error_marks_the_send_not_confirmed_and_says_so() {
        let (mut model, frame) = sent("hi");
        let error = ClientError::Daemon {
            code: "not_confirmed".to_owned(),
            message: "timed out".to_owned(),
        };
        assert!(matches!(
            settle(&mut model, &frame, Err(error), X),
            Outcome::Done
        ));
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, true));
        assert_eq!(model.notice(), Some("daemon: not_confirmed: timed out"));
    }

    #[test]
    fn any_other_daemon_error_marks_the_send_not_sent() {
        let (mut model, frame) = sent("hi");
        let error = ClientError::Daemon {
            code: "not_linked".to_owned(),
            message: "no device".to_owned(),
        };
        settle(&mut model, &frame, Err(error), X);
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, false));
        assert_eq!(model.notice(), Some("daemon: not_linked: no device"));
    }

    #[test]
    fn a_broken_connection_marks_the_send_not_sent_and_is_returned_as_lost() {
        let (mut model, frame) = sent("hi");
        let outcome = settle(&mut model, &frame, Err(ClientError::Closed), X);
        assert!(
            matches!(outcome, Outcome::Lost(ClientError::Closed)),
            "{outcome:?}"
        );
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, false));
    }

    /// F3: a send (or retry) written in full whose reply did not come in time may be with
    /// WhatsApp: not confirmed, like the daemon's `not_confirmed`, and still a loss that
    /// reconnects; a retry of it keeps the same client_id, so the same WhatsApp id.
    #[test]
    fn an_unanswered_send_is_not_confirmed_and_lost_and_its_retry_keeps_the_client_id() {
        let (mut model, frame) = sent("hi");
        let ClientFrame::Send { client_id, .. } = &frame else {
            panic!("a send");
        };
        let client_id = client_id.clone();
        let error = ClientError::SendUnanswered(Duration::from_secs(5));
        assert!(is_loss(&error));
        let outcome = settle(&mut model, &frame, Err(error), X);
        assert!(
            matches!(outcome, Outcome::Lost(ClientError::SendUnanswered(_))),
            "{outcome:?}"
        );
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, true));
        let Outcome::Lost(error) = outcome else {
            unreachable!()
        };
        lost(&mut model, &error);
        assert_eq!(
            model.notice(),
            Some(
                "daemon connection lost: not confirmed: the daemon did not answer within 5s; \
                 it may have been delivered; reconnecting…"
            )
        );
        reconnected(&mut model);
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, true));
        model.select_older();
        let retry = model.retry_selected();
        assert_eq!(retry, Some(ClientFrame::retry(client_id.clone())));
        let retry = retry.expect("a retry");
        settle(
            &mut model,
            &retry,
            Err(ClientError::SendUnanswered(Duration::from_secs(5))),
            X,
        );
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, true));
    }

    /// A send that failed before (or while) it was written never reached the daemon: not sent,
    /// and a loss.
    #[test]
    fn a_send_that_failed_before_it_was_written_is_not_sent() {
        let errors = [
            ClientError::Connect {
                path: "/nonexistent/core.sock".into(),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
            ClientError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
            ClientError::Closed,
            ClientError::Timeout(Duration::from_secs(5)),
        ];
        for error in errors {
            let (mut model, frame) = sent("hi");
            let shown = format!("{error:?}");
            let outcome = settle(&mut model, &frame, Err(error), X);
            assert!(matches!(outcome, Outcome::Lost(_)), "{shown}: {outcome:?}");
            assert_eq!(
                state(&model),
                (Some(MessageState::NotSent), false, false),
                "{shown}"
            );
        }
    }

    /// The daemon's own answers keep their meaning: `not_confirmed` is not confirmed, any other
    /// `error` and a `send_failed` reply are not sent, for a send and for a retry alike.
    #[test]
    fn the_daemons_not_sent_and_not_confirmed_answers_keep_their_marks() {
        let daemon = |code: &str| ClientError::Daemon {
            code: code.to_owned(),
            message: "m".to_owned(),
        };
        for retrying in [false, true] {
            for (code, unconfirmed) in [
                ("not_confirmed", true),
                ("not_linked", false),
                ("internal", false),
            ] {
                let (mut model, mut frame) = sent("hi");
                if retrying {
                    let ClientFrame::Send { client_id, .. } = &frame else {
                        panic!("a send");
                    };
                    frame = ClientFrame::retry(client_id.clone());
                }
                let outcome = settle(&mut model, &frame, Err(daemon(code)), X);
                assert!(matches!(outcome, Outcome::Done), "{code}: {outcome:?}");
                assert_eq!(
                    state(&model),
                    (Some(MessageState::NotSent), false, unconfirmed),
                    "{code} (retry: {retrying})"
                );
            }
            let (mut model, frame) = sent("hi");
            let ClientFrame::Send { client_id, .. } = &frame else {
                panic!("a send");
            };
            let reply = DaemonFrame::SendFailed {
                v: PROTOCOL_VERSION,
                client_id: client_id.clone(),
                message: "not accepted".to_owned(),
            };
            settle(&mut model, &frame, Ok(reply), X);
            assert_eq!(state(&model), (Some(MessageState::NotSent), false, false));
        }
    }

    #[test]
    fn an_unknown_reply_says_the_client_may_be_too_old_and_the_send_is_not_confirmed() {
        let (mut model, frame) = sent("hi");
        let reply = DaemonFrame::Unknown {
            op: "sent_v9".to_owned(),
        };
        settle(&mut model, &frame, Ok(reply), X);
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, true));
        assert_eq!(
            model.notice(),
            Some("daemon: unknown reply `sent_v9`; this nuncio-desktop may be too old")
        );
    }

    #[test]
    fn a_reply_is_folded_in_and_what_it_asks_for_returned() {
        let (mut model, frame) = sent("hi");
        let reply = DaemonFrame::Sent {
            v: PROTOCOL_VERSION,
            client_id: "t-1".to_owned(),
            state: SendState::Sent,
            id: Some("M1".to_owned()),
            timestamp: Some("2026-10-20T10:00:00Z".to_owned()),
        };
        assert!(matches!(
            settle(&mut model, &frame, Ok(reply), X),
            Outcome::Done
        ));
        assert_eq!(model.loaded()[0].id, "M1");

        // Opening a chat with unread messages asks for a mark_read once they are shown.
        let mut model = ana();
        let unread = DaemonFrame::EventUnread {
            v: PROTOCOL_VERSION,
            unread: 2,
            jid: ANA.to_owned(),
            chat_unread: 2,
        };
        model.apply(&unread);
        let open = model.open_selected().expect("open");
        let messages = DaemonFrame::Messages {
            v: PROTOCOL_VERSION,
            jid: ANA.to_owned(),
            messages: Vec::new(),
        };
        let outcome = settle(&mut model, &open, Ok(messages), X);
        assert!(
            matches!(&outcome, Outcome::Effect(Effect::Request(ClientFrame::MarkRead { jid, .. })) if jid == ANA),
            "{outcome:?}"
        );
    }

    #[test]
    fn while_disconnected_a_send_is_not_sent_and_the_notice_says_how_to_retry() {
        let (mut model, frame) = sent("hi");
        not_connected(&mut model, &frame, X);
        assert_eq!(state(&model), (Some(MessageState::NotSent), false, false));
        assert_eq!(
            model.notice(),
            Some("not connected to nunciod: not sent (Retry once reconnected)")
        );
        not_connected(&mut model, &ClientFrame::status(), X);
        assert_eq!(
            model.notice(),
            Some("not connected to nunciod; reconnecting…")
        );
    }

    #[test]
    fn a_loss_and_a_reconnect_say_so_and_the_reconnect_resyncs() {
        let mut model = ana();
        lost(&mut model, &ClientError::Closed);
        assert!(!model.connected());
        assert!(
            model.notice().is_some_and(
                |n| n.starts_with("daemon connection lost: ") && n.ends_with("; reconnecting…")
            ),
            "{:?}",
            model.notice()
        );
        let frames = reconnected(&mut model);
        assert!(model.connected());
        assert_eq!(model.notice(), Some("reconnected to nunciod"));
        assert_eq!(frames, model.clone().resync());
        assert_eq!(frames[0], ClientFrame::status());
    }
}
