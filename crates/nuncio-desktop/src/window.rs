//! The window: three panes side by side (design plan §5) — the rail (the one filled button, the
//! filters, the connection), the chat list, and the open conversation (transcript, search,
//! composer) — with a draggable `gtk::Paned` at each seam, laid out after Sealwax
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
//! - chat list: Up/Down (or `j`/`k`) move, Enter opens the chat (and only opening marks it
//!   read), Escape closes the search; the rail's All / Unread / Archived choose which chats it
//!   lists;
//! - conversation: Up/Down (or `j`/`k`) move between messages, `g` goes to the first and Shift+G
//!   to the last; `r` replies, `e` reacts, Shift+R retries a send
//!   that was not sent, `o` opens a media file (a picture's full file is shown in its bubble),
//!   Shift+O opens it with the desktop's opener, Enter shows a picture in full (on any other
//!   message it opens the menu), Menu or Shift+F10 opens the message's menu, Escape goes to the
//!   composer (on a window narrow enough for one pane, back to the list);
//! - composer: Enter (or Ctrl+Enter) sends, Alt+Enter (or Shift+Enter) starts a new line, Escape
//!   cancels the reply, else goes to the last message;
//! - Ctrl+F, the conversation header's ⌕, or `/` while no text field has the focus, searches the
//!   selected chat: a 32 px input under the conversation header, the results above the
//!   transcript (plan §6.6, S7); Escape closes the search; Ctrl+Q quits (the app's `app.quit`, also in the menu);
//! - F1 or Ctrl+K opens the command palette ([`crate::palette`]) over a scrim: type to filter,
//!   Up/Down choose, Enter runs, Escape closes; Ctrl+, opens the settings
//!   ([`crate::preferences`]);
//! - the rail's New message (also in the palette; no key) asks for a phone number in a small
//!   dialog (plan §6.8): Enter opens its chat as a `whatsapp://send?phone=…` link with no text
//!   does (an empty composer, nothing sent), Escape cancels.

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

use crate::controller::{Alert, Command, Controller, OPEN_CHAT_ACTION, notification_id};
use crate::lifecycle::{Close, Lifecycle};
use crate::palette::{self, Step};
use crate::pictures::{self, Cache, Decoded, Decoder, Variant};
use crate::preferences::{Change, IMAGE_PROTOCOLS, Preferences, TITLE_BARS, Takes};
use crate::present::{self, Bubble, ChatRow, Filter, PictureView};
use crate::style::{self, LIST_WIDTH, Layout, RAIL_COMPACT_WIDTH, RAIL_WIDTH, SETTINGS_WIDTH};
use crate::worker::{Backlog, EVENT_BACKLOG, Incoming, Settings, Worker};
use crate::{ROOT_CLASS, WINDOW_CLASS};
use nuncio_client::TitleBar;
use nuncio_client::config::App;

/// The rail's one filled button: its label, its accessible name and its tooltip (S6).
const NEW_MESSAGE: &str = "New message";

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
    list_header: adw::HeaderBar,
    content_header: adw::HeaderBar,
    /// The window controls are wanted (`[app] title_bar`).
    title_bar: Cell<bool>,
    /// The window's own shortcuts (Ctrl+F, `/`, F1, Ctrl+K, Ctrl+,).
    shortcuts: gtk::ShortcutController,
    /// The command palette while it is open.
    palette: RefCell<Option<Rc<PaletteView>>>,
    /// The "New message" dialog while it is open.
    new_message: RefCell<Option<Rc<NewMessageView>>>,
    /// What dims the window while the palette is open.
    scrim: gtk::Box,
    /// The configuration file the settings edit (`None`: there is none).
    config_path: RefCell<Option<PathBuf>>,
    toasts: adw::ToastOverlay,
    /// The seam between the rail and the rest.
    panes: gtk::Paned,
    /// The seam between the chat list and the conversation.
    lists: gtk::Paned,
    rail: Rail,
    list_view: adw::ToolbarView,
    /// The chat list's title.
    list_title: gtk::Label,
    content_view: adw::ToolbarView,
    /// Back to the chat list, when one pane is shown at a time.
    back: gtk::Button,
    content_title: gtk::Label,
    /// Why WhatsApp stopped the session, under the conversation's header.
    strip: gtk::Label,
    /// How the panes are laid out at the window's width now.
    layout: Cell<Layout>,
    /// The conversation is the pane shown when there is room for one pane only.
    show_content: Cell<bool>,
    /// Which chats the list shows.
    filter: Cell<Filter>,
    /// The filter the open conversation was opened from in the list, if it was.
    opened_from: Cell<Option<Filter>>,
    chats: gtk::ListBox,
    /// What the chat list says when the filter keeps no chat.
    chats_empty: Empty,
    transcript: gtk::ListBox,
    /// What the transcript says when it has no message.
    transcript_empty: Empty,
    transcript_scroll: gtk::ScrolledWindow,
    /// Keep the conversation scrolled to the newest message as it grows.
    stick: Cell<bool>,
    /// The search is open while this (the conversation header's ⌕) is active.
    search_toggle: gtk::ToggleButton,
    /// The search input's row, under the conversation header.
    search_row: gtk::Box,
    search_entry: gtk::SearchEntry,
    /// What the search does, while its input is empty and focused.
    search_hint: gtk::Label,
    search_status: gtk::Label,
    results: gtk::ListBox,
    results_scroll: gtk::ScrolledWindow,
    blocked: adw::Banner,
    reply_bar: gtk::Box,
    reply_label: gtk::Label,
    composer: gtk::TextView,
    composer_scroll: gtk::ScrolledWindow,
    send_button: gtk::Button,
    /// What is drawn now, to change only what changed.
    /// The chat rows drawn, each with its position in the model's chat list.
    shown_chats: RefCell<Vec<(usize, ChatRow)>>,
    shown_bubbles: RefCell<Vec<Bubble>>,
    shown_results: RefCell<Vec<String>>,
    /// The highlighted chat's row at the last render.
    highlighted: Cell<Option<usize>>,
    /// The model's search was open at the last render.
    searching: Cell<bool>,
    /// The draft is being set from code, not typed.
    setting_draft: Cell<bool>,
    /// The decoded pictures, as textures ([`crate::pictures`]).
    pictures: RefCell<Cache<Drawn>>,
    /// The threads that decode pictures; what they decode comes back on the main loop.
    decoder: Decoder,
    /// The picture widgets drawn now, by chat and message id, to put a texture in once decoded.
    live: RefCell<HashMap<(String, String), Vec<Slot>>>,
    /// The bubbles the render in progress (re)built: their pictures are asked for.
    built: RefCell<Vec<Bubble>>,
    /// The last toast shown (tests).
    last_toast: RefCell<Option<String>>,
}

/// The rail's widgets that change: the filters with their counts, the connection line, and what
/// the icon strip hides or shows.
struct Rail {
    root: gtk::Box,
    /// New message, the one filled button.
    primary: gtk::Button,
    /// Its icon on the icon strip.
    primary_icon: gtk::Image,
    filters: Vec<(Filter, gtk::ToggleButton, gtk::Label)>,
    linked_mark: gtk::Box,
    connection: gtk::Box,
    connection_word: gtk::Label,
    /// Shown with the labels; hidden on the icon strip.
    texts: Vec<gtk::Widget>,
    /// Shown on the icon strip only.
    icons: Vec<gtk::Image>,
}

impl Rail {
    /// The rail (plan §5): the one filled button, the three flat filters with their counts right
    /// aligned, and the connection line at the bottom.
    fn new() -> Self {
        let mut texts: Vec<gtk::Widget> = Vec::new();
        let mut icons = Vec::new();
        // An icon (the strip's) and a label (the full rail's), one shown at a time.
        let mut content = |icon: &str, text: &str, texts: &mut Vec<gtk::Widget>| {
            let image = gtk::Image::from_icon_name(icon);
            image.set_visible(false);
            let label = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .hexpand(true)
                .build();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&image);
            row.append(&label);
            icons.push(image);
            texts.push(label.upcast());
            row
        };

        // New message (S6): it asks for a phone number; the palette stays on F1 and Ctrl+K.
        let primary = gtk::Button::builder()
            .child(&content("list-add-symbolic", NEW_MESSAGE, &mut texts))
            .tooltip_text(NEW_MESSAGE)
            .action_name("win.new-message")
            .build();
        primary.add_css_class("nuncio-primary");
        primary.update_property(&[Property::Label(NEW_MESSAGE)]);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("nuncio-chrome");
        root.add_css_class("nuncio-rail");
        root.append(&primary);
        let filter_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        filter_box.set_margin_top(12);
        let mut filters = Vec::new();
        let mut group: Option<gtk::ToggleButton> = None;
        for filter in Filter::EACH {
            let icon = match filter {
                Filter::All => "view-list-symbolic",
                Filter::Unread => "mail-unread-symbolic",
                Filter::Archived => "folder-symbolic",
            };
            let row = content(icon, filter.name(), &mut texts);
            let count = gtk::Label::new(None);
            count.add_css_class("nuncio-meta");
            row.append(&count);
            texts.push(count.clone().upcast());
            let button = gtk::ToggleButton::builder()
                .child(&row)
                .tooltip_text(filter.name())
                .active(filter == Filter::All)
                .build();
            button.add_css_class("nuncio-filter");
            button.set_group(group.as_ref());
            group.get_or_insert_with(|| button.clone());
            filter_box.append(&button);
            filters.push((filter, button, count));
        }
        root.append(&filter_box);

        let linked_mark = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        linked_mark.add_css_class("nuncio-linked-mark");
        let connection_word = gtk::Label::builder().xalign(0.0).build();
        connection_word.add_css_class("nuncio-meta");
        texts.push(connection_word.clone().upcast());
        let connection = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .vexpand(true)
            .valign(gtk::Align::End)
            .margin_start(8)
            .margin_bottom(4)
            .accessible_role(gtk::AccessibleRole::Status)
            .build();
        connection.append(&linked_mark);
        connection.append(&connection_word);
        root.append(&connection);

        // The first icon made is the button's.
        let primary_icon = icons.first().cloned().unwrap_or_default();
        Self {
            root,
            primary,
            primary_icon,
            filters,
            linked_mark,
            connection,
            connection_word,
            texts,
            icons,
        }
    }

    /// The labels, or the 48 px icon strip.
    fn set_compact(&self, compact: bool) {
        for text in &self.texts {
            text.set_visible(!compact);
        }
        for icon in &self.icons {
            icon.set_visible(compact);
        }
    }
}

/// A picture widget drawn now.
struct Slot {
    picture: glib::WeakRef<gtk::Picture>,
    sticker: bool,
    /// Its box came from the size the message states; else from the picture once decoded.
    sized: bool,
    /// How many frames the picture in it has (one when it does not animate).
    frames: Cell<usize>,
    /// The tick that plays its animation, stopped before another picture goes in (and gone once
    /// it stopped itself).
    ticking: Rc<RefCell<Option<gtk::TickCallbackId>>>,
}

impl Slot {
    fn new(picture: &gtk::Picture, sticker: bool, sized: bool) -> Self {
        Self {
            picture: picture.downgrade(),
            sticker,
            sized,
            frames: Cell::new(0),
            ticking: Rc::default(),
        }
    }

    /// Put `drawn` in its picture: its first frame, and an animation plays it from there unless
    /// the desktop asks for no animations (`gtk-enable-animations`). The setting is read when a
    /// sticker is drawn: turned off, a playing sticker stops at its first frame; turned on, a
    /// sticker plays the next time it is drawn (its row rebuilt), not before.
    fn show(&self, picture: &gtk::Picture, drawn: &Drawn) {
        if let Some(tick) = self.ticking.take() {
            tick.remove();
        }
        picture.set_paintable(Some(&drawn.first));
        self.frames
            .set(drawn.animation.as_ref().map_or(1, |a| a.textures.len()));
        let Some(animation) = &drawn.animation else {
            return;
        };
        if !picture.settings().is_gtk_enable_animations() {
            return;
        }
        let animation = Rc::clone(animation);
        let started = Cell::new(None::<i64>);
        let shown = Cell::new(0_usize);
        let ticking = Rc::downgrade(&self.ticking);
        let tick = picture.add_tick_callback(move |picture, clock| {
            if !picture.settings().is_gtk_enable_animations() {
                picture.set_paintable(animation.textures.first());
                // Breaking removes this tick: its id goes with it.
                if let Some(ticking) = ticking.upgrade() {
                    ticking.borrow_mut().take();
                }
                return glib::ControlFlow::Break;
            }
            // Microseconds, from the frame clock: the frame painted next.
            let now = clock.frame_time();
            let start = started.get().unwrap_or(now);
            started.set(Some(start));
            let elapsed = u64::try_from(now.saturating_sub(start) / 1000).unwrap_or(0);
            let index = pictures::frame_at(&animation.delays_ms, elapsed);
            if index != shown.get()
                && let Some(texture) = animation.textures.get(index)
            {
                shown.set(index);
                picture.set_paintable(Some(texture));
            }
            glib::ControlFlow::Continue
        });
        self.ticking.replace(Some(tick));
    }
}

/// A decoded picture as the window draws it: its first frame, and an animated sticker's frames.
#[derive(Clone)]
struct Drawn {
    first: gdk::Texture,
    animation: Option<Rc<Animation>>,
}

/// An animation's frames, one texture each built once, and how long each shows.
struct Animation {
    textures: Vec<gdk::Texture>,
    delays_ms: Vec<u32>,
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
            // The smallest single pane: below it the window would clip its content.
            .width_request(360)
            .height_request(320)
            .build();
        if let Some(app) = app {
            window.set_application(Some(app));
        }
        window.add_css_class(WINDOW_CLASS);
        if !title_bar {
            window.add_css_class("nuncio-no-title-bar");
        }

        let rail = Rail::new();

        // The chat list, with its header (its title and the menu).
        let list_title = pane_title("Chats");
        let list_header = header_bar(&list_title);
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
        list_header.pack_end(&menu_button);
        let chats = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        chats.add_css_class("navigation-sidebar");
        chats.update_property(&[Property::Label("Chats")]);
        chats.add_css_class("nuncio-chats");
        let chats_empty = Empty::new("system-users-symbolic");
        chats.set_placeholder(Some(&chats_empty.root));
        let chats_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&chats)
            .build();
        let list_view = adw::ToolbarView::new();
        list_view.add_css_class("nuncio-chrome");
        list_view.add_top_bar(&list_header);
        list_view.set_content(Some(&chats_scroll));

        // The conversation: its header holds only the chat's name (the connection is the rail's).
        let content_title = pane_title("No chat");
        content_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let content_header = header_bar(&content_title);
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back to the chats (Escape)")
            .visible(false)
            .build();
        back.update_property(&[Property::Label("Back to the chats")]);
        content_header.pack_start(&back);
        // A stop reason: one line under the header, in the warning colour, while it holds.
        let strip = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .visible(false)
            .build();
        strip.add_css_class("nuncio-strip");
        // The search of the open chat (plan §6.6, S7): the header's ⌕ opens a 32 px input under
        // the header, with its hint under it while it is empty and focused; the results show
        // above the transcript. It searches as the daemon does: every word must be in a message.
        let search_toggle = gtk::ToggleButton::builder()
            .icon_name("edit-find-symbolic")
            .tooltip_text("Search in this chat (/ or Ctrl+F)")
            .build();
        search_toggle.update_property(&[Property::Label("Search in this chat")]);
        content_header.pack_end(&search_toggle);
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search this chat")
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        search_entry.add_css_class("nuncio-search");
        search_entry.update_property(&[Property::Label("Search this chat")]);
        let search_row = gtk::Box::builder().visible(false).build();
        search_row.add_css_class("nuncio-search-row");
        search_row.append(&search_entry);
        let search_hint = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        search_hint.add_css_class("nuncio-meta");
        search_hint.add_css_class("nuncio-search-hint");

        let search_status = gtk::Label::builder().xalign(0.0).visible(false).build();
        search_status.add_css_class("nuncio-meta");
        search_status.add_css_class("nuncio-search-hint");
        let results = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        results.update_property(&[Property::Label("Search results")]);
        results.add_css_class("navigation-sidebar");
        results.add_css_class("nuncio-results");
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
        let transcript_empty = Empty::new("mail-unread-symbolic");
        transcript.set_placeholder(Some(&transcript_empty.root));
        let transcript_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&transcript)
            .build();

        let blocked = adw::Banner::new("");
        // The composer (plan §6.4): a bar docked under the transcript, a 1 px line above it that
        // turns accent while the composer has the focus. A reply strip above the field, the
        // field growing to six lines, and Send at the right only while there is text.
        let reply_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        reply_label.add_css_class("nuncio-reply-quote");
        let cancel_reply = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Cancel the reply (Escape)")
            .valign(gtk::Align::Center)
            .build();
        cancel_reply.add_css_class("flat");
        cancel_reply.add_css_class("nuncio-icon-button");
        cancel_reply.update_property(&[Property::Label("Cancel the reply")]);
        let reply_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        reply_bar.add_css_class("nuncio-reply-strip");
        reply_bar.append(&reply_label);
        reply_bar.append(&cancel_reply);
        reply_bar.set_visible(false);

        let composer = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(style::COMPOSER_PAD)
            .bottom_margin(style::COMPOSER_PAD)
            .left_margin(8)
            .right_margin(8)
            .build();
        composer.update_property(&[Property::Label("Message")]);
        let composer_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(style::composer_max_height(0))
            .hexpand(true)
            .valign(gtk::Align::Center)
            .child(&composer)
            .build();
        let send_button = gtk::Button::builder()
            .label("Send")
            .tooltip_text(SEND_TOOLTIP)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        send_button.add_css_class("nuncio-send");
        let composer_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        composer_row.add_css_class("nuncio-composer-row");
        composer_row.append(&composer_scroll);
        composer_row.append(&send_button);
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom.add_css_class("nuncio-composer");
        bottom.append(&reply_bar);
        bottom.append(&composer_row);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&search_status);
        content.append(&results_scroll);
        content.append(&transcript_scroll);
        let content_view = adw::ToolbarView::new();
        content_view.add_css_class("nuncio-content");
        content_view.add_top_bar(&content_header);
        content_view.add_top_bar(&strip);
        content_view.add_top_bar(&search_row);
        content_view.add_top_bar(&search_hint);
        content_view.set_content(Some(&content));
        content_view.add_bottom_bar(&blocked);
        content_view.add_bottom_bar(&bottom);

        // A draggable seam between each two panes, drawn as a 1 px line; the panes fill the
        // window, with no outer margin.
        let lists = seam(&list_view, &content_view, LIST_WIDTH);
        let panes = seam(&rail.root, &lists, RAIL_WIDTH);
        // The palette's scrim (plan §6.5): the window dimmed behind it while it is open; the
        // palette's own grab takes the clicks.
        let scrim = gtk::Box::builder()
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .visible(false)
            .build();
        scrim.add_css_class("nuncio-scrim");
        let dimmed = gtk::Overlay::builder().child(&panes).build();
        dimmed.add_overlay(&scrim);
        let toasts = adw::ToastOverlay::new();
        toasts.add_css_class(ROOT_CLASS);
        toasts.set_child(Some(&dimmed));
        window.set_content(Some(&toasts));

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
            list_header,
            content_header,
            title_bar: Cell::new(title_bar),
            shortcuts,
            palette: RefCell::new(None),
            new_message: RefCell::new(None),
            scrim,
            config_path: RefCell::new(None),
            toasts,
            panes,
            lists,
            rail,
            list_view,
            list_title,
            content_view,
            back,
            content_title,
            strip,
            layout: Cell::new(Layout::ThreePanes),
            show_content: Cell::new(false),
            filter: Cell::new(Filter::All),
            opened_from: Cell::new(None),
            chats,
            chats_empty,
            transcript,
            transcript_empty,
            transcript_scroll,
            stick: Cell::new(true),
            search_toggle,
            search_row,
            search_entry,
            search_hint,
            search_status,
            results,
            results_scroll,
            blocked,
            reply_bar,
            reply_label,
            composer,
            composer_scroll,
            send_button: send_button.clone(),
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
        ui.add_breakpoints();
        ui.apply_layout(Layout::ThreePanes);
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
        self.0.opened_from.set(None);
        if self.0.controller.borrow().model().is_open() {
            self.set_show_content(true);
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

    /// What is drawn around message `index`'s bubble: its day label, author line and meta line
    /// (tests).
    pub fn message_parts(&self, index: usize) -> Option<MessageParts> {
        let row = self.0.transcript.row_at_index(i32::try_from(index).ok()?)?;
        let first = |class: &str| {
            let mut found = Vec::new();
            texts_with_class(row.upcast_ref(), class, &mut found);
            found.into_iter().next()
        };
        Some(MessageParts {
            day: first("nuncio-day"),
            author: first("nuncio-author-line"),
            meta: first("nuncio-meta-line"),
        })
    }

    /// What the chat list and the transcript say while empty, when they do (tests).
    pub fn empty_states(&self) -> (Option<EmptyTexts>, Option<EmptyTexts>) {
        let shows = |list: &gtk::ListBox, empty: &Empty| {
            (list.row_at_index(0).is_none())
                .then(|| empty.texts())
                .flatten()
        };
        (
            shows(&self.0.chats, &self.0.chats_empty),
            shows(&self.0.transcript, &self.0.transcript_empty),
        )
    }

    /// The conversation header's title (tests).
    pub fn conversation_title(&self) -> String {
        self.0.content_title.text().to_string()
    }

    /// The strip under the conversation header, while it is shown (tests).
    pub fn stop_strip(&self) -> Option<String> {
        self.0
            .strip
            .get_visible()
            .then(|| self.0.strip.text().to_string())
    }

    /// The position of the selected row in the chat list, as drawn (tests).
    pub fn selected_chat_row(&self) -> Option<usize> {
        let row = self.0.chats.selected_row()?;
        usize::try_from(row.index()).ok()
    }

    /// The count the rail shows next to `filter` (tests).
    pub fn filter_count(&self, filter: Filter) -> String {
        self.0
            .rail
            .filters
            .iter()
            .find(|(f, ..)| *f == filter)
            .map(|(_, _, count)| count.text().to_string())
            .unwrap_or_default()
    }

    /// The rail's connection word (tests).
    pub fn connection_word(&self) -> String {
        self.0.rail.connection_word.text().to_string()
    }

    /// List the chats `filter` keeps, as clicking it in the rail does.
    pub fn set_filter(&self, filter: Filter) {
        if let Some((_, button, _)) = self.0.rail.filters.iter().find(|(f, ..)| *f == filter) {
            // Its `toggled` handler renders.
            button.set_active(true);
        }
    }

    /// Lay the panes out as at a window this wide ([`style::layout_for`]), as the window's
    /// breakpoints do when it is resized.
    pub fn apply_layout(&self, layout: Layout) {
        let inner = &self.0;
        inner.layout.set(layout);
        let single = layout == Layout::SinglePane;
        inner.rail.root.set_visible(!single);
        inner.rail.set_compact(layout == Layout::CompactRail);
        inner.panes.set_position(if layout == Layout::CompactRail {
            RAIL_COMPACT_WIDTH
        } else {
            RAIL_WIDTH
        });
        inner.lists.set_position(LIST_WIDTH);
        inner.back.set_visible(single);
        self.show_panes();
    }

    /// The panes laid out now (tests).
    pub fn layout(&self) -> Layout {
        self.0.layout.get()
    }

    /// Which of the rail, the chat list and the conversation are shown (tests).
    pub fn panes_shown(&self) -> (bool, bool, bool) {
        let inner = &self.0;
        (
            inner.rail.root.get_visible(),
            inner.list_view.get_visible(),
            inner.content_view.get_visible(),
        )
    }

    /// The window's breakpoints ([`style::breakpoints`]): each narrower layout applies while its
    /// condition holds; none applied is three panes.
    fn add_breakpoints(&self) {
        for (condition, layout) in style::breakpoints() {
            let Ok(condition) = adw::BreakpointCondition::parse(&condition) else {
                continue;
            };
            let breakpoint = adw::Breakpoint::new(condition);
            let apply = self.with(move |ui, (): ()| ui.apply_layout(layout));
            breakpoint.connect_apply(move |_| apply(()));
            // Unapplied first or after the next one applies: only the layout it set is undone.
            let unapply = self.with(move |ui, (): ()| {
                if ui.0.layout.get() == layout {
                    ui.apply_layout(Layout::ThreePanes);
                }
            });
            breakpoint.connect_unapply(move |_| unapply(()));
            self.0.window.add_breakpoint(breakpoint);
        }
    }

    /// Show the conversation (true) or the chat list when there is room for one pane only.
    fn set_show_content(&self, shown: bool) {
        self.0.show_content.set(shown);
        self.show_panes();
    }

    /// Which panes are visible, and which header carries the window controls: with one pane the
    /// shown one holds them all; otherwise the list's header the start ones, the conversation's
    /// the end ones.
    fn show_panes(&self) {
        let inner = &self.0;
        let single = inner.layout.get() == Layout::SinglePane;
        let content = inner.show_content.get();
        inner.list_view.set_visible(!single || !content);
        inner.content_view.set_visible(!single || content);
        let shown = inner.title_bar.get();
        inner.list_header.set_show_start_title_buttons(shown);
        inner
            .list_header
            .set_show_end_title_buttons(shown && single);
        inner
            .content_header
            .set_show_start_title_buttons(shown && single);
        inner.content_header.set_show_end_title_buttons(shown);
    }

    /// Back from the conversation to the chat list, when one pane is shown at a time (Escape, or
    /// the header's back button); true when it went back.
    fn go_back(&self) -> bool {
        if self.0.layout.get() != Layout::SinglePane || !self.0.show_content.get() {
            return false;
        }
        self.set_show_content(false);
        self.0.controller.borrow_mut().back_to_list();
        self.render();
        self.0.chats.grab_focus();
        true
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
        self.0.opened_from.set(Some(self.0.filter.get()));
        self.set_show_content(true);
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

    /// Show or hide the window controls in the header bars.
    pub fn set_title_bar(&self, shown: bool) {
        self.0.title_bar.set(shown);
        self.show_panes();
        if shown {
            self.0.window.remove_css_class("nuncio-no-title-bar");
        } else {
            self.0.window.add_css_class("nuncio-no-title-bar");
        }
    }

    /// Whether the header bars show the window controls (tests).
    pub fn title_bar_shown(&self) -> bool {
        self.0.content_header.shows_end_title_buttons()
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
        // One dialog at a time (plan §6.8).
        self.close_new_message();
        // Only a message the user can see is chosen: the focused bubble.
        let message = self.focused_message().map(|(index, _)| index);
        let (entries, conversation) = {
            let controller = self.0.controller.borrow();
            let model = controller.model();
            (palette::entries(model, message), open_conversation(model))
        };
        let width = self.0.toasts.width();
        let view = Rc::new(PaletteView::new(
            entries,
            conversation,
            style::palette_width(width),
        ));
        view.popover.set_parent(&self.0.toasts);
        // Centred, a little under the top edge.
        view.popover
            .set_pointing_to(Some(&gdk::Rectangle::new(width / 2, 48, 1, 1)));

        view.entry.connect_changed({
            let view = Rc::downgrade(&view);
            move |entry| {
                if let Some(view) = view.upgrade() {
                    view.filter(&entry.text());
                }
            }
        });
        // The × at the right of the input closes the palette.
        view.entry.connect_icon_press({
            let close = self.with(|ui, (): ()| ui.close_palette());
            move |_, position| {
                if position == gtk::EntryIconPosition::Secondary {
                    close(());
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = self.weak();
            move |_, key, _, _| {
                if weak
                    .upgrade()
                    .is_some_and(|inner| Ui(inner).palette_key(key))
                {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
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
                        inner.scrim.set_visible(false);
                    }
                }
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            }
        });

        view.filter("");
        *self.0.palette.borrow_mut() = Some(Rc::clone(&view));
        self.0.scrim.set_visible(true);
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
        self.0.scrim.set_visible(false);
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
        self.close_palette();
    }

    /// A key pressed in the open palette's input, as pressing it there does: true when the
    /// palette acted on it (Up/Down, Enter, Escape), false when it is typed.
    pub fn palette_key(&self, key: gdk::Key) -> bool {
        let Some(action) = palette_key_action(key) else {
            return false;
        };
        let Some(view) = self.0.palette.borrow().clone() else {
            return false;
        };
        match action {
            PaletteKey::Up => view.move_selection(-1),
            PaletteKey::Down => view.move_selection(1),
            PaletteKey::Run => self.palette_enter(),
            PaletteKey::Close => self.close_palette(),
        }
        true
    }

    /// The open palette's selected command (tests).
    pub fn palette_selected(&self) -> Option<String> {
        let view = self.0.palette.borrow().clone()?;
        let position = usize::try_from(view.list.selected_row()?.index()).ok()?;
        let index = *view.shown.borrow().get(position)?;
        Some(view.entries[index].title.clone())
    }

    /// The open palette's group headers, in order (tests).
    pub fn palette_headers(&self) -> Vec<String> {
        let Some(view) = self.0.palette.borrow().clone() else {
            return Vec::new();
        };
        (0..)
            .map_while(|i| view.list.row_at_index(i))
            .filter_map(|row| row.header()?.downcast::<gtk::Label>().ok())
            .map(|header| header.text().to_string())
            .collect()
    }

    /// The open palette's width, in pixels (tests).
    pub fn palette_width(&self) -> i32 {
        self.0
            .palette
            .borrow()
            .as_ref()
            .map_or(0, |view| view.column.width_request())
    }

    /// Whether the window is dimmed behind the palette (tests).
    pub fn palette_scrim_shown(&self) -> bool {
        self.0.scrim.get_visible()
    }

    /// Ask for a phone number ("New message", the rail's button or the palette): a §6.8 dialog
    /// whose Enter opens its chat ([`Ui::confirm_new_message`]) and whose Escape cancels.
    pub fn open_new_message(&self) {
        if let Some(view) = self.0.new_message.borrow().as_ref() {
            view.entry.grab_focus();
            return;
        }
        // One dialog at a time (plan §6.8).
        self.close_palette();
        let width = self.0.toasts.width();
        let view = Rc::new(NewMessageView::new(style::dialog_width(width)));
        view.popover.set_parent(&self.0.toasts);
        // Centred, a little under the top edge, where the palette opens.
        view.popover
            .set_pointing_to(Some(&gdk::Rectangle::new(width / 2, 48, 1, 1)));
        // Typing again clears the error.
        view.entry.connect_changed({
            let view = Rc::downgrade(&view);
            move |_| {
                if let Some(view) = view.upgrade() {
                    view.error.set_visible(false);
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = self.weak();
            move |_, key, _, _| {
                if weak
                    .upgrade()
                    .is_some_and(|inner| Ui(inner).new_message_key(key))
                {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        view.entry.add_controller(keys);
        view.open.connect_clicked({
            let confirm = self.with(|ui, (): ()| ui.confirm_new_message());
            move |_| confirm(())
        });
        view.cancel.connect_clicked({
            let close = self.with(|ui, (): ()| ui.close_new_message());
            move |_| close(())
        });
        view.popover.connect_closed({
            let weak = self.weak();
            move |popover| {
                if let Some(inner) = weak.upgrade() {
                    let mut open = inner.new_message.borrow_mut();
                    if open.as_ref().is_some_and(|view| &view.popover == popover) {
                        open.take();
                    }
                }
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            }
        });
        *self.0.new_message.borrow_mut() = Some(Rc::clone(&view));
        view.popover.popup();
        view.entry.grab_focus();
    }

    /// Close the "New message" dialog, if it is open; nothing is opened.
    fn close_new_message(&self) {
        let Some(view) = self.0.new_message.borrow_mut().take() else {
            return;
        };
        view.popover.popdown();
        // A dialog never mapped (a hidden window) is not `closed`: let it go here too.
        let popover = view.popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    }

    /// The number typed: its chat opens as its link with no text does
    /// ([`Controller::new_message`]), the dialog closes and the composer takes the focus. An
    /// invalid number opens nothing: the dialog stays open and says why.
    fn confirm_new_message(&self) {
        let Some(view) = self.0.new_message.borrow().clone() else {
            return;
        };
        let written = view.entry.text();
        let result = self.0.controller.borrow_mut().new_message(&written);
        match result {
            Ok(commands) => {
                self.close_new_message();
                self.show_conversation(commands);
            }
            Err(error) => {
                view.error.set_text(error);
                view.error.set_visible(true);
                view.entry.grab_focus();
            }
        }
    }

    /// A key pressed in the "New message" field, as pressing it there does: true when the
    /// dialog acted on it (Enter confirms, Escape cancels), false when it is typed.
    pub fn new_message_key(&self, key: gdk::Key) -> bool {
        if self.0.new_message.borrow().is_none() {
            return false;
        }
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                self.confirm_new_message();
            }
            gdk::Key::Escape => self.close_new_message(),
            _ => return false,
        }
        true
    }

    /// Whether the "New message" dialog is open (tests).
    pub fn new_message_open(&self) -> bool {
        self.0.new_message.borrow().is_some()
    }

    /// Type `number` into the open "New message" field, replacing what was there.
    pub fn new_message_type(&self, number: &str) {
        let entry = self
            .0
            .new_message
            .borrow()
            .as_ref()
            .map(|v| v.entry.clone());
        if let Some(entry) = entry {
            entry.set_text(number);
        }
    }

    /// What the open "New message" dialog says is wrong, if it says anything (tests).
    pub fn new_message_error(&self) -> Option<String> {
        let view = self.0.new_message.borrow().clone()?;
        view.error
            .get_visible()
            .then(|| view.error.text().to_string())
    }

    /// The open "New message" dialog's width, in pixels (tests).
    pub fn new_message_width(&self) -> i32 {
        self.0
            .new_message
            .borrow()
            .as_ref()
            .map_or(0, |view| view.column.width_request())
    }

    /// Whether the "New message" field has the keyboard focus (tests).
    pub fn new_message_focused(&self) -> bool {
        let Some(view) = self.0.new_message.borrow().clone() else {
            return false;
        };
        self.focused_widget().is_some_and(|widget| {
            &widget == view.entry.upcast_ref::<gtk::Widget>() || widget.is_ancestor(&view.entry)
        })
    }

    /// The rail's filled button: its label, its tooltip, and whether it carries an accessible
    /// name (tests; GTK gives no way to read the name back).
    pub fn rail_primary(&self) -> (String, Option<String>, bool) {
        let primary = &self.0.rail.primary;
        let label = self
            .0
            .rail
            .texts
            .first()
            .and_then(|text| text.downcast_ref::<gtk::Label>())
            .map(|label| label.text().to_string())
            .unwrap_or_default();
        (
            label,
            primary.tooltip_text().map(|t| t.to_string()),
            gtk::test_accessible_has_property(primary, gtk::AccessibleProperty::Label),
        )
    }

    /// Whether the rail's filled button shows its icon (the 48 px icon strip; tests).
    pub fn rail_primary_icon_shown(&self) -> bool {
        self.0.rail.primary_icon.get_visible()
    }

    /// Click the rail's filled button (tests).
    pub fn click_rail_primary(&self) {
        self.0.rail.primary.emit_clicked();
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
            Step::Search => {
                self.start_search();
            }
            Step::NewMessage => self.open_new_message(),
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
        // Plan §6.7: one 480 px column of 40 px rows (label left, libadwaita's control right),
        // each section under a `meta` header with a 1 px rule; help text as a second line.
        let window = adw::Window::builder()
            .title("Settings")
            .modal(true)
            .transient_for(&self.0.window)
            .default_width(SETTINGS_WIDTH)
            .default_height(620)
            .build();
        window.add_css_class(ROOT_CLASS);
        window.add_css_class(WINDOW_CLASS);
        window.add_css_class("nuncio-settings");
        // Escape closes it, as it closed the preferences window before.
        let keys = gtk::ShortcutController::new();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("window.close")),
        ));
        window.add_controller(keys);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.set_margin_start(16);
        column.set_margin_end(16);
        column.set_margin_bottom(24);
        let problem = prefs.borrow().problem().map(str::to_owned);
        if let Some(problem) = &problem {
            column.append(&settings_header(
                "The configuration file cannot be changed here",
            ));
            let text = plain_label(problem);
            text.add_css_class("nuncio-error");
            text.set_margin_top(8);
            column.append(&text);
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
        let toasts = adw::ToastOverlay::new();
        let mut view = SettingsView {
            window,
            notifications,
            show_preview,
            background,
            start_hidden,
            title_bar,
            image_protocol,
            problem,
            toasts: toasts.clone(),
            headers: Vec::new(),
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
            let header = settings_header(title);
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .accessible_role(gtk::AccessibleRole::List)
                .build();
            list.add_css_class("nuncio-settings-list");
            list.update_property(&[Property::Label(title)]);
            for row in rows {
                row.add_css_class("nuncio-setting");
                list.append(&row);
            }
            list.set_sensitive(editable);
            column.append(&header);
            column.append(&list);
            view.headers.push(header);
        }
        let saved_to = match prefs.borrow().path() {
            Some(path) => format!(
                "Saved to {} after each change. Comments in that file are not kept.",
                path.display()
            ),
            None => "There is no configuration file.".to_owned(),
        };
        let footer_text = plain_label(&saved_to);
        footer_text.add_css_class("nuncio-meta");
        footer_text.set_margin_top(24);
        column.append(&footer_text);
        let clamp = adw::Clamp::builder()
            .maximum_size(SETTINGS_WIDTH)
            .tightening_threshold(SETTINGS_WIDTH)
            .child(&column)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let header_bar = adw::HeaderBar::new();
        header_bar.add_css_class("nuncio-headerbar");
        header_bar.add_css_class("nuncio-header");
        let shell = adw::ToolbarView::new();
        shell.add_top_bar(&header_bar);
        shell.set_content(Some(&scroll));
        toasts.set_child(Some(&shell));
        view.window.set_content(Some(&toasts));
        let view = view;

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

        // A row's position in the list is not the chat's in the model while a filter is on.
        inner.chats.connect_row_activated({
            let f = self.with(|ui, row: usize| {
                let chat = ui.0.shown_chats.borrow().get(row).map(|(chat, _)| *chat);
                if let Some(chat) = chat {
                    ui.open_chat(chat);
                }
            });
            move |_, row| f(usize::try_from(row.index()).unwrap_or(usize::MAX))
        });
        inner.back.connect_clicked({
            let f = self.with(|ui, (): ()| {
                ui.go_back();
            });
            move |_| f(())
        });
        for (filter, button, _) in &inner.rail.filters {
            let filter = *filter;
            let f = self.with(move |ui, active: bool| {
                if active && ui.0.filter.replace(filter) != filter {
                    ui.render();
                }
            });
            button.connect_toggled(move |button| f(button.is_active()));
        }

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
            let weak = self.weak();
            move |_, key, _, state| {
                let handled = weak
                    .upgrade()
                    .is_some_and(|inner| Ui(inner).composer_press(key, state));
                if handled {
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
                if message_key_handled(key, state) || list_key_action(key, state).is_some() {
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
        inner.search_toggle.connect_toggled({
            let f = self.with(|ui, on: bool| ui.set_search_mode(on));
            move |toggle| f(toggle.is_active())
        });
        inner.search_entry.connect_stop_search({
            let f = self.with(|ui, (): ()| ui.close_search());
            move |_| f(())
        });
        inner.search_entry.connect_changed({
            let f = self.with(|ui, (): ()| ui.update_search_hint());
            move |_| f(())
        });
        // The hint follows the focus once it has moved.
        let focus = gtk::EventControllerFocus::new();
        let refocused = Rc::new(self.with(|ui, (): ()| {
            let later = ui.with(|ui, (): ()| ui.update_search_hint());
            glib::idle_add_local_once(move || later(()));
        }));
        focus.connect_enter({
            let refocused = Rc::clone(&refocused);
            move |_| refocused(())
        });
        focus.connect_leave(move |_| refocused(()));
        inner.search_entry.add_controller(focus);
        inner.results.connect_row_activated({
            let f = self.with(|ui, index: usize| ui.jump_to_result(index));
            move |_, row| f(usize::try_from(row.index()).unwrap_or(usize::MAX))
        });
        let open_search = self.with(|ui, (): ()| {
            ui.start_search();
        });
        inner.shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                open_search(());
                glib::Propagation::Stop
            })),
        ));
        // `/` opens the search too (plan §7), unless it is being typed in a text field.
        let weak = self.weak();
        inner.shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("slash"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if weak.upgrade().is_some_and(|inner| Ui(inner).slash()) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            })),
        ));
        // The chat list's keys: `j`/`k` move as Down/Up do; Escape closes the search.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let weak = self.weak();
            move |_, key, _, state| {
                let handled = weak
                    .upgrade()
                    .is_some_and(|inner| Ui(inner).chat_list_key(key, state));
                if handled {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        inner.chats.add_controller(keys);

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
            // The rail's New message: plan §7 gives it no key.
            (
                "new-message",
                &[][..],
                Box::new(self.with(|ui, (): ()| ui.open_new_message())),
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

    /// Open the search of the selected chat (Ctrl+F, `/`, the ⌕): true when there is one to
    /// search.
    fn start_search(&self) -> bool {
        if self.0.controller.borrow().model().selected_chat().is_none() {
            return false;
        }
        // Its `toggled` handler shows the input; already open, it only takes the focus.
        self.0.search_toggle.set_active(true);
        self.0.search_entry.grab_focus();
        self.update_search_hint();
        true
    }

    /// The search opens (its input shows under the conversation header) or closes (the input
    /// and the model's search go).
    fn set_search_mode(&self, on: bool) {
        let inner = &self.0;
        if on && inner.controller.borrow().model().selected_chat().is_none() {
            inner.search_toggle.set_active(false);
            return;
        }
        inner.search_row.set_visible(on);
        if on {
            inner.search_entry.grab_focus();
        } else {
            inner.search_entry.set_text("");
            inner.controller.borrow_mut().close_search();
            self.render();
        }
        self.update_search_hint();
    }

    /// Show the search's hint only while its input is empty and has the focus (plan §6.6).
    fn update_search_hint(&self) {
        let inner = &self.0;
        let focused = self.focused_widget().is_some_and(|widget| {
            &widget == inner.search_entry.upcast_ref::<gtk::Widget>()
                || widget.is_ancestor(&inner.search_entry)
        });
        let hint = present::search_hint(
            inner.search_toggle.is_active(),
            &inner.search_entry.text(),
            focused,
        );
        inner.search_hint.set_text(hint.unwrap_or_default());
        inner.search_hint.set_visible(hint.is_some());
    }

    /// `/` pressed: it opens the search, unless a text field has the focus (the composer, the
    /// search input itself), where it is typed. True when it opened the search.
    fn slash(&self) -> bool {
        let typing = self.focused_widget().is_some_and(|widget| {
            widget.is::<gtk::TextView>() || widget.is::<gtk::Text>() || widget.is::<gtk::Entry>()
        });
        !typing && !self.palette_open() && self.start_search()
    }

    /// A key in the chat list: `j`/`k` move to the next or previous chat as Down/Up do (the
    /// chat is selected, not opened); Escape closes the search. True when it was handled.
    fn chat_list_key(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let inner = &self.0;
        if key == gdk::Key::Escape && inner.search_toggle.is_active() {
            self.close_search();
            return true;
        }
        let step = match list_key_action(key, state) {
            Some(ListKey::Down) => 1,
            Some(ListKey::Up) => -1,
            _ => return false,
        };
        let count = count_rows(&inner.chats);
        let at = inner
            .chats
            .focus_child()
            .and_then(|row| row.downcast::<gtk::ListBoxRow>().ok())
            .or_else(|| inner.chats.selected_row())
            .and_then(|row| usize::try_from(row.index()).ok());
        let next = at.map_or(0, |at| palette::moved(at, step, count));
        if let Some(row) = i32::try_from(next)
            .ok()
            .and_then(|i| inner.chats.row_at_index(i))
        {
            inner.chats.select_row(Some(&row));
            row.grab_focus();
        }
        true
    }

    /// Whether the search is open (tests).
    pub fn search_open(&self) -> bool {
        self.0.search_toggle.is_active()
    }

    /// Close the search, as Escape in it does.
    pub fn close_search(&self) {
        self.0.search_toggle.set_active(false);
    }

    /// Type `query` into the open search, replacing what was there (tests).
    pub fn type_search(&self, query: &str) {
        self.0.search_entry.set_text(query);
    }

    /// Search for what is typed, as Enter in the search does (tests).
    pub fn submit_search(&self) {
        self.0.search_entry.emit_activate();
    }

    /// The search's hint, while it is shown (tests).
    pub fn search_hint(&self) -> Option<String> {
        let hint = &self.0.search_hint;
        hint.get_visible().then(|| hint.text().to_string())
    }

    /// Whether the chat list's header shows its title (tests).
    pub fn list_title_shown(&self) -> bool {
        self.0.list_title.get_visible()
    }

    /// Whether the search's input is shown in the conversation pane (tests).
    pub fn search_in_conversation(&self) -> bool {
        self.0.search_row.get_visible() && self.0.search_entry.is_ancestor(&self.0.content_view)
    }

    /// Whether the search's results are shown, above the transcript (tests).
    pub fn results_shown(&self) -> bool {
        self.0.results_scroll.get_visible()
    }

    /// Whether the composer has the focus (tests).
    pub fn composer_focused(&self) -> bool {
        self.focused_widget()
            .is_some_and(|widget| &widget == self.0.composer.upcast_ref::<gtk::Widget>())
    }

    /// The widget with the keyboard focus in the window, if one has it.
    fn focused_widget(&self) -> Option<gtk::Widget> {
        RootExt::focus(&self.0.window)
    }

    /// Put the focus in the chat list, as Tab to it does (tests).
    pub fn focus_chat_list(&self) {
        self.0.chats.grab_focus();
    }

    /// A key pressed in the composer, as pressing it there does: true when the composer acted on
    /// it (Enter and Ctrl+Enter send, Alt+Enter or Shift+Enter start a line, Escape cancels the
    /// reply or leaves), false when GTK's text view takes it.
    pub fn composer_press(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let Some(action) = composer_key_action(key, state) else {
            return false;
        };
        match action {
            ComposerKey::Send => self.send(),
            ComposerKey::Newline => self.0.composer.buffer().insert_at_cursor("\n"),
            ComposerKey::Escape => {
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
                } else {
                    self.go_back();
                }
            }
        }
        true
    }

    /// The Send button's label and whether it can be pressed, or `None` while it is hidden
    /// (tests).
    pub fn send_button(&self) -> Option<(String, bool)> {
        let button = &self.0.send_button;
        button.get_visible().then(|| {
            (
                button.label().unwrap_or_default().to_string(),
                button.get_sensitive(),
            )
        })
    }

    /// The Send button's tooltip (tests).
    pub fn send_tooltip(&self) -> String {
        self.0
            .send_button
            .tooltip_text()
            .unwrap_or_default()
            .to_string()
    }

    /// The reply strip's quoted message, while it is shown (tests).
    pub fn reply_strip(&self) -> Option<String> {
        self.0
            .reply_bar
            .get_visible()
            .then(|| self.0.reply_label.text().to_string())
    }

    /// The composer field's tallest height and the height of one of its lines, in pixels
    /// (tests).
    pub fn composer_height_cap(&self) -> (i32, i32) {
        (
            self.0.composer_scroll.max_content_height(),
            composer_line_height(&self.0.composer),
        )
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
        if let Some(move_to) = list_key_action(key, state) {
            let count = count_rows(&self.0.transcript);
            let at = self.focused_message().map(|(index, _)| index);
            let next = match move_to {
                ListKey::Down => at.map_or(0, |at| palette::moved(at, 1, count)),
                ListKey::Up => at.map_or(0, |at| palette::moved(at, -1, count)),
                ListKey::Top => 0,
                ListKey::Bottom => count.saturating_sub(1),
            };
            if let Some(row) = i32::try_from(next)
                .ok()
                .and_then(|i| self.0.transcript.row_at_index(i))
            {
                row.grab_focus();
            }
            return;
        }
        if key == gdk::Key::Escape {
            if !self.go_back() {
                self.0.composer.grab_focus();
            }
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
        let outcome = outcome.map(|pixels| (pixels.bytes(), pixels.source, drawn(pixels)));
        let outcome = outcome.map(|(bytes, source, drawn)| (drawn, bytes, source));
        let shown = match &outcome {
            Ok((drawn, _, source)) => Ok((drawn.clone(), *source)),
            Err(error) => Err(error.clone()),
        };
        if !self.0.pictures.borrow_mut().finish(key.clone(), outcome) {
            return;
        }
        match shown {
            Ok((drawn, source)) => {
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
                    slot.show(&picture, &drawn);
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
        let live = self.0.live.borrow();
        let slot = live
            .values()
            .flatten()
            .find(|slot| slot.picture.upgrade().as_ref() == Some(&picture));
        Some(PictureState {
            reserved: picture.size_request(),
            laid_out: (width, height),
            texture: paintable.map(|p| (p.intrinsic_width(), p.intrinsic_height())),
            alt: picture.alternative_text().map(|t| t.to_string()),
            frames: slot.map_or(0, |slot| slot.frames.get()),
            animating: slot.is_some_and(|slot| slot.ticking.borrow().is_some()),
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
        self.close_search();
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
        let today = local_today();
        let model = controller.model();

        let status = present::rail_status(model);
        inner.rail.connection_word.set_text(&status.word);
        inner.rail.linked_mark.set_visible(status.linked);
        inner.rail.connection.set_tooltip_text(Some(&status.word));
        inner
            .rail
            .connection
            .update_property(&[Property::Label(&format!("Connection: {}", status.word))]);
        let title = present::conversation_title(model);
        inner.content_title.set_text(&title);
        let strip = present::stop_strip(model);
        inner.strip.set_text(strip.as_deref().unwrap_or_default());
        inner.strip.set_visible(strip.is_some());

        for (filter, button, count) in &inner.rail.filters {
            let listed = present::count(model, *filter);
            count.set_text(&listed.to_string());
            button.update_property(&[Property::Label(&format!(
                "{}, {listed} chats",
                filter.name()
            ))]);
        }
        let rows = present::chat_rows(model, offset, today);
        let listed = present::listed(model, inner.filter.get(), inner.opened_from.get());
        sync_rows(
            &inner.chats,
            &mut inner.shown_chats.borrow_mut(),
            listed
                .iter()
                .filter_map(|&chat| Some((chat, rows.get(chat)?.clone())))
                .collect(),
            |(_, chat)| chat_row(chat),
        );
        // The row to select: the highlighted chat's place in the list as filtered, when it moved,
        // or when its row was rebuilt in place and lost the selection. A row the keys moved the
        // selection to is left selected.
        let highlighted = model
            .chat_list()
            .highlighted()
            .and_then(|chat| listed.iter().position(|&shown| shown == chat));
        inner
            .chats_empty
            .show(Some(present::list_empty(inner.filter.get())));
        inner
            .transcript_empty
            .show(present::transcript_empty(model));
        let lost = highlighted.is_some() && inner.chats.selected_row().is_none();
        if inner.highlighted.replace(highlighted) != highlighted || lost {
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
            present::bubbles(model, offset, today),
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
            .send_button
            .set_visible(composer.send.label().is_some());
        if let Some(label) = composer.send.label() {
            inner.send_button.set_label(label);
        }
        inner
            .send_button
            .set_sensitive(composer.send == present::SendButton::Send);
        // Six lines of the font in use, then the field scrolls; measured here, so a font switch
        // is followed on the next render.
        let cap = style::composer_max_height(composer_line_height(&inner.composer));
        if inner.composer_scroll.max_content_height() != cap {
            inner.composer_scroll.set_max_content_height(cap);
        }
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
        // A draft the model changed (another chat opened with its own draft, a link's text) is
        // shown; a typed one is already there.
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
            inner.search_toggle.set_active(false);
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
    pub window: adw::Window,
    pub notifications: adw::SwitchRow,
    pub show_preview: adw::SwitchRow,
    pub background: adw::SwitchRow,
    pub start_hidden: adw::SwitchRow,
    pub title_bar: adw::ComboRow,
    pub image_protocol: adw::ComboRow,
    /// Why the file cannot be changed (shown at the top; every row is insensitive).
    pub problem: Option<String>,
    toasts: adw::ToastOverlay,
    /// The sections' headers, in order.
    headers: Vec<gtk::Label>,
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
        self.toasts.add_toast(toast);
    }

    /// The sections' headers as shown, in order (tests).
    pub fn headers(&self) -> Vec<String> {
        self.headers
            .iter()
            .map(|header| header.text().to_string())
            .collect()
    }
}

/// A settings section's header (plan §6.7): `meta` capitals over a 1 px rule; a screen reader
/// hears it as a heading, in the words' own case.
fn settings_header(title: &str) -> gtk::Label {
    let header = gtk::Label::builder()
        .label(title.to_uppercase())
        .xalign(0.0)
        .accessible_role(gtk::AccessibleRole::Heading)
        .build();
    header.add_css_class("nuncio-settings-header");
    header.update_property(&[Property::Label(title)]);
    header
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

/// The "New message" dialog (plan §6.8): a 480 px `surface` panel with a 1 px border, its title,
/// a line of body, the number field, the error (hidden until there is one), and Cancel and
/// Open chat bottom-right, the primary as an `accent` text button.
struct NewMessageView {
    popover: gtk::Popover,
    column: gtk::Box,
    entry: gtk::Entry,
    error: gtk::Label,
    open: gtk::Button,
    cancel: gtk::Button,
}

impl NewMessageView {
    fn new(width: i32) -> Self {
        let title = label(NEW_MESSAGE, "nuncio-title");
        let body = label(
            "Type a phone number with its country code. Its chat opens with an empty composer; \
             nothing is sent.",
            "nuncio-dialog-body",
        );
        body.set_wrap(true);
        let entry = gtk::Entry::builder()
            .placeholder_text("Phone number")
            .input_purpose(gtk::InputPurpose::Phone)
            .hexpand(true)
            .build();
        entry.add_css_class("nuncio-search");
        entry.update_property(&[Property::Label("Phone number, with its country code")]);
        let error = label("", "nuncio-dialog-error");
        error.set_visible(false);
        error.set_accessible_role(gtk::AccessibleRole::Alert);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("flat");
        let open = gtk::Button::with_label("Open chat");
        open.add_css_class("flat");
        open.add_css_class("nuncio-dialog-primary");
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(8)
            .build();
        actions.append(&cancel);
        actions.append(&open);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_width_request(width);
        column.append(&title);
        column.append(&body);
        column.append(&entry);
        column.append(&error);
        column.append(&actions);
        let popover = gtk::Popover::builder()
            .child(&column)
            .has_arrow(false)
            .autohide(true)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.add_css_class(ROOT_CLASS);
        popover.add_css_class("nuncio-dialog");
        popover.update_property(&[Property::Label(NEW_MESSAGE)]);
        Self {
            popover,
            column,
            entry,
            error,
            open,
            cancel,
        }
    }
}

/// The command palette's widgets and what it lists (plan §6.5): a panel of `width` pixels, the
/// 44 px input row (a magnifier, the query, an × that closes) over a 1 px line, then 36 px rows
/// grouped under `meta` headers, the first selected.
struct PaletteView {
    popover: gtk::Popover,
    column: gtk::Box,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    entries: Vec<palette::Entry>,
    /// The conversation open when the palette opened: another one opening closes the palette.
    conversation: Option<String>,
    /// The positions in `entries` listed now, in order.
    shown: RefCell<Vec<usize>>,
    /// The header above each row listed now ([`palette::headers`]).
    headers: Rc<RefCell<Vec<Option<palette::Group>>>>,
}

impl PaletteView {
    fn new(entries: Vec<palette::Entry>, conversation: Option<String>, width: i32) -> Self {
        let entry = gtk::Entry::builder()
            .placeholder_text("Type a command or a chat name")
            .primary_icon_name("system-search-symbolic")
            .secondary_icon_name("window-close-symbolic")
            .secondary_icon_tooltip_text("Close (Escape)")
            .hexpand(true)
            .build();
        entry.add_css_class("nuncio-palette-entry");
        entry.update_property(&[Property::Label("Command palette: type to filter")]);
        let input = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        input.add_css_class("nuncio-palette-input");
        input.append(&entry);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .accessible_role(gtk::AccessibleRole::List)
            .build();
        list.add_css_class("nuncio-palette-list");
        list.update_property(&[Property::Label("Commands")]);
        let nothing = label("No matching command", "nuncio-palette-header");
        list.set_placeholder(Some(&nothing));
        let headers: Rc<RefCell<Vec<Option<palette::Group>>>> = Rc::default();
        list.set_header_func({
            let headers = Rc::clone(&headers);
            move |row, _| {
                let group = usize::try_from(row.index())
                    .ok()
                    .and_then(|i| headers.borrow().get(i).copied().flatten());
                match group {
                    Some(group) => {
                        row.set_header(Some(&label(group.header(), "nuncio-palette-header")));
                    }
                    None => row.set_header(None::<&gtk::Widget>),
                }
            }
        });
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .child(&list)
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.set_width_request(width);
        column.append(&input);
        column.append(&scroll);
        let popover = gtk::Popover::builder()
            .child(&column)
            .has_arrow(false)
            .autohide(true)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.add_css_class(ROOT_CLASS);
        popover.add_css_class("nuncio-palette");
        popover.update_property(&[Property::Label("Command palette")]);
        Self {
            popover,
            column,
            entry,
            list,
            scroll,
            entries,
            conversation,
            shown: RefCell::new(Vec::new()),
            headers,
        }
    }

    /// List the commands `query` matches, grouped with the best first, each group under its
    /// header, and select the first.
    fn filter(&self, query: &str) {
        let shown = palette::filter(&self.entries, query);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        *self.headers.borrow_mut() = palette::headers(&self.entries, &shown);
        for &index in &shown {
            self.list.append(&palette_row(&self.entries[index]));
        }
        self.list.invalidate_headers();
        *self.shown.borrow_mut() = shown;
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    /// Move the selection `step` rows, stopping at both ends (the focus stays in the input).
    fn move_selection(&self, step: i32) {
        let count = self.shown.borrow().len();
        if count == 0 {
            return;
        }
        let at = self
            .list
            .selected_row()
            .and_then(|row| usize::try_from(row.index()).ok())
            .unwrap_or(0);
        let next = palette::moved(at, step, count);
        if let Some(row) = i32::try_from(next)
            .ok()
            .and_then(|i| self.list.row_at_index(i))
        {
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

/// One palette row: the command in `body`, its key right-aligned in `meta`/`text_faint`.
fn palette_row(entry: &palette::Entry) -> gtk::ListBoxRow {
    let title = label(&entry.title, "");
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&title);
    if let Some(key) = entry.shortcut {
        line.append(&label(key, "nuncio-palette-key"));
    }
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .child(&line)
        .build();
    row.update_property(&[Property::Label(&entry.label())]);
    row
}

/// A pane's 44 px header (plan §6.1): `title` at its start, no centred title; which window
/// controls it shows is [`Ui::show_panes`]'s.
fn header_bar(title: &gtk::Label) -> adw::HeaderBar {
    let header = adw::HeaderBar::new();
    header.add_css_class("nuncio-headerbar");
    header.add_css_class("nuncio-header");
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    header.pack_start(title);
    header
}

/// A pane header's title, 16 px from the pane's edge.
fn pane_title(text: &str) -> gtk::Label {
    let title = gtk::Label::builder().label(text).xalign(0.0).build();
    title.add_css_class("nuncio-title");
    title.set_margin_start(12);
    title
}

/// Two panes side by side with a draggable seam, the first `width` pixels wide at first: a
/// resized window gives or takes from the second, and neither is squeezed under its minimum.
fn seam(start: &impl IsA<gtk::Widget>, end: &impl IsA<gtk::Widget>, width: i32) -> gtk::Paned {
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(start)
        .end_child(end)
        .resize_start_child(false)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(width)
        .build();
    paned.set_wide_handle(false);
    paned
}

/// The Send button's tooltip: its keys (plan §7).
const SEND_TOOLTIP: &str = "Send (Enter; Alt+Enter starts a new line)";

/// What a key does in the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerKey {
    Send,
    Newline,
    Escape,
}

/// The composer's keys (plan §7): Enter sends and Ctrl+Enter too, Alt+Enter starts a new line
/// (Shift+Enter as well, as before), Escape cancels the reply; any other key is the text view's.
fn composer_key_action(key: gdk::Key, state: gdk::ModifierType) -> Option<ComposerKey> {
    if key == gdk::Key::Escape {
        Some(ComposerKey::Escape)
    } else if !is_enter(key) {
        None
    } else if state.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SHIFT_MASK) {
        Some(ComposerKey::Newline)
    } else {
        Some(ComposerKey::Send)
    }
}

/// The height of one line of the composer's font, in pixels.
fn composer_line_height(composer: &gtk::TextView) -> i32 {
    composer.create_pango_layout(Some("X")).pixel_size().1
}

fn is_enter(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
    )
}

/// A move the chat list or the conversation makes for a key (plan §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKey {
    Down,
    Up,
    Top,
    Bottom,
}

/// `j`/`k` move down/up, `g` to the top and Shift+G to the bottom, without Ctrl or Alt; the
/// arrows stay GTK's.
fn list_key_action(key: gdk::Key, state: gdk::ModifierType) -> Option<ListKey> {
    if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
        return None;
    }
    match key {
        gdk::Key::j => Some(ListKey::Down),
        gdk::Key::k => Some(ListKey::Up),
        gdk::Key::g => Some(ListKey::Top),
        gdk::Key::G => Some(ListKey::Bottom),
        _ => None,
    }
}

/// What a key does in the palette's input (plan §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteKey {
    Up,
    Down,
    Run,
    Close,
}

/// Up/Down move the selection, Enter runs it, Escape closes; any other key is typed.
fn palette_key_action(key: gdk::Key) -> Option<PaletteKey> {
    match key {
        gdk::Key::Up | gdk::Key::KP_Up => Some(PaletteKey::Up),
        gdk::Key::Down | gdk::Key::KP_Down => Some(PaletteKey::Down),
        gdk::Key::Escape => Some(PaletteKey::Close),
        key if is_enter(key) => Some(PaletteKey::Run),
        _ => None,
    }
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

/// One row of the chat list (plan §6.2): the unread marker, a square avatar, the name and time,
/// then the preview. Unread is told three ways, never by colour alone: the marker, the name's
/// tone and, past one message, the count chip.
fn chat_row(chat: &ChatRow) -> gtk::ListBoxRow {
    let marker = gtk::Box::builder()
        .width_request(6)
        .height_request(6)
        .valign(gtk::Align::Center)
        .margin_start(8)
        .build();
    if chat.unread.is_some() {
        marker.add_css_class("nuncio-unread-mark");
    }
    let avatar = adw::Avatar::new(32, Some(&chat.title), true);
    avatar.set_margin_start(2);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    if chat.group {
        let glyph = gtk::Image::from_icon_name("system-users-symbolic");
        glyph.set_pixel_size(12);
        glyph.add_css_class("nuncio-faint");
        glyph.set_tooltip_text(Some("Group"));
        top.append(&glyph);
    }
    let title = label(&chat.title, "nuncio-name");
    if chat.unread.is_some() {
        title.add_css_class("nuncio-name-unread");
    }
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    top.append(&title);
    // The same marks the terminal client shows, quiet: the accent is for what is alive.
    let marks: String = [(chat.pinned, '★'), (chat.muted, '⊘'), (chat.archived, '▤')]
        .iter()
        .filter_map(|&(on, mark)| on.then_some(mark))
        .collect();
    if !marks.is_empty() {
        top.append(&label(&marks, "nuncio-faint"));
    }
    let time = label(&chat.when, "nuncio-meta");
    time.add_css_class("nuncio-faint");
    top.append(&time);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    if let Some(ticks) = chat.ticks {
        bottom.append(&label(ticks, "nuncio-ticks"));
    }
    if let Some(author) = &chat.author {
        bottom.append(&label(&format!("{author}:"), "nuncio-preview-author"));
    }
    let preview = label(&chat.preview, "nuncio-preview");
    preview.set_hexpand(true);
    preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
    bottom.append(&preview);
    if let Some(count) = &chat.chip {
        let chip = label(count, "nuncio-chip");
        chip.set_xalign(0.5);
        chip.set_valign(gtk::Align::Center);
        bottom.append(&chip);
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.set_margin_start(12);
    text.set_margin_end(16);
    text.append(&top);
    text.append(&bottom);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&marker);
    content.append(&avatar);
    content.append(&text);
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .child(&content)
        .build();
    row.add_css_class("nuncio-chat-row");
    row.update_property(&[Property::Label(&chat.label)]);
    row
}

/// What an empty list or transcript shows (plan §6.3): a glyph, a title line and a hint line.
struct Empty {
    root: gtk::Box,
    title: gtk::Label,
    hint: gtk::Label,
}

impl Empty {
    fn new(icon: &str) -> Self {
        let glyph = gtk::Image::from_icon_name(icon);
        glyph.set_pixel_size(32);
        glyph.add_css_class("nuncio-faint");
        let title = label("", "nuncio-title");
        title.set_xalign(0.5);
        let hint = label("", "nuncio-meta");
        hint.set_xalign(0.5);
        hint.set_wrap(true);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Center);
        root.set_vexpand(true);
        root.set_margin_top(48);
        root.set_margin_bottom(48);
        root.add_css_class("nuncio-empty");
        root.append(&glyph);
        root.append(&title);
        root.append(&hint);
        Self { root, title, hint }
    }

    /// Say `state`, or nothing (`None`: there is nothing true to say yet).
    fn show(&self, state: Option<present::EmptyState>) {
        self.root.set_visible(state.is_some());
        let state = state.unwrap_or(present::EmptyState {
            title: "",
            hint: "",
        });
        self.title.set_text(state.title);
        self.hint.set_text(state.hint);
        self.root
            .update_property(&[Property::Label(&format!("{}. {}", state.title, state.hint))]);
    }

    /// The title and hint while shown (tests).
    fn texts(&self) -> Option<EmptyTexts> {
        self.root
            .get_visible()
            .then(|| (self.title.text().to_string(), self.hint.text().to_string()))
    }
}

/// Today in local time, for the day labels and the rows' times.
fn local_today() -> Option<present::Day> {
    let now = glib::DateTime::now_local().ok()?;
    Some(present::Day::from_civil(
        now.year(),
        u32::try_from(now.month()).ok()?,
        u32::try_from(now.day_of_month()).ok()?,
    ))
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

/// One message as a square bubble (plan §6.3, decision 1): mine on the right on the active tone,
/// theirs on the left on the chrome with a 1 px border. The day label goes above the first bubble
/// of a day, the author line above the first of an author group (theirs), and the meta line (time
/// and state) under the group's last. The row is what takes the focus and what a screen reader
/// reads, by its label: one row per message, so a row's index is its message's.
fn bubble_row(ui: &Weak<Inner>, bubble: &Bubble) -> gtk::ListBoxRow {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    if !bubble.mine && bubble.starts_group {
        column.append(&label(&bubble.sender, "nuncio-author-line"));
    }
    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.add_css_class("nuncio-bubble");
    body.add_css_class(if bubble.mine {
        "nuncio-mine"
    } else {
        "nuncio-theirs"
    });
    if bubble
        .picture
        .as_ref()
        .is_some_and(|picture| picture.sticker)
    {
        body.add_css_class("nuncio-sticker");
    }
    if let Some(quote) = &bubble.quote {
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lines.add_css_class("nuncio-quote");
        if let Some(author) = &bubble.quote_author {
            lines.append(&label(author, "nuncio-quote-author"));
        }
        let text = label(quote, "nuncio-quote-text");
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        lines.append(&text);
        body.append(&lines);
    }
    let downloading = bubble
        .media
        .as_deref()
        .is_some_and(|media| media.ends_with("downloading…"));
    match (&bubble.document, &bubble.media) {
        (Some(document), _) => {
            body.append(&document_row(document));
            if downloading {
                body.append(&label("downloading…", "nuncio-meta"));
            }
        }
        (None, Some(media)) => body.append(&label(media, "nuncio-media")),
        (None, None) => {}
    }
    if let Some(picture) = &bubble.picture {
        body.append(&picture_widget(ui, bubble.index, picture));
    }
    if !bubble.text.is_empty() {
        // A media message's text is its caption, in the meta size.
        let class = if bubble.media.is_some() {
            "nuncio-caption"
        } else {
            "nuncio-text"
        };
        let text = label(&bubble.text, class);
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_max_width_chars(68);
        body.append(&text);
    }
    column.append(&body);
    let chips = present::reaction_chips(&bubble.reactions);
    if !chips.is_empty() {
        let reactions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        reactions.set_halign(if bubble.mine {
            gtk::Align::End
        } else {
            gtk::Align::Start
        });
        for chip in &chips {
            reactions.append(&reaction_chip(chip));
        }
        column.append(&reactions);
    }
    if bubble.shows_meta {
        let meta = label(&present::meta_line(bubble), "nuncio-meta-line");
        meta.set_halign(gtk::Align::End);
        if bubble.mark.is_some_and(present::SendMark::is_failure) {
            meta.add_css_class("nuncio-warning");
        }
        column.append(&meta);
    }
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
    lane.set_margin_start(16);
    lane.set_margin_end(16);
    // 2 px between the bubbles of a group, 12 px between groups.
    lane.set_margin_top(if bubble.starts_group { 12 } else { 2 });
    let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(day) = &bubble.day {
        let day = label(&day.to_uppercase(), "nuncio-day");
        day.set_margin_start(16);
        stack.append(&day);
    }
    stack.append(&lane);
    let row = gtk::ListBoxRow::builder()
        .accessible_role(gtk::AccessibleRole::ListItem)
        .activatable(true)
        .child(&stack)
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

/// A document's 32 px row: a file glyph, its name and its size.
fn document_row(document: &present::DocumentView) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("nuncio-document");
    let glyph = gtk::Image::from_icon_name("text-x-generic-symbolic");
    glyph.set_pixel_size(16);
    row.append(&glyph);
    let name = label(&document.name, "nuncio-text");
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    name.set_hexpand(true);
    row.append(&name);
    if !document.size.is_empty() {
        row.append(&label(&document.size, "nuncio-meta"));
    }
    row
}

/// A flat reaction chip: the emoji and how many sent it.
fn reaction_chip(chip: &present::ReactionChip) -> gtk::Box {
    let shown = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    shown.add_css_class("nuncio-reaction");
    if chip.mine {
        shown.add_css_class("nuncio-reaction-mine");
    }
    shown.append(&label(&chip.emoji, "nuncio-reaction-emoji"));
    shown.append(&label(&chip.count.to_string(), "nuncio-meta"));
    shown
}

/// The texts of the labels with CSS class `class` inside `widget`, in order.
fn texts_with_class(widget: &gtk::Widget, class: &str, found: &mut Vec<String>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && label.has_css_class(class)
    {
        found.push(label.text().to_string());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        texts_with_class(&current, class, found);
        child = current.next_sibling();
    }
}

/// An empty state's title and hint, as drawn ([`Ui::empty_states`], tests).
pub type EmptyTexts = (String, String);

/// What is drawn around message `index`'s bubble ([`Ui::message_parts`], tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageParts {
    /// The day label above it.
    pub day: Option<String>,
    /// The author line above it.
    pub author: Option<String>,
    /// The meta line under it.
    pub meta: Option<String>,
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
    /// How many frames the picture in it has: none before one is drawn, one when it is still.
    pub frames: usize,
    /// An animation plays in it.
    pub animating: bool,
}

/// A decoded picture as textures, one per frame built once: RGBA, not premultiplied, so a
/// sticker's transparent pixels show the bubble behind it.
fn drawn(pixels: pictures::Pixels) -> Drawn {
    let width = i32::try_from(pixels.width).unwrap_or(1);
    let height = i32::try_from(pixels.height).unwrap_or(1);
    let stride = pixels.width as usize * 4;
    let texture = |rgba: Vec<u8>| -> gdk::Texture {
        gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(rgba),
            stride,
        )
        .upcast()
    };
    let first = texture(pixels.rgba);
    let animation = (!pixels.later.is_empty()).then(|| {
        let mut textures = vec![first.clone()];
        textures.extend(pixels.later.into_iter().map(texture));
        Rc::new(Animation {
            textures,
            delays_ms: pixels.delays_ms,
        })
    });
    Drawn { first, animation }
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
        let slot = Slot::new(&picture, view.sticker, view.size.is_some());
        if let Some((drawn, variant)) = cache.best(&view.chat, &view.id) {
            let texture = &drawn.first;
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
            slot.show(&picture, &drawn);
        }
        drop(cache);
        inner
            .live
            .borrow_mut()
            .entry((view.chat.clone(), view.id.clone()))
            .or_default()
            .push(slot);
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
    if view.sticker {
        return wide.upcast();
    }
    // A photo's 1 px frame goes around its box, so the box keeps its reserved size.
    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    frame.set_halign(gtk::Align::Start);
    frame.add_css_class("nuncio-frame");
    frame.append(&wide);
    frame.upcast()
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

#[cfg(test)]
mod tests {
    use super::{
        ComposerKey, ListKey, PaletteKey, composer_key_action, list_key_action, palette_key_action,
    };
    use gtk::gdk::{Key, ModifierType};

    #[test]
    fn the_palette_moves_with_up_and_down_runs_on_enter_and_closes_on_escape() {
        for (key, wanted) in [
            (Key::Up, PaletteKey::Up),
            (Key::KP_Up, PaletteKey::Up),
            (Key::Down, PaletteKey::Down),
            (Key::KP_Down, PaletteKey::Down),
            (Key::Return, PaletteKey::Run),
            (Key::KP_Enter, PaletteKey::Run),
            (Key::Escape, PaletteKey::Close),
        ] {
            assert_eq!(palette_key_action(key), Some(wanted), "{key:?}");
        }
        for typed in [Key::a, Key::j, Key::slash, Key::BackSpace] {
            assert_eq!(palette_key_action(typed), None, "{typed:?} is typed");
        }
    }

    #[test]
    fn j_k_move_g_and_shift_g_go_to_the_ends_only_without_ctrl_or_alt() {
        let none = ModifierType::empty();
        assert_eq!(list_key_action(Key::j, none), Some(ListKey::Down));
        assert_eq!(list_key_action(Key::k, none), Some(ListKey::Up));
        assert_eq!(list_key_action(Key::g, none), Some(ListKey::Top));
        assert_eq!(
            list_key_action(Key::G, ModifierType::SHIFT_MASK),
            Some(ListKey::Bottom)
        );
        for held in [ModifierType::CONTROL_MASK, ModifierType::ALT_MASK] {
            assert_eq!(list_key_action(Key::j, held), None, "{held:?}");
            assert_eq!(list_key_action(Key::g, held), None, "{held:?}");
        }
        assert_eq!(
            list_key_action(Key::u, none),
            None,
            "no unread toggle exists"
        );
        assert_eq!(
            list_key_action(Key::Down, none),
            None,
            "the arrows are GTK's"
        );
    }

    #[test]
    fn enter_and_ctrl_enter_send_alt_enter_starts_a_line_escape_cancels() {
        let none = ModifierType::empty();
        for enter in [Key::Return, Key::KP_Enter, Key::ISO_Enter] {
            assert_eq!(composer_key_action(enter, none), Some(ComposerKey::Send));
            assert_eq!(
                composer_key_action(enter, ModifierType::CONTROL_MASK),
                Some(ComposerKey::Send),
                "Ctrl+Enter is an alias"
            );
            assert_eq!(
                composer_key_action(enter, ModifierType::ALT_MASK),
                Some(ComposerKey::Newline)
            );
            assert_eq!(
                composer_key_action(enter, ModifierType::SHIFT_MASK),
                Some(ComposerKey::Newline),
                "Shift+Enter still starts a line"
            );
        }
        assert_eq!(
            composer_key_action(Key::Escape, none),
            Some(ComposerKey::Escape)
        );
        assert_eq!(composer_key_action(Key::a, none), None, "typing is GTK's");
        assert_eq!(composer_key_action(Key::Tab, none), None);
    }
}
