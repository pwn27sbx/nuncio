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
//! - [`args`]: the command line and the app id;
//! - [`hyprland`]: Hyprland's window rounding (`hyprctl`), behind an injectable runner;
//! - [`style`]: the design's tokens for GTK (the panes' CSS from the palette roles, the pane
//!   widths and breakpoints), without GTK;
//! - here: following Omarchy's live theme and font. The palette parser and the watcher live in
//!   `nuncio-client`, because following the theme is client-side work the TUI needs too; what
//!   lives here is turning a [`Palette`] and a [`Look`] (the Omarchy font, Hyprland's rounding)
//!   into GTK styling and asking libadwaita for the matching colour scheme.

pub mod args;
pub mod controller;
pub mod hyprland;
pub mod lifecycle;
pub mod palette;
pub mod pictures;
pub mod preferences;
pub mod present;
pub mod style;
pub mod window;
pub mod worker;

use gtk::prelude::*;
use nuncio_client::{Mode, Palette, Rgb};
use std::path::Path;

/// The CSS class the app's root widget carries, so generated rules can target it.
pub const ROOT_CLASS: &str = "nuncio-root";

/// The CSS class the app's windows carry, for the corner radius.
pub const WINDOW_CLASS: &str = "nuncio-window";

/// What the desktop sets besides the palette. A `None` font keeps libadwaita's; a `None`
/// rounding is 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Look {
    /// Omarchy's font (the family fontconfig resolves `monospace` to), for the whole UI: Omarchy
    /// uses it as both the terminal and the system font.
    pub font_family: Option<String>,
    /// Hyprland's `decoration:rounding`, in pixels, for the window corners and every element;
    /// `None` (not on Hyprland, or no answer) squares them.
    pub corner_radius: Option<u32>,
    /// The desktop's `gtk-enable-animations` (GNOME's `enable-animations` gsetting): off, the app
    /// has no transitions at all (design plan §4.4).
    pub animations: bool,
}

/// Whether Omarchy is installed here: its state directory (`~/.local/state/omarchy`, the one
/// holding `current/theme/colors.toml`) exists. Only then is fontconfig's `monospace` Omarchy's
/// choice; elsewhere the font stays libadwaita's.
pub fn omarchy_here(default_palette: Option<&Path>) -> bool {
    default_palette
        .and_then(|colors| colors.ancestors().nth(3))
        .is_some_and(Path::is_dir)
}

/// A CSS string literal holding `text`.
fn css_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// The rules [`Look`] adds to [`css_for`]'s.
fn look_css(look: &Look) -> String {
    let mut css = String::new();
    if let Some(family) = &look.font_family {
        css.push_str(&format!(
            ".{ROOT_CLASS} {{ font-family: {}, monospace; }}\n",
            css_string(family)
        ));
    }
    // Square without Hyprland too, as every element in it (design plan §4.3, §5).
    let radius = look.corner_radius.unwrap_or(0);
    css.push_str(&format!(
        "window.{WINDOW_CLASS}, window.{WINDOW_CLASS}.csd {{ border-radius: {radius}px; }}\n"
    ));
    css
}

/// The CSS that paints a widget tree in an Omarchy palette, with the desktop's [`Look`].
///
/// Generated rather than read from a theme-supplied `<app>.css`: Omarchy only delivers those
/// through its own per-app `omarchy-theme-set-<app>` hook (see `docs/`), so an app that is not on
/// that hardcoded list must colour itself.
///
/// Every element's corners, the window's included, are Hyprland's rounding, and square without
/// one (design plan §4.3).
pub fn css_for(palette: &Palette, look: &Look) -> String {
    let radius = look.corner_radius.unwrap_or(0);
    let bright_foreground = palette.bright_foreground.unwrap_or(palette.foreground);
    let mut css = format!(
        ".{ROOT_CLASS} {{\n  \
           background-color: {background};\n  \
           color: {foreground};\n\
         }}\n\
         .{ROOT_CLASS} .nuncio-accent {{ color: {accent}; }}\n\
         .{ROOT_CLASS} .nuncio-muted {{ color: {bright_foreground}; }}\n\
         .{ROOT_CLASS} .nuncio-error {{ color: {red}; }}\n\
         .{ROOT_CLASS} .nuncio-bold {{ font-weight: bold; }}\n\
         .{ROOT_CLASS} .nuncio-transcript {{ background-color: {background}; }}\n",
        background = palette.background.to_hex(),
        foreground = palette.foreground.to_hex(),
        accent = palette.accent.to_hex(),
        bright_foreground = bright_foreground.to_hex(),
        red = palette.red.to_hex(),
    );
    css.push_str(&style::layout_css(palette, radius));
    css.push_str(&look_css(look));
    css.push_str(&style::motion_css(look.animations));
    css
}

/// Install `palette` on a display: the generated CSS at application priority, plus the
/// libadwaita colour scheme implied by the palette's `mode`. The display's [`Look`] is kept.
///
/// The colour scheme is set from `colors.toml` and not from gsettings, because Omarchy's
/// `omarchy-theme-set-gnome` updates gsettings only after the theme directory has already been
/// swapped — waiting for it would repaint late.
///
/// One provider per display is installed and then reloaded on each switch, so a switch replaces
/// the old palette's rules instead of stacking a new provider over them (no stale colours).
pub fn apply_to_display(display: &gtk::gdk::Display, palette: &Palette) {
    restyle(display, |style| style.palette = palette.clone());

    adw::StyleManager::default().set_color_scheme(match palette.mode {
        Mode::Light => adw::ColorScheme::ForceLight,
        Mode::Dark => adw::ColorScheme::ForceDark,
        // An unparseable `mode` must not pin the scheme: follow the desktop.
        Mode::Unknown => adw::ColorScheme::Default,
    });
}

/// Change the display's [`Look`] (a font switch, the rounding at start), keeping its palette.
pub fn apply_look(display: &gtk::gdk::Display, change: impl FnOnce(&mut Look)) {
    restyle(display, |style| change(&mut style.look));
}

struct Installed {
    display: gtk::gdk::Display,
    provider: gtk::CssProvider,
    palette: Palette,
    look: Look,
}

/// Update what `display` is styled with and reload its one provider.
fn restyle(display: &gtk::gdk::Display, change: impl FnOnce(&mut Installed)) {
    thread_local! {
        static INSTALLED: std::cell::RefCell<Vec<Installed>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    INSTALLED.with(|installed| {
        let mut installed = installed.borrow_mut();
        let at = match installed.iter().position(|i| &i.display == display) {
            Some(at) => at,
            None => {
                let provider = gtk::CssProvider::new();
                gtk::style_context_add_provider_for_display(
                    display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
                installed.push(Installed {
                    display: display.clone(),
                    provider,
                    palette: Palette::default(),
                    look: Look::default(),
                });
                installed.len() - 1
            }
        };
        let style = &mut installed[at];
        change(style);
        style
            .provider
            .load_from_string(&css_for(&style.palette, &style.look));
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
    use super::{Look, ROOT_CLASS, WINDOW_CLASS, css_for, omarchy_here};
    use nuncio_client::{Palette, Rgb};

    #[test]
    fn the_generated_css_paints_every_palette_role() {
        let palette = Palette {
            background: Rgb::new(0xfd, 0xf6, 0xe3),
            foreground: Rgb::new(0x26, 0x36, 0x3d),
            accent: Rgb::new(0x26, 0x8b, 0xd3),
            ..Palette::default()
        };
        let css = css_for(&palette, &Look::default());
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
        let css = css_for(&palette, &Look::default());
        let muted = css
            .lines()
            .find(|line| line.contains("nuncio-muted"))
            .expect("a muted rule");
        assert!(muted.contains(&palette.foreground.to_hex()), "{muted}");
    }

    #[test]
    fn the_omarchy_font_styles_the_whole_ui() {
        let look = Look {
            font_family: Some("JetBrainsMono Nerd Font".to_owned()),
            ..Look::default()
        };
        let css = css_for(&Palette::default(), &look);
        assert!(
            css.contains(&format!(
                ".{ROOT_CLASS} {{ font-family: \"JetBrainsMono Nerd Font\", monospace; }}"
            )),
            "{css}"
        );
    }

    #[test]
    fn without_a_font_libadwaita_keeps_its_own_and_without_a_rounding_the_window_is_square() {
        let css = css_for(&Palette::default(), &Look::default());
        assert!(!css.contains("font-family"), "{css}");
        let rule = css
            .lines()
            .find(|line| line.contains(&format!("window.{WINDOW_CLASS}")))
            .expect("a window rule");
        assert!(rule.contains("border-radius: 0px;"), "{rule}");
    }

    #[test]
    fn hyprlands_rounding_shapes_the_window_corners() {
        for radius in [0, 10] {
            let look = Look {
                corner_radius: Some(radius),
                ..Look::default()
            };
            let css = css_for(&Palette::default(), &look);
            let rule = css
                .lines()
                .find(|line| line.contains(&format!("window.{WINDOW_CLASS}")))
                .expect("a window rule");
            assert!(
                rule.contains(&format!("border-radius: {radius}px;")),
                "{rule}"
            );
        }
    }

    #[test]
    fn a_font_name_cannot_break_out_of_its_css_string() {
        let look = Look {
            font_family: Some(r#"Evil" } * { color: red; } x {"#.to_owned()),
            ..Look::default()
        };
        let css = css_for(&Palette::default(), &look);
        assert!(
            css.contains(r#"font-family: "Evil\" } * { color: red; } x {", monospace;"#),
            "{css}"
        );
    }

    #[test]
    fn omarchy_is_recognised_by_its_state_directory() {
        let root = std::env::temp_dir().join(format!("nuncio-omarchy-{}", std::process::id()));
        let colors = root.join("omarchy/current/theme/colors.toml");
        assert!(!omarchy_here(Some(&colors)), "no Omarchy directory yet");
        std::fs::create_dir_all(root.join("omarchy")).expect("create state dir");
        assert!(
            omarchy_here(Some(&colors)),
            "the Omarchy state directory exists"
        );
        assert!(!omarchy_here(None));
        std::fs::remove_dir_all(&root).ok();
    }
}
