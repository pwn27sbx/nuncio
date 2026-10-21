use super::*;
use crate::controller::{Alert, Controller};
use crate::worker::Incoming;
use nuncio_client::wire::PROTOCOL_VERSION;
use nuncio_client::{Chat, ClientFrame, DaemonFrame, Message};

const ANA: &str = "15551230001@s.whatsapp.net";

/// A fresh directory under the system temp dir, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "nuncio-prefs-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("config.toml")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_missing_file_edits_the_defaults_and_the_first_change_writes_it() {
    let dir = TempDir::new("missing");
    let mut prefs = Preferences::load(Some(dir.file()));
    assert_eq!(prefs.problem(), None);
    assert_eq!(prefs.config(), &Config::default());
    assert!(!dir.file().exists(), "opening the settings writes nothing");
    assert_eq!(prefs.change(Change::ShowPreview(true)), Ok(true));
    let saved = Config::load(&dir.file()).expect("loads");
    assert!(saved.notifications.show_preview);
    assert_eq!(&saved, prefs.config());
}

#[test]
fn every_change_is_written_and_read_back() {
    let dir = TempDir::new("every");
    let mut prefs = Preferences::load(Some(dir.file()));
    for change in [
        Change::Notifications(false),
        Change::ShowPreview(true),
        Change::Background(false),
        Change::StartHidden(true),
        Change::TitleBar(TitleBar::Hidden),
        Change::ImageProtocol(Some(ImageProtocol::Sixel)),
    ] {
        assert_eq!(prefs.change(change), Ok(true), "{change:?}");
    }
    let saved = Config::load(&dir.file()).expect("loads");
    assert!(!saved.notifications.enabled);
    assert!(saved.notifications.show_preview);
    assert!(!saved.app.background);
    assert!(saved.app.start_hidden);
    assert_eq!(saved.app.title_bar, TitleBar::Hidden);
    assert_eq!(saved.ui.image_protocol, Some(ImageProtocol::Sixel));
    assert_eq!(
        Preferences::load(Some(dir.file())).config(),
        &saved,
        "opening the settings again shows what was saved"
    );
}

#[test]
fn a_hand_edit_to_another_key_made_while_the_window_is_open_is_kept() {
    let dir = TempDir::new("hand-edit");
    std::fs::write(dir.file(), "[app]\nstart_hidden = false\n").expect("write");
    let mut prefs = Preferences::load(Some(dir.file()));
    // The owner edits key B by hand while the settings are open.
    std::fs::write(dir.file(), "[app]\nstart_hidden = true\n").expect("write");
    // Key A changes in the window.
    assert_eq!(prefs.change(Change::ShowPreview(true)), Ok(true));
    let saved = Config::load(&dir.file()).expect("loads");
    assert!(saved.notifications.show_preview, "A is the new value");
    assert!(saved.app.start_hidden, "B keeps the hand edit");
    assert_eq!(prefs.config(), &saved, "the window shows both");
}

#[test]
fn a_change_to_the_same_value_writes_nothing() {
    let dir = TempDir::new("same");
    let mut prefs = Preferences::load(Some(dir.file()));
    assert_eq!(prefs.change(Change::Notifications(true)), Ok(false));
    assert!(!dir.file().exists());
}

#[test]
fn a_file_that_does_not_load_is_shown_and_never_overwritten() {
    let dir = TempDir::new("invalid");
    let text = "# my notes\n[notifications]\nshow_previews = true\n";
    std::fs::write(dir.file(), text).expect("write");
    let mut prefs = Preferences::load(Some(dir.file()));
    let problem = prefs.problem().expect("a problem").to_owned();
    assert!(problem.contains("show_previews"), "{problem}");
    assert!(problem.contains("will not overwrite"), "{problem}");
    let refused = prefs
        .change(Change::ShowPreview(true))
        .expect_err("refused");
    assert_eq!(refused, problem);
    assert_eq!(std::fs::read_to_string(dir.file()).expect("read"), text);
    assert!(!prefs.config().notifications.show_preview);
}

#[test]
fn a_file_broken_after_the_window_opened_is_not_overwritten_either() {
    let dir = TempDir::new("later");
    let mut prefs = Preferences::load(Some(dir.file()));
    let text = "[app]\nbackgroud = false\n";
    std::fs::write(dir.file(), text).expect("write");
    let refused = prefs
        .change(Change::StartHidden(true))
        .expect_err("refused");
    assert!(refused.contains("not overwriting"), "{refused}");
    assert_eq!(std::fs::read_to_string(dir.file()).expect("read"), text);
    assert!(!prefs.config().app.start_hidden, "the old value stays");
}

#[test]
fn a_write_that_fails_keeps_the_old_file_and_value_and_says_why() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("readonly");
    std::fs::write(dir.file(), "[app]\nstart_hidden = true\n").expect("write");
    let mut prefs = Preferences::load(Some(dir.file()));
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let result = prefs.change(Change::Background(false));
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let error = result.expect_err("cannot write");
    assert!(error.starts_with("settings not saved: "), "{error}");
    assert!(prefs.config().app.background, "the old value stays");
    assert_eq!(
        std::fs::read_to_string(dir.file()).expect("read"),
        "[app]\nstart_hidden = true\n"
    );
}

#[test]
fn with_no_path_nothing_can_be_changed() {
    let mut prefs = Preferences::load(None);
    assert!(prefs.problem().is_some_and(|p| p.contains("HOME")));
    assert!(prefs.change(Change::ShowPreview(true)).is_err());
}

#[test]
fn notifications_and_the_title_bar_apply_at_once_the_rest_later() {
    assert_eq!(Change::Notifications(false).takes(), Takes::Now);
    assert_eq!(Change::ShowPreview(true).takes(), Takes::Now);
    assert_eq!(Change::TitleBar(TitleBar::Shown).takes(), Takes::Now);
    assert_eq!(Change::Background(false).takes(), Takes::NextStart);
    assert_eq!(Change::StartHidden(true).takes(), Takes::NextStart);
    assert_eq!(Change::ImageProtocol(None).takes(), Takes::Tui);
}

#[test]
fn the_choices_list_every_value_once() {
    for choice in [TitleBar::Auto, TitleBar::Shown, TitleBar::Hidden] {
        assert_eq!(TITLE_BARS.iter().filter(|(c, _)| *c == choice).count(), 1);
    }
    assert_eq!(
        IMAGE_PROTOCOLS[0].0, None,
        "asking the terminal comes first"
    );
    for protocol in IMAGE_PROTOCOLS.iter().filter_map(|(p, _)| *p) {
        assert!(
            IMAGE_PROTOCOLS
                .iter()
                .any(|(_, name)| *name == protocol.as_str())
        );
    }
}

fn listed() -> Controller {
    let mut c = Controller::new("s");
    c.incoming(Incoming::Connected);
    c.incoming(Incoming::Reply {
        request: ClientFrame::chats(200),
        result: Ok(DaemonFrame::Chats {
            v: PROTOCOL_VERSION,
            chats: vec![Chat {
                jid: ANA.to_owned(),
                name: "Ana".to_owned(),
                unread: 0,
                archived: false,
                pinned: false,
                muted: false,
                aliases: Vec::new(),
                phone: None,
                last_message: None,
            }],
        }),
    });
    c
}

fn new_message(c: &mut Controller, id: &str) -> Vec<Alert> {
    c.incoming(Incoming::Event(DaemonFrame::EventMessage {
        v: PROTOCOL_VERSION,
        jid: ANA.to_owned(),
        message: Message {
            id: id.to_owned(),
            kind: "text".to_owned(),
            text: "see you at 5".to_owned(),
            from_me: false,
            timestamp: "2026-10-20T10:00:00Z".to_owned(),
            media: None,
            state: None,
            client_id: None,
            reply_to: None,
            reactions: Vec::new(),
            thumb: None,
            thumb_mime: None,
            width: None,
            height: None,
            animated: false,
            sender: None,
        },
    }));
    c.take_alerts()
}

#[test]
fn a_saved_change_reaches_the_running_controller_at_once() {
    let dir = TempDir::new("live");
    let mut prefs = Preferences::load(Some(dir.file()));
    let mut c = listed();
    assert!(
        matches!(new_message(&mut c, "M1").as_slice(), [Alert::Show(n)] if n.body == "New message")
    );

    prefs.change(Change::ShowPreview(true)).expect("saved");
    assert_eq!(
        c.reconfigure(prefs.config()),
        None,
        "the title bar is as it was"
    );
    assert!(
        matches!(new_message(&mut c, "M2").as_slice(), [Alert::Show(n)] if n.body == "see you at 5"),
        "the next notification has the text"
    );

    prefs.change(Change::Notifications(false)).expect("saved");
    c.reconfigure(prefs.config());
    assert!(
        new_message(&mut c, "M3").is_empty(),
        "no more notifications"
    );

    prefs
        .change(Change::TitleBar(TitleBar::Hidden))
        .expect("saved");
    assert_eq!(c.reconfigure(prefs.config()), Some(TitleBar::Hidden));
    assert_eq!(c.reconfigure(prefs.config()), None, "only when it changes");
    assert_eq!(c.config(), prefs.config());
}
