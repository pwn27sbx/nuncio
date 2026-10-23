//! Q6: the header says why WhatsApp stopped the session, in fixed text mapped from
//! `stop_reason`, and nothing more while the session runs normally.
use nuncio_client::wire::{PROTOCOL_VERSION, StopReason};
use nuncio_client::{DaemonFrame, Palette, Session, Snapshot};
use nuncio_tui::{App, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn header(app: &mut App) -> String {
    let width = 120;
    let mut terminal = Terminal::new(TestBackend::new(width, 10)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    let buffer = terminal.backend().buffer();
    (0..width)
        .map(|x| buffer.cell((x, 0)).map_or("", |c| c.symbol()).to_owned())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn app(session: Session, stop_reason: Option<StopReason>) -> App {
    App::new(
        Snapshot {
            daemon: "nunciod/0.1.0".to_owned(),
            linked: true,
            session,
            unread: 2,
            chats: Vec::new(),
            stop_reason,
        },
        Source::Daemon,
        Palette::default(),
    )
}

#[test]
fn the_header_names_the_stop_reason() {
    let mut a = app(Session::Linked, None);
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::TemporaryBan),
    });
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · offline · WhatsApp temporarily banned this account · 2 unread"
    );

    let mut a = app(Session::Unlinked, Some(StopReason::LoggedOut));
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · unlinked · WhatsApp logged this device out · 2 unread"
    );
}

#[test]
fn the_header_without_a_stop_is_unchanged() {
    let mut a = app(Session::Linked, None);
    assert_eq!(header(&mut a), "nuncio · nunciod/0.1.0 · linked · 2 unread");
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · offline · 2 unread"
    );
}

/// D4: a stored device nunciod keeps closed shows its fixed text, and a later frame without a
/// stop_reason (the device opened) clears it.
#[test]
fn the_header_names_a_closed_device_and_clears_it() {
    let mut a = app(Session::Offline, Some(StopReason::KeyringLocked));
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · offline · keyring locked: unlock it to connect · 2 unread"
    );
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::DeviceUnreadable),
    });
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · offline · the stored key does not open this device: see SECURITY.md · 2 unread"
    );
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert_eq!(
        header(&mut a),
        "nuncio · nunciod/0.1.0 · offline · 2 unread"
    );
}
