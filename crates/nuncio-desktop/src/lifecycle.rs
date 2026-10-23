//! When the window shows and what closing it does, from `[app]` in `config.toml`, without GTK.
//!
//! - `background = true` (the default): closing the window only hides it; the app keeps running
//!   (its daemon connection and its notifications too) until Quit (Ctrl+Q, or Quit in the menu).
//! - `background = false`: closing the window quits, as before D3b.
//! - `start_hidden = true`: the first launch shows no window; launching the app again, clicking a
//!   notification or opening a link shows it.

use nuncio_client::config::App;

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// Hide the window and keep running.
    Hide,
    /// Close the window and quit.
    Quit,
}

/// The window's lifecycle decisions.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    background: bool,
    start_hidden: bool,
    /// The window was shown once (or deliberately not, at a hidden start).
    started: bool,
}

impl Lifecycle {
    pub fn new(app: &App) -> Self {
        Self {
            background: app.background,
            start_hidden: app.start_hidden,
            started: false,
        }
    }

    /// The app is launched (or launched again, which only brings the window back): whether to
    /// show the window.
    pub fn activate(&mut self) -> bool {
        let first = !std::mem::replace(&mut self.started, true);
        !(first && self.start_hidden)
    }

    /// The window was shown for another reason (a notification clicked, a link opened): a later
    /// launch shows it as usual.
    pub fn shown(&mut self) {
        self.started = true;
    }

    /// What closing the window does.
    pub fn close(&self) -> Close {
        if self.background {
            Close::Hide
        } else {
            Close::Quit
        }
    }

    /// Whether the app must hold itself running while no window is shown.
    pub fn holds(&self) -> bool {
        self.background
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle(background: bool, start_hidden: bool) -> Lifecycle {
        Lifecycle::new(&App {
            background,
            start_hidden,
            ..App::default()
        })
    }

    #[test]
    fn by_default_the_window_shows_and_closing_it_keeps_the_app_running() {
        let mut l = Lifecycle::new(&App::default());
        assert!(l.activate());
        assert_eq!(l.close(), Close::Hide);
        assert!(l.holds());
    }

    #[test]
    fn without_background_closing_the_window_quits() {
        let l = lifecycle(false, false);
        assert_eq!(l.close(), Close::Quit);
        assert!(!l.holds());
    }

    #[test]
    fn start_hidden_shows_nothing_at_the_first_launch_and_the_window_on_the_next() {
        let mut l = lifecycle(true, true);
        assert!(!l.activate(), "the first launch stays hidden");
        assert!(l.activate(), "launching again brings the window");
        assert!(l.activate());
    }

    #[test]
    fn a_window_shown_for_a_notification_or_a_link_counts_as_the_start() {
        let mut l = lifecycle(true, true);
        l.shown();
        assert!(l.activate(), "a later launch shows the window");
    }

    #[test]
    fn without_start_hidden_every_launch_shows_the_window() {
        let mut l = lifecycle(true, false);
        assert!(l.activate());
        assert!(l.activate());
    }
}
