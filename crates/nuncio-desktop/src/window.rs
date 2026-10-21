//! The window: an `AdwNavigationSplitView` with the chat list in the sidebar and the open
//! conversation (transcript, search, composer) as the content, laid out after Sealwax
//! (`PixDevsApps/sealwax`, MIT: its DESIGN.md and window module were read for the design; no code
//! was copied): flat theme surfaces, one pane at a time on a narrow tiled window, no title bar
//! controls on a tiling desktop, toasts rather than dialogs, and every action reachable from the
//! keyboard.
//!
//! The widgets hold no state of their own beyond the draft's text: every action goes through the
//! [`Controller`], and [`Ui::render`] redraws what [`crate::present`] makes of the model, changing
//! only the rows that changed so the focus and the scroll stay where they were.
//!
//! Keyboard (Tab moves between the chat list, the conversation and the composer):
//!
//! - chat list: Up/Down move, Enter opens the chat (and only opening marks it read);
//! - conversation: Up/Down move between messages; `r` replies, `e` reacts, Shift+R retries a send
//!   that was not sent, `o` opens a media file (a picture's full file is shown in its bubble),
//!   Shift+O opens it with the desktop's opener, Enter shows a picture in full (on any other
//!   message it opens the menu), Menu or Shift+F10 opens the message's menu, Escape goes to the
//!   composer;
//! - composer: Enter sends, Shift+Enter starts a new line, Escape cancels the reply;
//! - Ctrl+F searches the open chat; Escape closes the search; Ctrl+Q quits (the app's
//!   `app.quit`, also in the menu);
//! - F1 or Ctrl+K opens the command palette ([`crate::palette`]): type to filter, Up/Down choose,
//!   Enter runs, Escape closes; Ctrl+, opens the settings ([`crate::preferences`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;
use gtk::accessible::Property;
use gtk::{gdk, gio, glib};
use nuncio_client::driver::open_externally;
use nuncio_client::model::REACTIONS;

use crate::ROOT_CLASS;
use crate::controller::{Alert, Command, Controller, OPEN_CHAT_ACTION, notification_id};
use crate::lifecycle::{Close, Lifecycle};
use crate::palette::{self, Step};
use crate::pictures::{self, Cache, Decoded, Decoder, Variant};
use crate::preferences::{Change, IMAGE_PROTOCOLS, Preferences, TITLE_BARS, Takes};
use crate::present::{self, Bubble, ChatRow, PictureView};
use crate::worker::{Backlog, EVENT_BACKLOG, Incoming, Settings, Worker};
use nuncio_client::TitleBar;
use nuncio_client::config::App;

/// Below this width the window shows one pane at a time, with back navigation.
const NARROW: &str = "max-width: 600sp";

/// The window and the state behind it. Cheap to clone. The window itself keeps the state alive
/// until it is closed, so a caller may drop every `Ui` once the window is shown.
#[derive(Clone)]
pub struct Ui(Rc<Inner>);

/// A [`Ui`] that does not keep the window's state alive.
#[derive(Clone)]
pub struct WeakUi(Weak<Inner>);

impl WeakUi {
    /// The window's state, while the window exists.
    pub fn upgrade(&self) -> Option<Ui> {
        self.0.upgrade().map(Ui)
    }
}

struct Inner {
    controller: RefCell<Controller>,
    worker: RefCell<Option<Worker>>,
    /// Commands made with no worker attached (the smoke test reads them).
    undelivered: RefCell<Vec<Command>>,
    /// Notifications made with no `GApplication` attached (the smoke test reads them).
    undelivered_alerts: RefCell<Vec<Alert>>,
    /// What closing the window does (`[app] background`).
    lifecycle: RefCell<Lifecycle>,
    /// Quit was chosen: closing the window really closes it.
    quitting: Cell<bool>,
    offset_minutes: i32,
    window: adw::ApplicationWindow,
    sidebar_header: adw::HeaderBar,
    content_header: adw::HeaderBar,
    /// The window's own shortcuts (Ctrl+F, F1, Ctrl+K, Ctrl+,).
    shortcuts: gtk::ShortcutController,
    /// The command palette while it is open.
    palette: RefCell<Option<Rc<PaletteView>>>,
    /// The configuration file the settings edit (`None`: there is none).
    config_path: RefCell<Option<PathBuf>>,
    split: adw::NavigationSplitView,
    toasts: adw::ToastOverlay,
    sidebar_title: adw::WindowTitle,
    content_title: adw::WindowTitle,
    content_page: adw::NavigationPage,
    chats: gtk::ListBox,
    transcript: gtk::ListBox,
    transcript_scroll: gtk::ScrolledWindow,
    /// Keep the conversation scrolled to the newest message as it grows.
    stick: Cell<bool>,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    search_status: gtk::Label,
    results: gtk::ListBox,
    results_scroll: gtk::ScrolledWindow,
    blocked: adw::Banner,
    reply_bar: gtk::Box,
    reply_label: gtk::Label,
    composer: gtk::TextView,
    /// What is drawn now, to change only what changed.
    shown_chats: RefCell<Vec<ChatRow>>,
    shown_bubbles: RefCell<Vec<Bubble>>,
    shown_results: RefCell<Vec<String>>,
    highlighted: Cell<Option<usize>>,
    /// The model's search was open at the last render.
    searching: Cell<bool>,
    /// The draft is being set from code, not typed.
    setting_draft: Cell<bool>,
    /// The decoded pictures, as textures ([`crate::pictures`]).
    pictures: RefCell<Cache<gdk::Texture>>,
    /// The threads that decode pictures; what they decode comes back on the main loop.
    decoder: Decoder,
    /// The picture widgets drawn now, by chat and message id, to put a texture in once decoded.
    live: RefCell<HashMap<(String, String), Vec<Slot>>>,
    /// The bubbles the render in progress (re)built: their pictures are asked for.
    built: RefCell<Vec<Bubble>>,
    /// The last toast shown (tests).
    last_toast: RefCell<Option<String>>,
}

/// A picture widget drawn now.
struct Slot {
    picture: glib::WeakRef<gtk::Picture>,
    sticker: bool,
    /// Its box came from the size the message states; else from the picture once decoded.
    sized: bool,
}

impl Ui {
    /// Build the window (not shown yet). `title_bar` false leaves out the window controls, as
    /// on a tiling desktop ([`present::shows_title_bar`]).
    pub fn new(
        app: Option<&adw::Application>,
        controller: Controller,
        offset_minutes: i32,
        title_bar: bool,
    ) -> Self {
        let window = adw::ApplicationWindow::builder()
            .title("nuncio")
            .default_width(960)
            .default_height(640)
            .build();
        if let Some(app) = app {
            window.set_application(Some(app));
        }
        if !title_bar {
            window.add_css_class("nuncio-no-title-bar");
        }

        // Sidebar: the chat list.
        let sidebar_title = adw::WindowTitle::new("nuncio", "");
        let sidebar_header = header_bar(title_bar);
        sidebar_header.set_title_widget(Some(&sidebar_title));
        // The app menu: Quit really quits, also in background mode (Ctrl+Q).
        let menu = gio::Menu::new();
        menu.append(Some("Command palette"), Some("win.command-palette"));
        menu.append(Some("Settings"), Some("win.settings"));
        menu.append(Some("Quit"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .menu_model(&menu)
            .build();
        menu_button.update_property(&[Property::Label("Menu")]);
        sidebar_header.pack_end(&menu_button);
        let chats = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        chats.add_css_class("navigation-sidebar");
        chats.update_property(&[Property::Label("Chats")]);
        let chats_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&chats)
            .build();
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        sidebar_view.set_content(Some(&chats_scroll));
        let sidebar_page = adw::NavigationPage::builder()
            .title("Chats")
            .tag("chats")
            .child(&sidebar_view)
            .build();

        // Content: the conversation.
        let content_title = adw::WindowTitle::new("No chat", "");
        let content_header = header_bar(title_bar);
        content_header.set_title_widget(Some(&content_title));
        let search_button = gtk::ToggleButton::builder()
            .icon_name("edit-find-symbolic")
            .tooltip_text("Search in this chat (Ctrl+F)")
            .build();
        search_button.update_property(&[Property::Label("Search in this chat")]);
        content_header.pack_end(&search_button);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search this chat")
            .hexpand(true)
            .build();
        search_entry.update_property(&[Property::Label("Search this chat")]);
        let search_bar = gtk::SearchBar::builder().child(&search_entry).build();
        search_bar.connect_entry(&search_entry);
        search_button
            .bind_property("active", &search_bar, "search-mode-enabled")
            .bidirectional()
            .build();

        let search_status = gtk::Label::builder().xalign(0.0).visible(false).build();
        search_status.add_css_class("dim-label");
        search_status.set_margin_start(12);
        let results = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        results.update_property(&[Property::Label("Search results")]);
        results.add_css_class("navigation-sidebar");
        let results_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(200)
            .visible(false)
            .child(&results)
            .build();

        let transcript = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(false)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        transcript.add_css_class("nuncio-transcript");
        transcript.update_property(&[Property::Label("Messages")]);
        let transcript_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&transcript)
            .build();

        let blocked = adw::Banner::new("");
        let reply_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        reply_label.add_css_class("nuncio-quote");
        let cancel_reply = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Cancel the reply (Escape)")
            .build();
        cancel_reply.add_css_class("flat");
        cancel_reply.update_property(&[Property::Label("Cancel the reply")]);
        let reply_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        reply_bar.append(&reply_label);
        reply_bar.append(&cancel_reply);
        reply_bar.set_visible(false);
        reply_bar.set_margin_start(12);
        reply_bar.set_margin_end(6);

        let composer = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(10)
            .right_margin(10)
            .build();
        composer.add_css_class("nuncio-composer");
        composer.update_property(&[Property::Label("Message")]);
        let composer_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(160)
            .hexpand(true)
            .child(&composer)
            .build();
        let send_button = gtk::Button::builder()
            .label("Send")
            .tooltip_text("Send (Enter; Shift+Enter starts a new line)")
            .valign(gtk::Align::End)
            .build();
        send_button.add_css_class("suggested-action");
        let composer_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        composer_row.set_margin_start(6);
        composer_row.set_margin_end(6);
        composer_row.set_margin_top(6);
        composer_row.set_margin_bottom(6);
        composer_row.append(&composer_scroll);
        composer_row.append(&send_button);
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom.append(&blocked);
        bottom.append(&reply_bar);
        bottom.append(&composer_row);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&search_status);
        content.append(&results_scroll);
        content.append(&transcript_scroll);
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_header);
        content_view.add_top_bar(&search_bar);
        content_view.set_content(Some(&content));
        content_view.add_bottom_bar(&bottom);
        let content_page = adw::NavigationPage::builder()
            .title("Conversation")
            .tag("conversation")
            .child(&content_view)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .build();
        split.add_css_class(ROOT_CLASS);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));
        if let Ok(condition) = adw::BreakpointCondition::parse(NARROW) {
            let narrow = adw::Breakpoint::new(condition);
            narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
            window.add_breakpoint(narrow);
        }

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_scope(gtk::ShortcutScope::Global);
        window.add_controller(shortcuts.clone());

        let (post_picture, mut decoded) = futures_channel::mpsc::unbounded::<Decoded>();
        let decoder = Decoder::spawn(move |done| {
            // The receiver only goes with the main loop, at exit.
            let _ = post_picture.unbounded_send(done);
        });
        let ui = Self(Rc::new(Inner {
            controller: RefCell::new(controller),
            worker: RefCell::new(None),
            undelivered: RefCell::new(Vec::new()),
            undelivered_alerts: RefCell::new(Vec::new()),
            // Until told otherwise, closing the window closes it (as before background mode).
            lifecycle: RefCell::new(Lifecycle::new(&App {
                background: false,
                ..App::default()
            })),
            quitting: Cell::new(false),
            offset_minutes,
            window,
            sidebar_header,
            content_header,
            shortcuts,
            palette: RefCell::new(None),
            config_path: RefCell::new(None),
            split,
            toasts,
            sidebar_title,
            content_title,
            content_page,
            chats,
            transcript,
            transcript_scroll,
            stick: Cell::new(true),
            search_bar,
            search_entry,
            search_status,
            results,
            results_scroll,
            blocked,
            reply_bar,
            reply_label,
            composer,
            shown_chats: RefCell::new(Vec::new()),
            shown_bubbles: RefCell::new(Vec::new()),
            shown_results: RefCell::new(Vec::new()),
            highlighted: Cell::new(None),
            searching: Cell::new(false),
            setting_draft: Cell::new(false),
            pictures: RefCell::new(Cache::default()),
            decoder,
            live: RefCell::new(HashMap::new()),
            built: RefCell::new(Vec::new()),
            last_toast: RefCell::new(None),
        }));
        ui.connect(&send_button, &cancel_reply);
        let weak = ui.weak();
        glib::spawn_future_local(async move {
            while let Ok(done) = decoded.recv().await {
                match weak.upgrade() {
                    Some(inner) => Ui(inner).picture_decoded(done),
                    None => return,
                }
            }
        });
        // Every signal handler holds the state weakly; the window holds it strongly, so the state
        // (and its worker) lives exactly as long as the window, whoever else drops their `Ui`.
        // Closing the window lets the state go (and with it the worker's threads). Not on
        // `destroy`: that only comes once the last reference to the window goes, and the state
        // holds one. In background mode closing only hides the window: the state, the worker and
        // the notifications stay until Quit ([`Ui::quit`]).
        let owner = RefCell::new(Some(ui.clone()));
        ui.0.window.connect_close_request(move |window| {
            let hide = owner.borrow().as_ref().is_some_and(|ui| {
                !ui.0.quitting.get() && ui.0.lifecycle.borrow().close() == Close::Hide
            });
            if hide {
                window.set_visible(false);
                return glib::Propagation::Stop;
            }
            owner.borrow_mut().take();
            glib::Propagation::Proceed
        });
        ui.render();
        ui
    }

    /// A handle that does not keep the window's state alive.
    pub fn downgrade(&self) -> WeakUi {
        WeakUi(self.weak())
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.0.window
    }

    /// Follow `lifecycle` when the window is closed (`[app] background`).
    pub fn set_lifecycle(&self, lifecycle: Lifecycle) {
        *self.0.lifecycle.borrow_mut() = lifecycle;
    }

    /// Close the window for good, also in background mode: the state and the worker go with it.
    pub fn quit(&self) {
        self.0.quitting.set(true);
        // `close` only acts on a realized window, and a window started hidden never was.
        WidgetExt::realize(&self.0.window);
        self.0.window.close();
    }

    /// Whether the daemon worker is running (tests).
    pub fn worker_running(&self) -> bool {
        self.0.worker.borrow().is_some()
    }

    /// The notifications made while no `GApplication` was attached, oldest first (tests).
    pub fn take_undelivered_alerts(&self) -> Vec<Alert> {
        std::mem::take(&mut self.0.undelivered_alerts.borrow_mut())
    }

    /// The `open-chat` action a notification's `default` action names ([`OPEN_CHAT_ACTION`]),
    /// for the application: its string target is the chat JID to open
    /// ([`Controller::open_jid`]); `show` then brings the window up.
    pub fn open_chat_action(&self, show: impl Fn(&Ui) + 'static) -> gio::SimpleAction {
        let name = OPEN_CHAT_ACTION
            .strip_prefix("app.")
            .unwrap_or(OPEN_CHAT_ACTION);
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let open = self.with(move |ui, jid: String| {
            ui.open_jid(&jid);
            show(ui);
        });
        action.connect_activate(move |_, target| {
            if let Some(jid) = target.and_then(|t| t.str()) {
                open(jid.to_owned());
            }
        });
        action
    }

    /// Open the chat `jid` (a clicked notification), as [`Controller::open_jid`] does.
    pub fn open_jid(&self, jid: &str) {
        let commands = self.0.controller.borrow_mut().open_jid(jid);
        self.show_conversation(commands);
    }

    /// Open a `whatsapp:` / `wa.me` link, as [`Controller::open_link`] does: its chat, its text in
    /// the composer, never a send.
    pub fn open_link(&self, link: &str) {
        let commands = self.0.controller.borrow_mut().open_link(link);
        self.show_conversation(commands);
    }

    fn show_conversation(&self, commands: Vec<Command>) {
        if self.0.controller.borrow().model().is_open() {
            self.0.split.set_show_content(true);
        }
        self.run(commands);
        self.0.composer.grab_focus();
    }

    /// Start the daemon worker ([`Worker`]) and fold what it posts in on the GLib main loop, through
    /// a channel the main loop awaits; every command from now on goes to it. At most
    /// [`EVENT_BACKLOG`] events wait in the channel ([`Backlog`]); past that they are dropped and
    /// the model resyncs.
    pub fn start_worker(&self, settings: Settings) {
        let backlog = Arc::new(Backlog::new(EVENT_BACKLOG));
        let (post, mut posted) = futures_channel::mpsc::unbounded();
        let admit = Arc::clone(&backlog);
        let worker = Worker::spawn(settings, move |incoming| {
            if let Some(incoming) = admit.admit(incoming) {
                // The receiver only goes with the main loop, at exit.
                let _ = post.unbounded_send(incoming);
            }
        });
        *self.0.worker.borrow_mut() = Some(worker);
        let weak = self.weak();
        glib::spawn_future_local(async move {
            while let Ok(incoming) = posted.recv().await {
                backlog.delivered(&incoming);
                match weak.upgrade() {
                    Some(inner) => Ui(inner).incoming(incoming),
                    None => return,
                }
            }
        });
    }

    /// Fold what the worker posted in, and redraw.
    pub fn incoming(&self, incoming: Incoming) {
        let commands = self.0.controller.borrow_mut().incoming(incoming);
        self.run(commands);
    }

    /// The commands made while no worker was attached, oldest first (tests).
    pub fn take_undelivered(&self) -> Vec<Command> {
        std::mem::take(&mut self.0.undelivered.borrow_mut())
    }

    /// The model behind the window (tests).
    pub fn controller(&self) -> std::cell::Ref<'_, Controller> {
        self.0.controller.borrow()
    }

    /// How many chat rows and message bubbles are drawn (tests).
    pub fn drawn(&self) -> (usize, usize) {
        (count_rows(&self.0.chats), count_rows(&self.0.transcript))
    }

    /// The conversation header's title (tests).
    pub fn conversation_title(&self) -> String {
        self.0.content_title.title().to_string()
    }

    /// The accessible role of the chat row and the bubble at these positions (tests).
    pub fn roles(
        &self,
        chat: usize,
        bubble: usize,
    ) -> (Option<gtk::AccessibleRole>, Option<gtk::AccessibleRole>) {
        let role = |list: &gtk::ListBox, i: usize| {
            list.row_at_index(i32::try_from(i).ok()?)
                .map(|row| row.accessible_role())
        };
        (role(&self.0.chats, chat), role(&self.0.transcript, bubble))
    }

    /// Open the chat at list position `index`, as a click or Enter on its row does.
    pub fn open_chat(&self, index: usize) {
        let commands = self.0.controller.borrow_mut().open_chat(index);
        self.0.split.set_show_content(true);
        self.run(commands);
        self.0.composer.grab_focus();
    }

    /// Put `text` in the composer, as typing does (tests).
    pub fn type_draft(&self, text: &str) {
        self.0.composer.buffer().set_text(text);
    }

    /// The composer's text (tests).
    pub fn draft(&self) -> String {
        let buffer = self.0.composer.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    /// Send the draft, as Enter in the composer or the Send button does.
    pub fn send(&self) {
        let (sent, commands) = self.0.controller.borrow_mut().send();
        if sent {
            self.set_draft_text("");
        }
        self.0.stick.set(true);
        self.run(commands);
    }

    /// The configuration file the settings edit ([`nuncio_client::Config::default_path`] in the
    /// app; a temporary file in the tests).
    pub fn set_config_path(&self, path: Option<PathBuf>) {
        *self.0.config_path.borrow_mut() = path;
    }

    /// Follow a configuration saved in the settings: notifications at once, and the header bars
    /// when the title bar choice changed.
    fn reconfigure(&self, config: &nuncio_client::Config) {
        let title_bar = self.0.controller.borrow_mut().reconfigure(config);
        if let Some(choice) = title_bar {
            self.set_title_bar(title_bar_here(choice));
        }
    }

    /// Show or hide the window controls in both header bars.
    pub fn set_title_bar(&self, shown: bool) {
        for header in [&self.0.sidebar_header, &self.0.content_header] {
            header.set_show_start_title_buttons(shown);
            header.set_show_end_title_buttons(shown);
        }
        if shown {
            self.0.window.remove_css_class("nuncio-no-title-bar");
        } else {
            self.0.window.add_css_class("nuncio-no-title-bar");
        }
    }

    /// Whether the header bars show the window controls (tests).
    pub fn title_bar_shown(&self) -> bool {
        self.0.sidebar_header.shows_end_title_buttons()
    }

    /// Activate the window shortcut with this trigger (`"F1"`, `"<Control>k"`), as pressing it
    /// does; false when there is none (tests).
    pub fn press(&self, trigger: &str) -> bool {
        let shortcuts = &self.0.shortcuts;
        let found = (0..shortcuts.n_items())
            .filter_map(|i| shortcuts.item(i)?.downcast::<gtk::Shortcut>().ok())
            .find(|shortcut| {
                shortcut
                    .trigger()
                    .is_some_and(|t| t.to_str().as_str() == trigger)
            });
        let Some(action) = found.and_then(|shortcut| shortcut.action()) else {
            return false;
        };
        action.activate(gtk::ShortcutActionFlags::empty(), &self.0.window, None)
    }

    /// Open the command palette (F1, Ctrl+K): every command that applies now, filtered as you
    /// type. The focused message, if any, is the one its message commands act on.
    pub fn open_palette(&self) {
        if let Some(view) = self.0.palette.borrow().as_ref() {
            view.entry.grab_focus();
            return;
        }
        // Only a message the user can see is chosen: the focused bubble.
        let message = self.focused_message().map(|(index, _)| index);
        let (entries, conversation) = {
            let controller = self.0.controller.borrow();
            let model = controller.model();
            (palette::entries(model, message), open_conversation(model))
        };
        let view = Rc::new(PaletteView::new(entries, conversation));
        view.popover.set_parent(&self.0.toasts);
        let width = self.0.toasts.width();
        view.popover
            .set_pointing_to(Some(&gdk::Rectangle::new(width / 2, 12, 1, 1)));

        view.entry.connect_changed({
            let view = Rc::downgrade(&view);
            move |entry| {
                if let Some(view) = view.upgrade() {
                    view.filter(&entry.text());
                }
            }
        });
        let run_selected = Rc::new(self.with(|ui, (): ()| ui.palette_enter()));
        view.entry.connect_activate({
            let run = Rc::clone(&run_selected);
            move |_| run(())
        });
        view.entry.connect_stop_search({
            let close = self.with(|ui, (): ()| ui.close_palette());
            move |_| close(())
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let view = Rc::downgrade(&view);
            move |_, key, _, _| {
                let step = match key {
                    gdk::Key::Down | gdk::Key::KP_Down => 1,
                    gdk::Key::Up | gdk::Key::KP_Up => -1,
                    _ => return glib::Propagation::Proceed,
                };
                if let Some(view) = view.upgrade() {
                    view.move_selection(step);
                }
                glib::Propagation::Stop
            }
        });
        view.entry.add_controller(keys);
        view.list.connect_row_activated({
            let run = self.with(|ui, position: usize| ui.palette_run(position));
            move |_, row| run(usize::try_from(row.index()).unwrap_or(usize::MAX))
        });
        view.popover.connect_closed({
            let weak = self.weak();
            move |popover| {
                if let Some(inner) = weak.upgrade() {
                    let mut open = inner.palette.borrow_mut();
                    if open.as_ref().is_some_and(|view| &view.popover == popover) {
                        open.take();
                    }
                }
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            }
        });

        view.filter("");
        *self.0.palette.borrow_mut() = Some(Rc::clone(&view));
        view.popover.popup();
        view.entry.grab_focus();
    }

    /// Whether the command palette is open (tests).
    pub fn palette_open(&self) -> bool {
        self.0.palette.borrow().is_some()
    }

    /// Close the command palette, if it is open.
    fn close_palette(&self) {
        let Some(view) = self.0.palette.borrow_mut().take() else {
            return;
        };
        view.popover.popdown();
        // A palette never mapped (a hidden window) is not `closed`: let it go here too.
        let popover = view.popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    }

    /// Type `query` into the open palette's search, replacing what was there.
    pub fn palette_type(&self, query: &str) {
        let entry = self.0.palette.borrow().as_ref().map(|v| v.entry.clone());
        if let Some(entry) = entry {
            entry.set_text(query);
        }
    }

    /// The titles the open palette lists now, in order (tests).
    pub fn palette_titles(&self) -> Vec<String> {
        self.0
            .palette
            .borrow()
            .as_ref()
            .map(|view| {
                view.shown
                    .borrow()
                    .iter()
                    .map(|&i| view.entries[i].title.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run the palette's selected command, as Enter in its search does.
    pub fn palette_enter(&self) {
        let position = self.0.palette.borrow().as_ref().map(|view| {
            view.list
                .selected_row()
                .and_then(|row| usize::try_from(row.index()).ok())
                .unwrap_or(0)
        });
        if let Some(position) = position {
            self.palette_run(position);
        }
    }

    /// Close the palette, as Escape does.
    pub fn palette_escape(&self) {
        let entry = self.0.palette.borrow().as_ref().map(|v| v.entry.clone());
        if let Some(entry) = entry {
            entry.emit_stop_search();
        }
    }

    /// Close the palette and run the command listed at `position`, once it is closed.
    fn palette_run(&self, position: usize) {
        let Some(view) = self.0.palette.borrow().clone() else {
            return;
        };
        let action = view
            .shown
            .borrow()
            .get(position)
            .map(|&i| view.entries[i].action.clone());
        let Some(action) = action else {
            return;
        };
        self.close_palette();
        let run = self.with(move |ui, (): ()| ui.palette_step(&action));
        glib::idle_add_local_once(move || run(()));
    }

    fn palette_step(&self, action: &palette::Action) {
        let step = palette::run(&mut self.0.controller.borrow_mut(), action);
        match step {
            Step::Opened(commands) => self.show_conversation(commands),
            Step::Commands(commands) => self.run(commands),
            Step::Reply(index) => self.reply(index),
            Step::React(index) => {
                if let Some(row) = i32::try_from(index)
                    .ok()
                    .and_then(|i| self.0.transcript.row_at_index(i))
                {
                    row.grab_focus();
                    self.reaction_picker(&row, index);
                }
            }
            Step::Search => self.start_search(),
            Step::Settings => self.open_settings(),
            Step::Quit => {
                // No application in the tests: nothing to quit.
                let _ = WidgetExt::activate_action(&self.0.window, "app.quit", None);
            }
            Step::Gone => {
                let toast = adw::Toast::new("that message is no longer shown; nothing was done");
                toast.set_use_markup(false);
                toast.set_timeout(4);
                self.0.toasts.add_toast(toast);
            }
        }
    }

    /// Show the settings window over the configuration file.
    pub fn open_settings(&self) {
        self.settings_view().window.present();
    }

    /// Build the settings window (not shown): the file is read now. A file that does not load is
    /// shown as the problem, with every setting insensitive, and is never overwritten.
    pub fn settings_view(&self) -> SettingsView {
        let prefs = Rc::new(RefCell::new(Preferences::load(
            self.0.config_path.borrow().clone(),
        )));
        let config = prefs.borrow().config().clone();
        let window = adw::PreferencesWindow::builder()
            .title("Settings")
            .modal(true)
            .transient_for(&self.0.window)
            .search_enabled(false)
            .default_width(560)
            .default_height(620)
            .build();
        window.add_css_class(ROOT_CLASS);
        let page = adw::PreferencesPage::new();
        let problem = prefs.borrow().problem().map(str::to_owned);
        if let Some(problem) = &problem {
            let group = adw::PreferencesGroup::builder()
                .title("The configuration file cannot be changed here")
                .build();
            let text = plain_label(problem);
            text.add_css_class("nuncio-error");
            group.add(&text);
            page.add(&group);
        }

        let switch = |title: &str, subtitle: &str, active: bool| {
            adw::SwitchRow::builder()
                .title(title)
                .subtitle(subtitle)
                .use_markup(false)
                .active(active)
                .build()
        };
        let combo = |title: &str, subtitle: &str, names: &[&str], selected: usize| {
            adw::ComboRow::builder()
                .title(title)
                .subtitle(subtitle)
                .use_markup(false)
                .model(&gtk::StringList::new(names))
                .selected(u32::try_from(selected).unwrap_or(0))
                .build()
        };
        let notifications = switch(
            "Notifications",
            "A desktop notification for each new message",
            config.notifications.enabled,
        );
        let show_preview = switch(
            "Show the message text",
            "Off: a notification says only New message, Photo, Sticker…",
            config.notifications.show_preview,
        );
        let title_bar = combo(
            "Title bar",
            "The window controls in the header bars",
            &TITLE_BARS.map(|(_, name)| name),
            TITLE_BARS
                .iter()
                .position(|(c, _)| *c == config.app.title_bar)
                .unwrap_or(0),
        );
        let background = switch(
            "Run in the background",
            "Closing the window keeps nuncio connected and notifying until Quit. Takes effect on next start",
            config.app.background,
        );
        let start_hidden = switch(
            "Start hidden",
            "Start with no window; launching it again shows it. Takes effect on next start",
            config.app.start_hidden,
        );
        let image_protocol = combo(
            "Pictures in nuncio-tui",
            "How the terminal client draws pictures, at its next start (NUNCIO_IMAGE_PROTOCOL wins)",
            &IMAGE_PROTOCOLS.map(|(_, name)| name),
            IMAGE_PROTOCOLS
                .iter()
                .position(|(p, _)| *p == config.ui.image_protocol)
                .unwrap_or(0),
        );
        let view = SettingsView {
            window,
            notifications,
            show_preview,
            background,
            start_hidden,
            title_bar,
            image_protocol,
            problem,
        };

        let editable = view.problem.is_none();
        for (title, rows) in [
            (
                "Notifications",
                vec![
                    view.notifications.clone().upcast::<gtk::Widget>(),
                    view.show_preview.clone().upcast(),
                ],
            ),
            (
                "Window",
                vec![
                    view.title_bar.clone().upcast(),
                    view.background.clone().upcast(),
                    view.start_hidden.clone().upcast(),
                ],
            ),
            (
                "Terminal client",
                vec![view.image_protocol.clone().upcast()],
            ),
        ] {
            let group = adw::PreferencesGroup::builder().title(title).build();
            for row in rows {
                group.add(&row);
            }
            group.set_sensitive(editable);
            page.add(&group);
        }
        let saved_to = match prefs.borrow().path() {
            Some(path) => format!(
                "Saved to {} after each change. Comments in that file are not kept.",
                path.display()
            ),
            None => "There is no configuration file.".to_owned(),
        };
        let footer = adw::PreferencesGroup::new();
        let footer_text = plain_label(&saved_to);
        footer_text.add_css_class("dim-label");
        footer.add(&footer_text);
        page.add(&footer);
        view.window.add(&page);

        // Each change is written at once; a refused or failed one puts the rows back and says why.
        let syncing = Rc::new(Cell::new(false));
        // The rows' handlers hold the rows only until the window goes, so nothing keeps it alive.
        let held = Rc::new(RefCell::new(Some(view.clone())));
        view.window.connect_destroy({
            let held = Rc::clone(&held);
            move |_| {
                held.borrow_mut().take();
            }
        });
        let apply = Rc::new(self.with(move |ui, change: Change| {
            let Some(rows) = held.borrow().clone() else {
                return;
            };
            if syncing.get() {
                return;
            }
            let result = prefs.borrow_mut().change(change);
            match result {
                Ok(true) => {
                    let config = prefs.borrow().config().clone();
                    ui.reconfigure(&config);
                    // A hand edit to another key, read back with this change, shows too.
                    syncing.set(true);
                    rows.show(&config);
                    syncing.set(false);
                    if change.takes() == Takes::NextStart {
                        rows.toast("Saved; takes effect on next start");
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    syncing.set(true);
                    rows.show(prefs.borrow().config());
                    syncing.set(false);
                    rows.toast(&error);
                }
            }
        }));
        for (row, make) in [
            (
                &view.notifications,
                Change::Notifications as fn(bool) -> Change,
            ),
            (&view.show_preview, Change::ShowPreview),
            (&view.background, Change::Background),
            (&view.start_hidden, Change::StartHidden),
        ] {
            let apply = Rc::clone(&apply);
            row.connect_active_notify(move |row| apply(make(row.is_active())));
        }
        view.title_bar.connect_selected_notify({
            let apply = Rc::clone(&apply);
            move |row| {
                if let Some((choice, _)) = usize::try_from(row.selected())
                    .ok()
                    .and_then(|i| TITLE_BARS.get(i))
                {
                    apply(Change::TitleBar(*choice));
                }
            }
        });
        view.image_protocol.connect_selected_notify({
            let apply = Rc::clone(&apply);
            move |row| {
                if let Some((protocol, _)) = usize::try_from(row.selected())
                    .ok()
                    .and_then(|i| IMAGE_PROTOCOLS.get(i))
                {
                    apply(Change::ImageProtocol(*protocol));
                }
            }
        });
        view
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    /// A closure that runs `f` on this window while it exists.
    fn with<A>(&self, f: impl Fn(&Ui, A) + 'static) -> impl Fn(A) + 'static {
        let weak = self.weak();
        move |arg| {
            if let Some(inner) = weak.upgrade() {
                f(&Ui(inner), arg);
            }
        }
    }

    /// Carry out what an action or an answer asks for, then redraw.
    fn run(&self, commands: Vec<Command>) {
        // A picture asked for again (a click, a key, the menu or the palette): one that could not
        // be shown may be decoded once more.
        let retries = self.0.controller.borrow_mut().take_picture_retries();
        for (chat, id) in retries {
            self.0.pictures.borrow_mut().retry_full(&chat, &id);
        }
        for command in commands {
            match command {
                Command::Send(frame) => match self.0.worker.borrow().as_ref() {
                    Some(worker) => worker.send(frame),
                    None => self.0.undelivered.borrow_mut().push(Command::Send(frame)),
                },
                Command::Open(path) => {
                    if self.0.worker.borrow().is_none() {
                        self.0.undelivered.borrow_mut().push(Command::Open(path));
                    } else if let Err(error) = open_externally(&path) {
                        self.0.controller.borrow_mut().open_failed(&error);
                    }
                }
            }
        }
        let alerts = self.0.controller.borrow_mut().take_alerts();
        self.deliver(alerts);
        self.render();
    }

    /// Show or withdraw notifications through the application (one id per chat, so a newer
    /// message replaces the older one's notification); without one they are kept for the tests.
    fn deliver(&self, alerts: Vec<Alert>) {
        let Some(app) = self.0.window.application() else {
            self.0.undelivered_alerts.borrow_mut().extend(alerts);
            return;
        };
        for alert in alerts {
            match alert {
                Alert::Show(shown) => {
                    let notification = gio::Notification::new(&shown.title);
                    notification.set_body(Some(&shown.body));
                    notification.set_default_action_and_target_value(
                        OPEN_CHAT_ACTION,
                        Some(&shown.chat_jid.to_variant()),
                    );
                    app.send_notification(Some(&notification_id(&shown.chat_jid)), &notification);
                }
                Alert::Withdraw(jid) => app.withdraw_notification(&notification_id(&jid)),
            }
        }
    }

    fn set_draft_text(&self, text: &str) {
        self.0.setting_draft.set(true);
        self.0.composer.buffer().set_text(text);
        self.0.setting_draft.set(false);
    }

    fn connect(&self, send_button: &gtk::Button, cancel_reply: &gtk::Button) {
        let inner = &self.0;

        inner.chats.connect_row_activated({
            let f = self.with(|ui, index: usize| ui.open_chat(index));
            move |_, row| f(usize::try_from(row.index()).unwrap_or(usize::MAX))
        });
        inner.split.connect_show_content_notify({
            let f = self.with(|ui, shown: bool| {
                if !shown && ui.0.split.is_collapsed() {
                    ui.0.controller.borrow_mut().back_to_list();
                    ui.render();
                }
            });
            move |split| f(split.shows_content())
        });

        // Only a window the user can see marks new messages read.
        let seen = self.with(|ui, (): ()| {
            let window = &ui.0.window;
            let seen = window.is_active() && window.is_visible() && !window.is_suspended();
            let commands = ui.0.controller.borrow_mut().set_seen(seen);
            ui.run(commands);
        });
        let seen = Rc::new(seen);
        for property in ["is-active", "visible", "suspended"] {
            let seen = Rc::clone(&seen);
            inner
                .window
                .connect_notify_local(Some(property), move |_, _| seen(()));
        }

        // The composer: the model holds the draft as it is typed.
        inner.composer.buffer().connect_changed({
            let f = self.with(|ui, (): ()| {
                if ui.0.setting_draft.get() {
                    return;
                }
                let text = ui.draft();
                ui.0.controller.borrow_mut().set_draft(&text);
                ui.render();
            });
            move |_| f(())
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let f = self.with(|ui, (key, state): (gdk::Key, gdk::ModifierType)| {
                ui.composer_key(key, state)
            });
            move |_, key, _, state| {
                if is_enter(key) && !state.contains(gdk::ModifierType::SHIFT_MASK)
                    || key == gdk::Key::Escape
                {
                    f((key, state));
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        inner.composer.add_controller(keys);
        send_button.connect_clicked({
            let f = self.with(|ui, (): ()| ui.send());
            move |_| f(())
        });
        cancel_reply.connect_clicked({
            let f = self.with(|ui, (): ()| {
                ui.0.controller.borrow_mut().cancel_reply();
                ui.render();
                ui.0.composer.grab_focus();
            });
            move |_| f(())
        });

        // The conversation's keys act on the focused message.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let f = self.with(|ui, (key, state): (gdk::Key, gdk::ModifierType)| {
                ui.message_key(key, state);
            });
            move |_, key, _, state| {
                if message_key_handled(key, state) {
                    f((key, state));
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        inner.transcript.add_controller(keys);
        // Enter on a picture shows it in full; on any other message it opens the menu.
        inner.transcript.connect_row_activated({
            let f = self.with(|ui, row: gtk::ListBoxRow| {
                let picture = usize::try_from(row.index())
                    .ok()
                    .filter(|&index| ui.bubble(index).is_some_and(|b| b.picture.is_some()));
                match picture {
                    Some(index) => ui.open_picture(index),
                    None => ui.message_menu(&row),
                }
            });
            move |_, row| f(row.clone())
        });
        let adjustment = inner.transcript_scroll.vadjustment();
        adjustment.connect_changed({
            let f = self.with(|ui, adjustment: gtk::Adjustment| {
                if ui.0.stick.get() {
                    adjustment.set_value(adjustment.upper() - adjustment.page_size());
                }
            });
            move |adjustment| f(adjustment.clone())
        });
        adjustment.connect_value_changed({
            let f = self.with(|ui, adjustment: gtk::Adjustment| {
                let bottom = adjustment.upper() - adjustment.page_size();
                ui.0.stick.set(adjustment.value() >= bottom - 4.0);
            });
            move |adjustment| f(adjustment.clone())
        });

        // The search.
        inner.search_entry.connect_activate({
            let f = self.with(|ui, query: String| {
                let commands = ui.0.controller.borrow_mut().search(&query);
                ui.run(commands);
            });
            move |entry| f(entry.text().to_string())
        });
        inner.search_bar.connect_search_mode_enabled_notify({
            let f = self.with(|ui, enabled: bool| {
                if !enabled {
                    ui.0.controller.borrow_mut().close_search();
                    ui.render();
                }
            });
            move |bar| f(bar.is_search_mode())
        });
        inner.results.connect_row_activated({
            let f = self.with(|ui, index: usize| ui.jump_to_result(index));
            move |_, row| f(usize::try_from(row.index()).unwrap_or(usize::MAX))
        });
        let open_search = self.with(|ui, (): ()| ui.start_search());
        inner.shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                open_search(());
                glib::Propagation::Stop
            })),
        ));

        // The command palette and the settings: window actions (also in the menu) with keys.
        for (name, triggers, run) in [
            (
                "command-palette",
                &["F1", "<Control>k"][..],
                Box::new(self.with(|ui, (): ()| ui.open_palette())) as Box<dyn Fn(())>,
            ),
            (
                "settings",
                &["<Control>comma"][..],
                Box::new(self.with(|ui, (): ()| ui.open_settings())),
            ),
        ] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| run(()));
            inner.window.add_action(&action);
            for trigger in triggers {
                inner.shortcuts.add_shortcut(gtk::Shortcut::new(
                    gtk::ShortcutTrigger::parse_string(trigger),
                    Some(gtk::NamedAction::new(&format!("win.{name}"))),
                ));
            }
        }
    }

    /// Open the search of the selected chat (Ctrl+F).
    fn start_search(&self) {
        if self.0.controller.borrow().model().selected_chat().is_some() {
            self.0.search_bar.set_search_mode(true);
            self.0.search_entry.grab_focus();
        }
    }

    fn composer_key(&self, key: gdk::Key, _state: gdk::ModifierType) {
        if key == gdk::Key::Escape {
            let replying = self
                .0
                .controller
                .borrow()
                .model()
                .composer()
                .reply_to()
                .is_some();
            if replying {
                self.0.controller.borrow_mut().cancel_reply();
                self.render();
            } else if let Some(row) = last_row(&self.0.transcript) {
                row.grab_focus();
            }
        } else {
            self.send();
        }
    }

    /// The focused message's position in the conversation.
    fn focused_message(&self) -> Option<(usize, gtk::ListBoxRow)> {
        let row = self
            .0
            .transcript
            .focus_child()?
            .downcast::<gtk::ListBoxRow>()
            .ok()?;
        Some((usize::try_from(row.index()).ok()?, row))
    }

    fn message_key(&self, key: gdk::Key, state: gdk::ModifierType) {
        if key == gdk::Key::Escape {
            self.0.composer.grab_focus();
            return;
        }
        let Some((index, row)) = self.focused_message() else {
            return;
        };
        match key {
            gdk::Key::r => self.reply(index),
            gdk::Key::e => self.reaction_picker(&row, index),
            gdk::Key::R => {
                let commands = self.0.controller.borrow_mut().retry(index);
                self.run(commands);
            }
            gdk::Key::o => self.open_picture(index),
            gdk::Key::O => {
                let commands = self.0.controller.borrow_mut().open_externally(index);
                self.run(commands);
            }
            gdk::Key::Menu => self.message_menu(&row),
            gdk::Key::F10 if state.contains(gdk::ModifierType::SHIFT_MASK) => {
                self.message_menu(&row);
            }
            _ => {}
        }
    }

    /// Open media message `index` ([`Controller::open_media`]): a picture's full file is fetched
    /// and shown in its bubble (one that could not be shown is tried again), any other file is
    /// opened outside.
    fn open_picture(&self, index: usize) {
        // The retry of a picture that could not be shown is taken in by `run`, as for the
        // palette's Open.
        let commands = self.0.controller.borrow_mut().open_media(index);
        self.run(commands);
    }

    /// A picture was decoded (or could not be): its texture goes into the cache and into every
    /// widget drawing that message now, a full picture over its thumbnail. A full picture that
    /// cannot be shown says why; a thumbnail just keeps its label.
    fn picture_decoded(&self, done: Decoded) {
        let Decoded { key, outcome } = done;
        let outcome = outcome.map(|pixels| (pixels.rgba.len(), pixels.source, texture(pixels)));
        let outcome = outcome.map(|(bytes, source, texture)| (texture, bytes, source));
        let shown = match &outcome {
            Ok((texture, _, source)) => Ok((texture.clone(), *source)),
            Err(error) => Err(error.clone()),
        };
        if !self.0.pictures.borrow_mut().finish(key.clone(), outcome) {
            return;
        }
        match shown {
            Ok((texture, source)) => {
                let mut live = self.0.live.borrow_mut();
                let Some(slots) = live.get_mut(&(key.chat.clone(), key.id.clone())) else {
                    return;
                };
                slots.retain(|slot| slot.picture.upgrade().is_some());
                for slot in slots.iter() {
                    let Some(picture) = slot.picture.upgrade() else {
                        continue;
                    };
                    let full = key.variant == Variant::Full;
                    if !full && picture.paintable().is_some() {
                        continue;
                    }
                    if !slot.sized {
                        // Its first image fixes its box; later ones keep it (one resize).
                        let fixed =
                            self.0
                                .pictures
                                .borrow_mut()
                                .settle_box(&key, source, slot.sticker);
                        reserve(&picture, fixed);
                    }
                    picture.set_paintable(Some(&texture));
                }
            }
            Err(error) if key.variant == Variant::Full => {
                self.0.controller.borrow_mut().picture_failed(&error);
                self.render();
            }
            Err(_) => {}
        }
    }

    /// The picture drawn in bubble `index` (tests): the box it reserves, the size of the texture
    /// in it, and its alternative text.
    pub fn picture(&self, index: usize) -> Option<PictureState> {
        let row = self.0.transcript.row_at_index(i32::try_from(index).ok()?)?;
        let picture = find_picture(row.upcast_ref())?;
        let paintable = picture.paintable();
        // The box around it (its two clamps), as GTK would lay it out: its natural width, and its
        // natural height at that width.
        let outer = picture
            .parent()
            .and_then(|tall| tall.parent())
            .unwrap_or_else(|| picture.clone().upcast());
        let (_, width, _, _) = outer.measure(gtk::Orientation::Horizontal, -1);
        let (_, height, _, _) = outer.measure(gtk::Orientation::Vertical, width);
        Some(PictureState {
            reserved: picture.size_request(),
            laid_out: (width, height),
            texture: paintable.map(|p| (p.intrinsic_width(), p.intrinsic_height())),
            alt: picture.alternative_text().map(|t| t.to_string()),
        })
    }

    /// Show media message `index` as a click on its picture does (tests).
    pub fn click_picture(&self, index: usize) {
        self.open_picture(index);
    }

    /// The text of the last toast shown (tests).
    pub fn last_toast(&self) -> Option<String> {
        self.0.last_toast.borrow().clone()
    }

    /// Let every decoded picture go, as eviction does (tests).
    pub fn evict_pictures(&self) {
        self.0.pictures.borrow_mut().clear();
    }

    /// Run a palette command as choosing it does (tests).
    pub fn run_palette_action(&self, action: &palette::Action) {
        self.palette_step(action);
    }

    /// How many decoded pictures the cache holds (tests).
    pub fn pictures_cached(&self) -> usize {
        self.0.pictures.borrow().len()
    }

    fn reply(&self, index: usize) {
        let replying = self.0.controller.borrow_mut().reply(index);
        self.render();
        if replying {
            self.0.composer.grab_focus();
        }
    }

    fn jump_to_result(&self, index: usize) {
        let found = self.0.controller.borrow_mut().jump_to_result(index);
        self.0.search_bar.set_search_mode(false);
        self.0.stick.set(false);
        self.render();
        if let Some(row) = found
            .and_then(|i| i32::try_from(i).ok())
            .and_then(|i| self.0.transcript.row_at_index(i))
        {
            row.grab_focus();
        }
    }

    /// The bubble drawn at `index`.
    fn bubble(&self, index: usize) -> Option<Bubble> {
        self.0.shown_bubbles.borrow().get(index).cloned()
    }

    /// The message's menu: what can be done with it now.
    fn message_menu(&self, row: &gtk::ListBoxRow) {
        let Some(index) = usize::try_from(row.index()).ok() else {
            return;
        };
        let Some(bubble) = self.bubble(index) else {
            return;
        };
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let popover = popover_on(row, &menu);
        popover.update_property(&[Property::Label("Message menu")]);
        let item = |label: &str, key: &str, enabled: bool, action: Box<dyn Fn(&Ui)>| {
            let button = gtk::Button::builder()
                .label(label)
                .sensitive(enabled)
                .tooltip_text(key)
                .build();
            button.add_css_class("flat");
            let popover = popover.clone();
            let run = self.with(move |ui, (): ()| {
                popover.popdown();
                action(ui);
            });
            button.connect_clicked(move |_| run(()));
            menu.append(&button);
        };
        let row_for_react = row.clone();
        item(
            "Reply",
            "r",
            bubble.can_target,
            Box::new(move |ui| ui.reply(index)),
        );
        item(
            "React…",
            "e",
            bubble.can_target,
            Box::new(move |ui| ui.reaction_picker(&row_for_react, index)),
        );
        item(
            "Retry",
            "Shift+R",
            bubble.can_retry,
            Box::new(move |ui| {
                let commands = ui.0.controller.borrow_mut().retry(index);
                ui.run(commands);
            }),
        );
        item(
            "Open",
            "o",
            bubble.can_open,
            Box::new(move |ui| ui.open_picture(index)),
        );
        if bubble.picture.is_some() {
            item(
                "Open externally",
                "Shift+O",
                bubble.can_open,
                Box::new(move |ui| {
                    let commands = ui.0.controller.borrow_mut().open_externally(index);
                    ui.run(commands);
                }),
            );
        }
        popover.popup();
    }

    /// The reaction picker: the shared fixed set, and removing mine.
    fn reaction_picker(&self, row: &gtk::ListBoxRow, index: usize) {
        if !self.bubble(index).is_some_and(|b| b.can_target) {
            // The model says why in the notice.
            let commands = self.0.controller.borrow_mut().react(index, "");
            self.run(commands);
            return;
        }
        let picker = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let popover = popover_on(row, &picker);
        popover.update_property(&[Property::Label("React")]);
        for emoji in REACTIONS.iter().copied().chain([""]) {
            let button = if emoji.is_empty() {
                gtk::Button::builder()
                    .label("Remove")
                    .tooltip_text("Remove my reaction")
                    .build()
            } else {
                gtk::Button::builder()
                    .label(emoji)
                    .tooltip_text(format!("React with {emoji}"))
                    .build()
            };
            button.add_css_class("flat");
            let popover = popover.clone();
            let react = self.with(move |ui, (): ()| {
                popover.popdown();
                let commands = ui.0.controller.borrow_mut().react(index, emoji);
                ui.run(commands);
            });
            button.connect_clicked(move |_| react(()));
            picker.append(&button);
        }
        popover.popup();
    }

    /// Redraw what changed in the model.
    pub fn render(&self) {
        let inner = &self.0;
        let mut controller = inner.controller.borrow_mut();
        let offset = inner.offset_minutes;
        let model = controller.model();

        let sidebar = present::sidebar_header(model);
        inner.sidebar_title.set_title(&sidebar.title);
        inner.sidebar_title.set_subtitle(&sidebar.subtitle);
        let header = present::conversation_header(model);
        inner.content_title.set_title(&header.title);
        inner.content_title.set_subtitle(&header.subtitle);
        inner.content_page.set_title(&header.title);

        sync_rows(
            &inner.chats,
            &mut inner.shown_chats.borrow_mut(),
            present::chat_rows(model, offset),
            chat_row,
        );
        let highlighted = model.chat_list().highlighted();
        if highlighted != inner.highlighted.get() {
            inner.highlighted.set(highlighted);
            match highlighted
                .and_then(|i| i32::try_from(i).ok())
                .and_then(|i| inner.chats.row_at_index(i))
            {
                Some(row) => inner.chats.select_row(Some(&row)),
                None => inner.chats.unselect_all(),
            }
        }

        let weak = self.weak();
        sync_rows(
            &inner.transcript,
            &mut inner.shown_bubbles.borrow_mut(),
            present::bubbles(model, offset),
            |bubble| {
                inner.built.borrow_mut().push(bubble.clone());
                bubble_row(&weak, bubble)
            },
        );
        // Only a bubble just (re)built asks for its pictures: one drawn already keeps its texture
        // even once the cache let it go, so nothing is decoded over and over.
        let built = std::mem::take(&mut *inner.built.borrow_mut());
        {
            let mut cache = inner.pictures.borrow_mut();
            for bubble in &built {
                for request in pictures::requests(model, bubble, &cache) {
                    if cache.start(&request.key) {
                        inner.decoder.decode(request);
                    }
                }
            }
        }
        inner.live.borrow_mut().retain(|_, slots| {
            slots.retain(|slot| slot.picture.upgrade().is_some());
            !slots.is_empty()
        });

        let composer = present::composer_view(model);
        inner.reply_bar.set_visible(composer.replying.is_some());
        inner
            .reply_label
            .set_text(composer.replying.as_deref().unwrap_or_default());
        inner.reply_label.update_property(&[Property::Label(
            composer.replying.as_deref().unwrap_or_default(),
        )]);
        inner
            .blocked
            .set_title(composer.blocked.as_deref().unwrap_or_default());
        inner.blocked.set_revealed(composer.blocked.is_some());

        let search = present::search_view(model, offset);
        let (status, lines) = match &search {
            None => (None, Vec::new()),
            Some(view) if view.awaiting => (Some("searching…"), Vec::new()),
            Some(view) => match &view.results {
                None => (None, Vec::new()),
                Some(lines) if lines.is_empty() => (Some("no messages found"), Vec::new()),
                Some(lines) => (None, lines.clone()),
            },
        };
        inner.search_status.set_visible(status.is_some());
        inner.search_status.set_text(status.unwrap_or_default());
        inner.results_scroll.set_visible(!lines.is_empty());
        sync_rows(
            &inner.results,
            &mut inner.shown_results.borrow_mut(),
            lines,
            |line: &String| result_row(line),
        );
        // The model closed its search (a result chosen, another chat opened): so does the bar.
        let was_searching = inner.searching.replace(search.is_some());
        let close_search_bar = was_searching && search.is_none();

        if controller.take_view_reset() {
            inner.stick.set(true);
            let adjustment = inner.transcript_scroll.vadjustment();
            adjustment.set_value(adjustment.upper() - adjustment.page_size());
        }
        if let Some(notice) = controller.take_notice() {
            let toast = adw::Toast::new(&notice);
            toast.set_use_markup(false);
            toast.set_timeout(4);
            inner.toasts.add_toast(toast);
            *inner.last_toast.borrow_mut() = Some(notice);
        }
        // Another conversation opened (a notification clicked, a link): the palette's commands
        // were for the one before, so it closes.
        let conversation = open_conversation(controller.model());
        let stale_palette = inner
            .palette
            .borrow()
            .as_ref()
            .is_some_and(|view| view.conversation != conversation);
        // A draft put there by the model (a link) is shown; a typed one is already there.
        let draft = controller
            .take_draft_changed()
            .then(|| controller.model().composer().text().to_owned());
        drop(controller);
        if let Some(draft) = draft {
            self.set_draft_text(&draft);
        }
        if stale_palette {
            self.close_palette();
        }
        // Its notify handler closes the model's search, so the model must be free.
        if close_search_bar {
            inner.search_bar.set_search_mode(false);
        }
    }
}

/// Whether the window shows its title bar for `choice` on this desktop
/// ([`present::shows_title_bar`] with this process's environment).
pub fn title_bar_here(choice: TitleBar) -> bool {
    present::shows_title_bar(
        choice,
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    )
}

/// The chat whose conversation is open, if one is.
fn open_conversation(model: &nuncio_client::Model) -> Option<String> {
    model
        .is_open()
        .then(|| model.selected_jid().map(str::to_owned))
        .flatten()
}

/// The settings window and its rows.
#[derive(Clone)]
pub struct SettingsView {
    pub window: adw::PreferencesWindow,
    pub notifications: adw::SwitchRow,
    pub show_preview: adw::SwitchRow,
    pub background: adw::SwitchRow,
    pub start_hidden: adw::SwitchRow,
    pub title_bar: adw::ComboRow,
    pub image_protocol: adw::ComboRow,
    /// Why the file cannot be changed (shown at the top; every row is insensitive).
    pub problem: Option<String>,
}

impl SettingsView {
    /// Put every row back to `config`.
    fn show(&self, config: &nuncio_client::Config) {
        self.notifications.set_active(config.notifications.enabled);
        self.show_preview
            .set_active(config.notifications.show_preview);
        self.background.set_active(config.app.background);
        self.start_hidden.set_active(config.app.start_hidden);
        let at = |found: Option<usize>| u32::try_from(found.unwrap_or(0)).unwrap_or(0);
        self.title_bar.set_selected(at(TITLE_BARS
            .iter()
            .position(|(c, _)| *c == config.app.title_bar)));
        self.image_protocol.set_selected(at(IMAGE_PROTOCOLS
            .iter()
            .position(|(p, _)| *p == config.ui.image_protocol)));
    }

    fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        toast.set_timeout(5);
        self.window.add_toast(toast);
    }
}

/// A wrapping, selectable label of plain text (no markup).
fn plain_label(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .use_markup(false)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();
    label.update_property(&[Property::Label(text)]);
    label
}

/// The command palette's widgets and what it lists.
struct PaletteView {
    popover: gtk::Popover,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    entries: Vec<palette::Entry>,
    /// The conversation open when the palette opened: another one opening closes the palette.
    conversation: Option<String>,
    /// The positions in `entries` listed now, in order.
    shown: RefCell<Vec<usize>>,
}

impl PaletteView {
    fn new(entries: Vec<palette::Entry>, conversation: Option<String>) -> Self {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Type a command or a chat name")
            .hexpand(true)
            .build();
        entry.update_property(&[Property::Label("Command palette: type to filter")]);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        list.add_css_class("navigation-sidebar");
        list.update_property(&[Property::Label("Commands")]);
        list.set_placeholder(Some(&label("No matching command", "dim-label")));
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .min_content_width(420)
            .child(&list)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.append(&entry);
        column.append(&scroll);
        let popover = gtk::Popover::builder()
            .child(&column)
            .has_arrow(false)
            .autohide(true)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.add_css_class(ROOT_CLASS);
        popover.update_property(&[Property::Label("Command palette")]);
        Self {
            popover,
            entry,
            list,
            scroll,
            entries,
            conversation,
            shown: RefCell::new(Vec::new()),
        }
    }

    /// List the commands `query` matches, best first, and select the first.
    fn filter(&self, query: &str) {
        let shown = palette::filter(&self.entries, query);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for &index in &shown {
            self.list.append(&palette_row(&self.entries[index]));
        }
        *self.shown.borrow_mut() = shown;
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    /// Move the selection `step` rows (the focus stays in the search).
    fn move_selection(&self, step: i32) {
        let count = i32::try_from(self.shown.borrow().len()).unwrap_or(0);
        if count == 0 {
            return;
        }
        let at = self.list.selected_row().map_or(0, |row| row.index());
        let next = (at + step).clamp(0, count - 1);
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            // Keep it in view; the focus stays in the search.
            if let Some(bounds) = row.compute_bounds(&self.list) {
                let adjustment = self.scroll.vadjustment();
                let (top, bottom) = (
                    f64::from(bounds.y()),
                    f64::from(bounds.y() + bounds.height()),
                );
                if top < adjustment.value() {
                    adjustment.set_value(top);
                } else if bottom > adjustment.value() + adjustment.page_size() {
                    adjustment.set_value(bottom - adjustment.page_size());
                }
            }
        }
    }
}

fn palette_row(entry: &palette::Entry) -> gtk::ListBoxRow {
    let title = label(&entry.title, "");
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_margin_top(4);
    line.set_margin_bottom(4);
    line.append(&title);
    if let Some(key) = entry.shortcut {
        let key = label(key, "dim-label");
        key.add_css_class("caption");
        line.append(&key);
    }
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .child(&line)
        .build();
    row.update_property(&[Property::Label(&entry.label())]);
    row
}

/// A header bar; without the title bar it shows no window controls.
fn header_bar(title_bar: bool) -> adw::HeaderBar {
    let header = adw::HeaderBar::new();
    header.add_css_class("nuncio-headerbar");
    header.set_show_start_title_buttons(title_bar);
    header.set_show_end_title_buttons(title_bar);
    header
}

fn is_enter(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
    )
}

/// The keys the conversation acts on (the others move the focus as GTK does).
fn message_key_handled(key: gdk::Key, state: gdk::ModifierType) -> bool {
    let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK);
    plain
        && matches!(
            key,
            gdk::Key::r
                | gdk::Key::R
                | gdk::Key::e
                | gdk::Key::o
                | gdk::Key::O
                | gdk::Key::Menu
                | gdk::Key::Escape
        )
        || key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK)
}

/// A popover on `row` holding `child`, removed once closed.
///
/// It hangs off the transcript, pointing at the row, rather than off the row itself: an event can
/// rebuild the row while the popover is open, and a popover must never outlive its parent.
fn popover_on(row: &gtk::ListBoxRow, child: &impl IsA<gtk::Widget>) -> gtk::Popover {
    let popover = gtk::Popover::builder().child(child).build();
    match row.parent() {
        Some(list) => {
            popover.set_parent(&list);
            if let Some(bounds) = row.compute_bounds(&list) {
                popover.set_pointing_to(Some(&gdk::Rectangle::new(
                    bounds.x() as i32,
                    bounds.y() as i32,
                    bounds.width() as i32,
                    bounds.height() as i32,
                )));
            }
        }
        None => popover.set_parent(row),
    }
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover
}

fn count_rows(list: &gtk::ListBox) -> usize {
    let mut n = 0;
    while list.row_at_index(n).is_some() {
        n += 1;
    }
    usize::try_from(n).unwrap_or_default()
}

fn last_row(list: &gtk::ListBox) -> Option<gtk::ListBoxRow> {
    let n = i32::try_from(count_rows(list)).ok()?;
    list.row_at_index(n - 1)
}

/// Make `list` show `new`, rebuilding only the rows that changed; a rebuilt row that had the
/// focus gets it back.
fn sync_rows<T: PartialEq>(
    list: &gtk::ListBox,
    shown: &mut Vec<T>,
    new: Vec<T>,
    build: impl Fn(&T) -> gtk::ListBoxRow,
) {
    for (index, item) in new.iter().enumerate() {
        let Ok(position) = i32::try_from(index) else {
            break;
        };
        match list.row_at_index(position) {
            Some(_) if shown.get(index) == Some(item) => {}
            Some(old) => {
                let focused = old.has_focus();
                list.remove(&old);
                let row = build(item);
                list.insert(&row, position);
                if focused {
                    row.grab_focus();
                }
            }
            None => list.append(&build(item)),
        }
    }
    while let Some(extra) = i32::try_from(new.len())
        .ok()
        .and_then(|n| list.row_at_index(n))
    {
        list.remove(&extra);
    }
    *shown = new;
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label
}

fn chat_row(chat: &ChatRow) -> gtk::ListBoxRow {
    let avatar = adw::Avatar::new(36, Some(&chat.title), true);
    let title = label(&chat.title, "heading");
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let time = label(&chat.time, "caption");
    time.add_css_class("nuncio-muted");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&title);
    top.append(&time);

    let preview = label(&chat.preview, "nuncio-muted");
    preview.set_hexpand(true);
    preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom.append(&preview);
    // The same marks the terminal client shows; never colour alone.
    let marks: String = [(chat.pinned, '★'), (chat.muted, '⊘'), (chat.archived, '▤')]
        .iter()
        .filter_map(|&(on, mark)| on.then_some(mark))
        .collect();
    if !marks.is_empty() {
        bottom.append(&label(&marks, "nuncio-accent"));
    }
    if let Some(unread) = &chat.unread {
        title.add_css_class("nuncio-bold");
        bottom.append(&label(unread, "nuncio-unread"));
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&top);
    text.append(&bottom);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.append(&avatar);
    content.append(&text);
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .child(&content)
        .build();
    row.update_property(&[Property::Label(&chat.label)]);
    row
}

fn result_row(line: &str) -> gtk::ListBoxRow {
    let text = label(line, "");
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_margin_top(4);
    text.set_margin_bottom(4);
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .child(&text)
        .build();
    row.update_property(&[Property::Label(line)]);
    row
}

/// One message as a bubble: mine on the right in the accent, theirs on the left. The row is what
/// takes the focus and what a screen reader reads, by its label.
fn bubble_row(ui: &Weak<Inner>, bubble: &Bubble) -> gtk::ListBoxRow {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.add_css_class("nuncio-bubble");
    column.add_css_class(if bubble.mine {
        "nuncio-mine"
    } else {
        "nuncio-theirs"
    });
    if bubble.mark.is_some_and(present::SendMark::is_failure) {
        column.add_css_class("nuncio-failed");
    }
    if !bubble.mine {
        column.append(&label(&bubble.sender, "nuncio-sender"));
    }
    if let Some(quote) = &bubble.quote {
        let quote = label(&format!("↪ {quote}"), "nuncio-quote");
        quote.set_ellipsize(gtk::pango::EllipsizeMode::End);
        column.append(&quote);
    }
    if let Some(media) = &bubble.media {
        column.append(&label(media, "nuncio-media"));
    }
    if let Some(picture) = &bubble.picture {
        column.append(&picture_widget(ui, bubble.index, picture));
    }
    if !bubble.text.is_empty() {
        let text = label(&bubble.text, "nuncio-text");
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_max_width_chars(56);
        column.append(&text);
    }
    if !bubble.reactions.is_empty() {
        let reactions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for reaction in &bubble.reactions {
            let shown = label(&reaction.emoji, "nuncio-reaction");
            if reaction.mine {
                shown.add_css_class("nuncio-reaction-mine");
            }
            reactions.append(&shown);
        }
        column.append(&reactions);
    }
    let footer = match bubble.mark {
        Some(mark) => format!("{} · {}", bubble.time, mark.as_str()),
        None => bubble.time.clone(),
    };
    let footer = label(&footer, "caption");
    footer.set_halign(gtk::Align::End);
    if bubble.mark.is_some_and(present::SendMark::is_failure) {
        footer.add_css_class("nuncio-error");
    }
    column.append(&footer);
    if bubble.can_retry {
        let retry = gtk::Button::builder()
            .label("Retry")
            .tooltip_text("Send it once more (Shift+R)")
            .halign(gtk::Align::End)
            .build();
        retry.add_css_class("flat");
        let ui = ui.clone();
        let index = bubble.index;
        retry.connect_clicked(move |_| {
            if let Some(inner) = ui.upgrade() {
                let ui = Ui(inner);
                let commands = ui.0.controller.borrow_mut().retry(index);
                ui.run(commands);
            }
        });
        column.append(&retry);
    }

    let lane = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    column.set_halign(if bubble.mine {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });
    column.set_hexpand(true);
    lane.append(&column);
    lane.set_margin_start(12);
    lane.set_margin_end(12);
    lane.set_margin_top(3);
    lane.set_margin_bottom(3);
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .activatable(true)
        .child(&lane)
        .build();
    row.add_css_class("nuncio-message");
    row.update_property(&[Property::Label(&bubble.label)]);

    // A right click opens the message's menu.
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let menu_ui = ui.clone();
    click.connect_pressed(move |gesture, _, _, _| {
        if let (Some(inner), Some(row)) = (
            menu_ui.upgrade(),
            gesture
                .widget()
                .and_then(|w| w.downcast::<gtk::ListBoxRow>().ok()),
        ) {
            row.grab_focus();
            Ui(inner).message_menu(&row);
        }
    });
    row.add_controller(click);
    row
}

/// What a bubble's picture widget shows ([`Ui::picture`], tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureState {
    /// The box it reserves; `(-1, -1)` before it has one.
    pub reserved: (i32, i32),
    /// The size its box takes in the bubble: the reserved box, whatever the texture's size.
    pub laid_out: (i32, i32),
    /// The size of the texture drawn in it, once one is.
    pub texture: Option<(i32, i32)>,
    pub alt: Option<String>,
}

/// A decoded picture as a texture: RGBA, not premultiplied, so a sticker's transparent pixels show
/// the bubble behind it.
fn texture(pixels: pictures::Pixels) -> gdk::Texture {
    let width = i32::try_from(pixels.width).unwrap_or(1);
    let height = i32::try_from(pixels.height).unwrap_or(1);
    let stride = pixels.width as usize * 4;
    gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(pixels.rgba),
        stride,
    )
    .upcast()
}

/// Give `picture` a box of exactly `width` x `height`: at least that (its size request) and at
/// most that (the two clamps around it), whatever the texture's own size.
fn reserve(picture: &gtk::Picture, (width, height): (u32, u32)) {
    let (width, height) = (
        i32::try_from(width).unwrap_or(1),
        i32::try_from(height).unwrap_or(1),
    );
    picture.set_size_request(width, height);
    let tall = picture.parent().and_downcast::<adw::Clamp>();
    let wide = tall
        .as_ref()
        .and_then(WidgetExt::parent)
        .and_downcast::<adw::Clamp>();
    for (clamp, side) in [(tall, height), (wide, width)] {
        if let Some(clamp) = clamp {
            clamp.set_maximum_size(side);
            clamp.set_tightening_threshold(side);
        }
    }
}

/// The picture inside an image or sticker bubble: its box reserved at once from the size the
/// message states, the best texture the cache has (the full picture, else the thumbnail), and a
/// click that shows the full picture ([`Ui::open_picture`]). Without a texture it stays empty
/// under the bubble's `[image]` / `[sticker]` label.
fn picture_widget(ui: &Weak<Inner>, index: usize, view: &PictureView) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .alternative_text(view.alt.as_str())
        .build();
    picture.add_css_class("nuncio-picture");
    let tall = adw::Clamp::builder()
        .orientation(gtk::Orientation::Vertical)
        .child(&picture)
        .build();
    let wide = adw::Clamp::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Start)
        .child(&tall)
        .build();
    if let Some(size) = view.size {
        reserve(&picture, size);
    }
    if let Some(inner) = ui.upgrade() {
        let mut cache = inner.pictures.borrow_mut();
        // A picture with no stated size keeps the box its first image gave it, even once its
        // pictures left the cache.
        let known = view
            .size
            .is_none()
            .then(|| cache.known_box(&view.chat, &view.id))
            .flatten();
        if let Some(size) = known {
            reserve(&picture, size);
        }
        if let Some((texture, variant)) = cache.best(&view.chat, &view.id) {
            if view.size.is_none() && known.is_none() {
                let source = match variant {
                    Variant::Full => cache.full_pixels(&view.chat, &view.id),
                    Variant::Thumb => None,
                }
                .unwrap_or((
                    u32::try_from(texture.width()).unwrap_or(1),
                    u32::try_from(texture.height()).unwrap_or(1),
                ));
                let key = pictures::Key::new(&view.chat, &view.id, variant);
                reserve(&picture, cache.settle_box(&key, source, view.sticker));
            }
            picture.set_paintable(Some(&texture));
        }
        drop(cache);
        inner
            .live
            .borrow_mut()
            .entry((view.chat.clone(), view.id.clone()))
            .or_default()
            .push(Slot {
                picture: picture.downgrade(),
                sticker: view.sticker,
                sized: view.size.is_some(),
            });
    }
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let ui = ui.clone();
    click.connect_released(move |gesture, presses, _, _| {
        if presses != 1 {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(inner) = ui.upgrade() {
            Ui(inner).open_picture(index);
        }
    });
    picture.add_controller(click);
    wide.upcast()
}

/// The first picture widget inside `widget`.
fn find_picture(widget: &gtk::Widget) -> Option<gtk::Picture> {
    if let Some(picture) = widget.downcast_ref::<gtk::Picture>() {
        return Some(picture.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_picture(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}
