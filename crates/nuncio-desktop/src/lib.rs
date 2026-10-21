//! The GTK 4 + libadwaita desktop client.
//!
//! - [`window`]: the window (chat list, transcript, composer), a thin drawing of
//!   [`present`]'s view of the shared `nuncio_client::Model`;
//! - [`controller`]: what each user action and daemon answer does to the model, with the shared
//!   driver's rules (`nuncio_client::driver`), without GTK;
//! - [`lifecycle`]: when the window shows and what closing it does (`[app] background`,
//!   `start_hidden`);
//! - [`palette`]: the command palette's commands, matching and actions, without GTK;
//! - [`pictures`]: the pictures inside image and sticker bubbles (boxes, decoding, the cache),
//!   without GTK;
//! - [`preferences`]: the settings window's rules over `config.toml`, without GTK;
//! - [`worker`]: the daemon connections on their own threads, posting into the main loop;
//! - here: following Omarchy's live theme. The palette parser and the watcher live in
//!   `nuncio-client`, because following the theme is client-side work the TUI needs too; what
//!   lives here is turning a [`Palette`] into GTK styling and asking libadwaita for the matching
//!   colour scheme.

pub mod controller;
pub mod lifecycle;
pub mod palette;
pub mod pictures;
pub mod preferences;
pub mod present;
pub mod window;
pub mod worker;

use gtk::prelude::*;
use nuncio_client::{Mode, Palette, Rgb};

/// The CSS class the app's root widget carries, so generated rules can target it.
pub const ROOT_CLASS: &str = "nuncio-root";

/// The CSS that paints a widget tree in an Omarchy palette.
///
/// Generated rather than read from a theme-supplied `<app>.css`: Omarchy only delivers those
/// through its own per-app `omarchy-theme-set-<app>` hook (see `docs/`), so an app that is not on
/// that hardcoded list must colour itself.
pub fn css_for(palette: &Palette) -> String {
    let bright_foreground = palette.bright_foreground.unwrap_or(palette.foreground);
    format!(
        ".{ROOT_CLASS} {{\n  \
           background-color: {background};\n  \
           color: {foreground};\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-accent {{ color: {accent}; }}\n\
         .{ROOT_CLASS} .nuncio-unread {{\n  \
           background-color: {accent};\n  \
           color: {background};\n  \
           border-radius: 6px;\n  \
           padding: 0 6px;\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-muted {{ color: {bright_foreground}; }}\n\
         .{ROOT_CLASS} .nuncio-error {{ color: {red}; }}\n\
         .{ROOT_CLASS} .nuncio-bold {{ font-weight: bold; }}\n\
         .{ROOT_CLASS} .nuncio-transcript {{ background-color: {background}; }}\n\
         .{ROOT_CLASS} .nuncio-bubble {{\n  \
           padding: 6px 10px;\n  \
           border-radius: 12px;\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-bubble.nuncio-mine {{\n  \
           background-color: {accent};\n  \
           color: {background};\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-bubble.nuncio-theirs {{\n  \
           background-color: alpha({foreground}, 0.08);\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-bubble.nuncio-failed {{\n  \
           background-color: alpha({red}, 0.15);\n  \
           color: {foreground};\n  \
           border: 1px solid {red};\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-sender {{ color: {accent}; font-weight: bold; }}\n\
         .{ROOT_CLASS} .nuncio-quote {{\n  \
           border-left: 2px solid currentColor;\n  \
           padding-left: 6px;\n  \
           opacity: 0.8;\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-media {{ font-style: italic; }}\n\
         .{ROOT_CLASS} .nuncio-reaction-mine {{ font-weight: bold; }}\n\
         .{ROOT_CLASS} row.nuncio-message:focus-visible .nuncio-bubble {{\n  \
           outline: 2px solid {accent};\n  \
           outline-offset: 1px;\n\
         }}\n",
        background = palette.background.to_hex(),
        foreground = palette.foreground.to_hex(),
        accent = palette.accent.to_hex(),
        bright_foreground = bright_foreground.to_hex(),
        red = palette.red.to_hex(),
    )
}

/// Install `palette` on a display: the generated CSS at application priority, plus the
/// libadwaita colour scheme implied by the palette's `mode`.
///
/// The colour scheme is set from `colors.toml` and not from gsettings, because Omarchy's
/// `omarchy-theme-set-gnome` updates gsettings only after the theme directory has already been
/// swapped — waiting for it would repaint late.
///
/// One provider per display is installed and then reloaded on each switch, so a switch replaces
/// the old palette's rules instead of stacking a new provider over them (no stale colours).
pub fn apply_to_display(display: &gtk::gdk::Display, palette: &Palette) {
    thread_local! {
        static INSTALLED: std::cell::RefCell<Vec<(gtk::gdk::Display, gtk::CssProvider)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    let provider = INSTALLED.with(|installed| {
        let mut installed = installed.borrow_mut();
        if let Some((_, provider)) = installed.iter().find(|(d, _)| d == display) {
            return provider.clone();
        }
        let provider = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        installed.push((display.clone(), provider.clone()));
        provider
    });
    provider.load_from_string(&css_for(palette));

    adw::StyleManager::default().set_color_scheme(match palette.mode {
        Mode::Light => adw::ColorScheme::ForceLight,
        Mode::Dark => adw::ColorScheme::ForceDark,
        // An unparseable `mode` must not pin the scheme: follow the desktop.
        Mode::Unknown => adw::ColorScheme::Default,
    });
}

/// The colour GTK has actually resolved for `widget`, which is the app's foreground once
/// [`apply_to_display`] has run. Used by `--probe` as observable evidence that the generated CSS
/// reached the style engine rather than merely being constructed.
///
/// `StyleContext::color` is deprecated in GTK 4.10 in favour of the CSS `color` property lookup,
/// but it is still the only way to ask the style engine what it settled on, which is exactly what
/// the probe needs.
#[allow(deprecated)]
pub fn resolved_foreground(widget: &impl IsA<gtk::Widget>) -> Option<Rgb> {
    let rgba = widget.as_ref().style_context().color();
    Some(Rgb::new(
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::{ROOT_CLASS, css_for};
    use nuncio_client::{Palette, Rgb};

    #[test]
    fn the_generated_css_paints_every_palette_role() {
        let palette = Palette {
            background: Rgb::new(0xfd, 0xf6, 0xe3),
            foreground: Rgb::new(0x26, 0x36, 0x3d),
            accent: Rgb::new(0x26, 0x8b, 0xd3),
            ..Palette::default()
        };
        let css = css_for(&palette);
        assert!(css.contains(&format!(".{ROOT_CLASS} {{")), "{css}");
        for expected in ["#fdf6e3", "#26363d", "#268bd3"] {
            assert!(css.contains(expected), "{expected} missing from: {css}");
        }
        assert!(
            !css.contains(&Palette::default().background.to_hex()),
            "the default background must not leak into a real palette: {css}"
        );
    }

    #[test]
    fn a_missing_bright_foreground_falls_back_to_the_foreground() {
        let palette = Palette {
            bright_foreground: None,
            ..Palette::default()
        };
        let css = css_for(&palette);
        let muted = css
            .lines()
            .find(|line| line.contains("nuncio-muted"))
            .expect("a muted rule");
        assert!(muted.contains(&palette.foreground.to_hex()), "{muted}");
    }
}
