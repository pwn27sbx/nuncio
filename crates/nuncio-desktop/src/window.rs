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
//!   that was not sent, `o` opens a media file, Enter, Menu or Shift+F10 opens the message's menu,
//!   Escape goes to the composer;
//! - composer: Enter sends, Shift+Enter starts a new line, Escape cancels the reply;
//! - Ctrl+F searches the open chat; Escape closes the search.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;
use gtk::accessible::Property;
use gtk::{gdk, glib};
use nuncio_client::driver::open_externally;
use nuncio_client::model::REACTIONS;

use crate::ROOT_CLASS;
use crate::controller::{Command, Controller};
use crate::present::{self, Bubble, ChatRow};
use crate::worker::{Backlog, EVENT_BACKLOG, Incoming, Settings, Worker};

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
    offset_minutes: i32,
    window: adw::ApplicationWindow,
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

        let ui = Self(Rc::new(Inner {
            controller: RefCell::new(controller),
            worker: RefCell::new(None),
            undelivered: RefCell::new(Vec::new()),
            offset_minutes,
            window,
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
        }));
        ui.connect(&send_button, &cancel_reply);
        // Every signal handler holds the state weakly; the window holds it strongly, so the state
        // (and its worker) lives exactly as long as the window, whoever else drops their `Ui`.
        // Closing the window lets the state go (and with it the worker's threads). Not on
        // `destroy`: that only comes once the last reference to the window goes, and the state
        // holds one.
        let owner = RefCell::new(Some(ui.clone()));
        ui.0.window.connect_close_request(move |_| {
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
        self.render();
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
        inner.transcript.connect_row_activated({
            let f = self.with(|ui, row: gtk::ListBoxRow| ui.message_menu(&row));
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
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_scope(gtk::ShortcutScope::Global);
        let open_search = self.with(|ui, (): ()| {
            if ui.0.controller.borrow().model().selected_chat().is_some() {
                ui.0.search_bar.set_search_mode(true);
                ui.0.search_entry.grab_focus();
            }
        });
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                open_search(());
                glib::Propagation::Stop
            })),
        ));
        inner.window.add_controller(shortcuts);
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
            gdk::Key::o => {
                let commands = self.0.controller.borrow_mut().open_media(index);
                self.run(commands);
            }
            gdk::Key::Menu => self.message_menu(&row),
            gdk::Key::F10 if state.contains(gdk::ModifierType::SHIFT_MASK) => {
                self.message_menu(&row);
            }
            _ => {}
        }
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
            Box::new(move |ui| {
                let commands = ui.0.controller.borrow_mut().open_media(index);
                ui.run(commands);
            }),
        );
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
            |bubble| bubble_row(&weak, bubble),
        );

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
        }
        drop(controller);
        // Its notify handler closes the model's search, so the model must be free.
        if close_search_bar {
            inner.search_bar.set_search_mode(false);
        }
    }
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
