//! Q6: the footer's state slot says why WhatsApp stopped the session, in fixed text mapped from
//! `stop_reason`, and nothing more while the session runs normally.
use nuncio_client::wire::{PROTOCOL_VERSION, StopReason};
use nuncio_client::{DaemonFrame, Palette, Session, Snapshot};
use nuncio_tui::{App, Source};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn footer(app: &mut App) -> String {
    footer_at(app, 120)
}

fn footer_at(app: &mut App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 10)).expect("a test terminal");
    terminal
        .draw(|frame| app.draw(frame))
        .expect("the app draws without panicking");
    let buffer = terminal.backend().buffer();
    (0..width)
        .map(|x| buffer.cell((x, 9)).map_or("", |c| c.symbol()).to_owned())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn app(session: Session, stop_reason: Option<StopReason>) -> App {
    App::new(
        Snapshot {
            daemon: "nunciod/0.1.1".to_owned(),
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
fn the_footer_names_the_stop_reason() {
    let mut a = app(Session::Linked, None);
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::TemporaryBan),
    });
    assert!(
        footer(&mut a).ends_with("offline · WhatsApp temporarily banned this account"),
        "{}",
        footer(&mut a)
    );

    let mut a = app(Session::Unlinked, Some(StopReason::LoggedOut));
    assert!(
        footer(&mut a).ends_with("not linked · WhatsApp logged this device out"),
        "{}",
        footer(&mut a)
    );
}

#[test]
fn the_footer_without_a_stop_is_unchanged() {
    let mut a = app(Session::Linked, None);
    assert!(footer(&mut a).ends_with("linked"), "{}", footer(&mut a));
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert!(footer(&mut a).ends_with("offline"), "{}", footer(&mut a));
}

/// D4: a stored device nunciod keeps closed shows its fixed text, and a later frame without a
/// stop_reason (the device opened) clears it.
#[test]
fn the_footer_names_a_closed_device_and_clears_it() {
    let mut a = app(Session::Offline, Some(StopReason::KeyringLocked));
    assert!(
        footer(&mut a).ends_with("offline · keyring locked: unlock it to connect"),
        "{}",
        footer(&mut a)
    );
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: Some(StopReason::DeviceUnreadable),
    });
    assert!(
        footer(&mut a)
            .ends_with("offline · the stored key does not open this device: see SECURITY.md"),
        "{}",
        footer(&mut a)
    );
    a.apply(&DaemonFrame::EventConnection {
        v: PROTOCOL_VERSION,
        session: Session::Offline,
        stop_reason: None,
    });
    assert!(footer(&mut a).ends_with("offline"), "{}", footer(&mut a));
}

#[test]
fn the_session_state_keeps_its_room_when_the_key_hints_would_take_it() {
    // A long stop reason at a narrow width: the key hints give way, the state does not.
    let mut a = app(Session::Offline, Some(StopReason::DeviceUnreadable));
    let footer = footer_at(&mut a, 80);
    assert!(
        footer.ends_with("offline · the stored key does not open this device: see SECURITY.md"),
        "{footer:?}"
    );
    assert!(
        !footer.contains("? all keys"),
        "the hints gave way: {footer:?}"
    );
}
