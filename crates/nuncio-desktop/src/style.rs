//! The design's tokens in GTK terms (design plan §4–§5): the CSS the window's panes, headers and
//! rail are painted with, generated from the palette's semantic roles, and the pane widths and
//! breakpoints of the layout. Without GTK, so both are tested as plain values.
//!
//! The CSS here is appended to [`crate::css_for`]'s, so it is loaded by the one provider per
//! display that a live theme switch reloads: a switch repaints the panes with the new roles.

use nuncio_client::Palette;

use crate::ROOT_CLASS;

/// The rail's width with its labels (plan §5).
pub const RAIL_WIDTH: i32 = 200;
/// The rail's width as an icon strip, on a half-screen tile.
pub const RAIL_COMPACT_WIDTH: i32 = 48;
/// The chat list's width.
pub const LIST_WIDTH: i32 = 360;
/// The height of a pane header.
pub const HEADER_HEIGHT: u32 = 44;
/// The composer bar's height with one line (plan §4.3).
pub const COMPOSER_MIN_HEIGHT: u32 = 48;
/// The composer grows to this many lines, then scrolls (plan §6.4).
pub const COMPOSER_LINES: i32 = 6;
/// The padding above and below the composer's text, inside its field.
pub const COMPOSER_PAD: i32 = 8;

/// The composer field's tallest height for a font whose lines are `line` pixels tall: six lines
/// and the padding; past that the field scrolls.
pub fn composer_max_height(line: i32) -> i32 {
    COMPOSER_LINES * line.max(1) + 2 * COMPOSER_PAD
}

/// The command palette's width (plan §6.5).
pub const PALETTE_WIDTH: i32 = 560;
/// The settings' column width (plan §6.7).
pub const SETTINGS_WIDTH: i32 = 480;
/// A dialog's width (plan §6.8).
pub const DIALOG_WIDTH: i32 = 480;
/// The room kept on each side of the palette on a window narrower than it.
const GUTTER: i32 = 16;

/// The palette's width over a window `window` pixels wide: [`PALETTE_WIDTH`], or less on a
/// narrow window, with a 16 px gutter each side. A window not laid out yet (0) gets the full
/// width.
pub fn palette_width(window: i32) -> i32 {
    fit(PALETTE_WIDTH, window)
}

/// A dialog's width over a window `window` pixels wide: [`DIALOG_WIDTH`], narrowed as
/// [`palette_width`] is.
pub fn dialog_width(window: i32) -> i32 {
    fit(DIALOG_WIDTH, window)
}

/// `width`, or less on a window narrower than it with a 16 px gutter each side; a window not
/// laid out yet (0) gets the full width.
fn fit(width: i32, window: i32) -> i32 {
    if window <= 0 {
        width
    } else {
        width.min(window - 2 * GUTTER).max(0)
    }
}

/// From this tile width up, three panes.
pub const THREE_PANES_FROM: i32 = 1100;
/// From this tile width up (and below [`THREE_PANES_FROM`]), the rail is an icon strip; below it,
/// one pane at a time.
pub const TWO_PANES_FROM: i32 = 760;

/// How the window lays its panes out at a tile width (plan §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The rail with its labels, the chat list and the transcript.
    ThreePanes,
    /// The rail as a 48 px icon strip, the chat list and the transcript.
    CompactRail,
    /// One pane at a time: the list, or the open conversation (Escape goes back).
    SinglePane,
}

/// The layout for a window `width` pixels wide.
pub fn layout_for(width: i32) -> Layout {
    if width >= THREE_PANES_FROM {
        Layout::ThreePanes
    } else if width >= TWO_PANES_FROM {
        Layout::CompactRail
    } else {
        Layout::SinglePane
    }
}

/// The libadwaita breakpoint conditions that switch to each narrower layout, widest first, so
/// that the last one matching (the one libadwaita applies) is the narrowest: no breakpoint
/// applied means [`Layout::ThreePanes`].
pub fn breakpoints() -> [(String, Layout); 2] {
    [
        (
            format!("max-width: {}px", THREE_PANES_FROM - 1),
            Layout::CompactRail,
        ),
        (
            format!("max-width: {}px", TWO_PANES_FROM - 1),
            Layout::SinglePane,
        ),
    ]
}

/// The rules for the panes, headers, rail and status strip, in `palette`'s roles, with every
/// corner of every element inside the app rounded by `radius` pixels (Hyprland's
/// `decoration:rounding`; 0 squares them all, libadwaita's own radii included).
pub fn layout_css(palette: &Palette, radius: u32) -> String {
    let root = ROOT_CLASS;
    format!(
        ".{root}, .{root} * {{ border-radius: {radius}px; }}\n\
         .{root} .nuncio-chrome {{ background-color: {chrome}; color: {text}; }}\n\
         .{root} .nuncio-content {{ background-color: {surface}; color: {text}; }}\n\
         .{root} paned > separator {{\n  \
           min-width: 1px;\n  \
           min-height: 1px;\n  \
           margin: 0;\n  \
           padding: 0;\n  \
           background-color: {border};\n  \
           background-image: none;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .top-bar {{ background-color: transparent; box-shadow: none; }}\n\
         .{root} headerbar.nuncio-header {{\n  \
           min-height: {header}px;\n  \
           padding: 0 4px;\n  \
           background-color: transparent;\n  \
           background-image: none;\n  \
           box-shadow: none;\n  \
           border-bottom: 1px solid {border};\n  \
           color: {text};\n\
         }}\n\
         .{root} .nuncio-title {{ font-size: 15px; font-weight: 600; }}\n\
         .{root} .nuncio-meta {{ font-size: 11px; color: {muted}; }}\n\
         .{root} .nuncio-rail {{ padding: 12px 8px; }}\n\
         .{root} button.nuncio-primary {{\n  \
           min-height: 32px;\n  \
           background-color: {accent};\n  \
           background-image: none;\n  \
           color: {accent_on};\n  \
           font-weight: 600;\n  \
           border: none;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} button.nuncio-primary:hover {{ filter: brightness(1.08); }}\n\
         .{root} button:focus-visible, .{root} row:focus-visible {{\n  \
           outline: 1px solid {accent};\n  \
           outline-offset: -1px;\n\
         }}\n\
         .{root} button.nuncio-primary:focus-visible, .{root} button.nuncio-send:focus-visible {{\n  \
           outline-offset: 1px;\n\
         }}\n\
         .{root} row.nuncio-message:focus-visible {{ outline: none; }}\n\
         .{root} button.nuncio-filter {{\n  \
           min-height: 32px;\n  \
           padding: 0 8px;\n  \
           background-color: transparent;\n  \
           background-image: none;\n  \
           color: {muted};\n  \
           font-weight: normal;\n  \
           border: none;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} button.nuncio-filter:hover {{ background-color: {hover}; }}\n\
         .{root} button.nuncio-filter:checked {{\n  \
           background-color: {active};\n  \
           color: {text};\n  \
           box-shadow: inset 2px 0 {accent};\n\
         }}\n\
         .{root} .nuncio-linked-mark {{\n  \
           min-width: 6px;\n  \
           min-height: 6px;\n  \
           background-color: {accent};\n\
         }}\n\
         .{root} .nuncio-composer {{\n  \
           min-height: {composer}px;\n  \
           background-color: {surface};\n  \
           border-top: 1px solid {border};\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-composer:focus-within {{ border-top-color: {accent}; }}\n\
         .{root} .nuncio-composer-row {{ padding: 0 12px 0 4px; }}\n\
         .{root} .nuncio-composer textview,\n\
         .{root} .nuncio-composer textview > text {{\n  \
           background-color: transparent;\n  \
           color: {text};\n  \
           font-size: 13px;\n\
         }}\n\
         .{root} button.nuncio-send {{\n  \
           min-height: 32px;\n  \
           padding: 0 12px;\n  \
           background-color: {accent};\n  \
           background-image: none;\n  \
           color: {accent_on};\n  \
           font-size: 12px;\n  \
           font-weight: 600;\n  \
           border: none;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} button.nuncio-send:hover {{ filter: brightness(1.08); }}\n\
         .{root} .nuncio-reply-strip {{\n  \
           padding: 0 4px 0 12px;\n  \
           border-bottom: 1px solid {border};\n\
         }}\n\
         .{root} .nuncio-reply-quote {{ font-size: 11px; color: {muted}; }}\n\
         .{root} menubutton.nuncio-icon-button > button, .{root} button.nuncio-icon-button {{\n  \
           min-width: 28px;\n  \
           min-height: 28px;\n  \
           padding: 0;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} menubutton.nuncio-icon-button > button:hover, .{root} button.nuncio-icon-button:hover {{\n  \
           background-color: {hover};\n\
         }}\n\
         .{root} button.nuncio-icon-button:checked {{ background-color: {active}; }}\n\
         .{root} .nuncio-strip {{\n  \
           padding: 6px 16px;\n  \
           color: {warning};\n  \
           border-bottom: 1px solid {border};\n\
         }}\n",
        chrome = palette.surface_chrome().to_hex(),
        surface = palette.surface().to_hex(),
        border = palette.border().to_hex(),
        text = palette.text().to_hex(),
        muted = palette.text_muted().to_hex(),
        hover = palette.surface_hover().to_hex(),
        active = palette.surface_active().to_hex(),
        accent = palette.accent().to_hex(),
        accent_on = palette.accent_on().to_hex(),
        warning = palette.warning().to_hex(),
        header = HEADER_HEIGHT,
        composer = COMPOSER_MIN_HEIGHT,
    ) + &list_and_transcript_css(palette)
        + &overlays_css(palette)
}

/// How long each of the design's animations takes: at most 120 ms (plan §4.4).
pub const MOTION_MS: u32 = 120;

/// The design's motion (plan §4.4): every transition in the app is off, libadwaita's own (some
/// 200 ms or longer) included, except the hover fill fading and the palette's scrim fading, and
/// those only while the desktop has animations on (`animations`, its `gtk-enable-animations`).
pub fn motion_css(animations: bool) -> String {
    let root = ROOT_CLASS;
    let mut css = format!(".{root}, .{root} * {{ transition: none; }}\n");
    if animations {
        css.push_str(&format!(
            ".{root} .nuncio-chats > row, .{root} .nuncio-results > row,\n\
             .{root} .nuncio-palette-list > row, .{root} button.nuncio-filter,\n\
             .{root} button.nuncio-icon-button, .{root} menubutton.nuncio-icon-button > button,\n\
             .{root} button.nuncio-dialog-primary {{\n  \
               transition: background-color {MOTION_MS}ms ease-out;\n\
             }}\n\
             .{root} .nuncio-panes {{ transition: opacity {MOTION_MS}ms ease-out; }}\n"
        ));
    }
    css
}

/// The transcript's measure, in characters of the message text (plan §5).
pub const TRANSCRIPT_MEASURE_CHARS: i32 = 72;

/// The transcript's width in a pane `available` pixels wide, for message text whose characters
/// are `char_width` pixels wide: [`TRANSCRIPT_MEASURE_CHARS`] characters at the left of the pane
/// (the room past them stays empty, on the right), all of a narrower pane, and never under `min`,
/// the least the transcript can take. No font measured yet (0) counts a pixel a character.
pub fn transcript_width(available: i32, char_width: i32, min: i32) -> i32 {
    available
        .min(TRANSCRIPT_MEASURE_CHARS * char_width.max(1))
        .max(min)
}

/// The rules for the command palette (plan §6.5), the inline search (§6.6), the settings
/// (§6.7) and the dialogs (§6.8).
fn overlays_css(palette: &Palette) -> String {
    let root = ROOT_CLASS;
    format!(
        ".{root} .nuncio-under {{ background-color: #000000; }}\n\
                 .{root} .nuncio-panes.nuncio-scrim-shown {{ opacity: 0.55; }}\n\
         popover.{root}.nuncio-palette > contents {{\n  \
           background-color: {chrome};\n  \
           color: {text};\n  \
           border: 1px solid {border};\n  \
           padding: 0;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-palette-input {{\n  \
           min-height: 44px;\n  \
           padding: 0 12px;\n  \
           border-bottom: 1px solid {border};\n\
         }}\n\
         .{root} entry.nuncio-palette-entry {{\n  \
           min-height: 44px;\n  \
           background-color: transparent;\n  \
           border: none;\n  \
           box-shadow: none;\n  \
           outline: none;\n  \
           font-size: 13px;\n  \
           color: {text};\n\
         }}\n\
         .{root} .nuncio-palette-list {{ background-color: transparent; }}\n\
         .{root} .nuncio-palette-list > row {{\n  \
           min-height: 36px;\n  \
           padding: 0 12px;\n  \
           font-size: 13px;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-palette-list > row:hover {{ background-color: {hover}; }}\n\
         .{root} .nuncio-palette-list > row:selected {{\n  \
           background-color: {active};\n  \
           color: {text};\n\
         }}\n\
         .{root} .nuncio-palette-key {{ font-size: 11px; color: {faint}; }}\n\
         .{root} .nuncio-palette-header {{\n  \
           padding: 8px 12px 4px 12px;\n  \
           font-size: 11px;\n  \
           letter-spacing: 0.06em;\n  \
           color: {faint};\n\
         }}\n\
         .{root} .nuncio-search {{\n  \
           min-height: 32px;\n  \
           background-color: {surface};\n  \
           color: {text};\n  \
           border: 1px solid {border};\n  \
           box-shadow: none;\n  \
           outline: none;\n\
         }}\n\
         .{root} .nuncio-search:focus-within {{ border-color: {accent}; }}\n\
         .{root} .nuncio-search-row {{ padding: 6px 12px; }}\n\
         .{root} .nuncio-search-hint {{ padding: 4px 16px; }}\n\
         .{root} .nuncio-results > row {{\n  \
           min-height: 36px;\n  \
           padding: 0 16px;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-results > row:hover {{ background-color: {hover}; }}\n\
         .{root} .nuncio-results > row:selected {{ background-color: {active}; }}\n\
         .{root}.nuncio-settings {{ background-color: {surface}; color: {text}; }}\n\
         .{root} .nuncio-settings-header {{\n  \
           padding: 24px 0 4px 0;\n  \
           font-size: 11px;\n  \
           letter-spacing: 0.06em;\n  \
           color: {muted};\n  \
           border-bottom: 1px solid {border};\n\
         }}\n\
         .{root} .nuncio-settings-list {{ background-color: transparent; }}\n\
         .{root} .nuncio-settings-list > row {{\n  \
           min-height: 40px;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-settings-list > row > box.header {{\n  \
           min-height: 40px;\n  \
           padding-top: 4px;\n  \
           padding-bottom: 4px;\n\
         }}\n\
         .{root} .nuncio-settings-list > row .title {{ font-size: 13px; color: {text}; }}\n\
         .{root} .nuncio-settings-list > row .subtitle {{ font-size: 11px; color: {muted}; }}\n\
         popover.{root}.nuncio-dialog > contents {{\n  \
           background-color: {surface};\n  \
           color: {text};\n  \
           border: 1px solid {border};\n  \
           padding: 16px;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-dialog-body {{ font-size: 13px; color: {text}; }}\n\
         .{root} .nuncio-dialog-error {{ font-size: 11px; color: {warning}; }}\n\
         .{root} button.nuncio-dialog-primary {{\n  \
           background-color: transparent;\n  \
           color: {accent};\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} button.nuncio-dialog-primary:hover {{ background-color: {hover}; }}\n",
        chrome = palette.surface_chrome().to_hex(),
        surface = palette.surface().to_hex(),
        border = palette.border().to_hex(),
        text = palette.text().to_hex(),
        muted = palette.text_muted().to_hex(),
        faint = palette.text_faint().to_hex(),
        hover = palette.surface_hover().to_hex(),
        active = palette.surface_active().to_hex(),
        accent = palette.accent().to_hex(),
        warning = palette.warning().to_hex(),
    )
}

/// How far an avatar's tone is mixed from the chrome toward its palette colour: a muted tone.
const AVATAR_TONE: f64 = 0.35;

/// The rules for the chat rows (plan §6.2), the bubbles and everything around them (plan §6.3),
/// the empty states, and the avatars' six muted tones (plan §4.3).
fn list_and_transcript_css(palette: &Palette) -> String {
    let root = ROOT_CLASS;
    let mut css = format!(
        ".{root} .nuncio-chats > row {{\n  \
           min-height: 72px;\n  \
           margin: 0;\n  \
           padding: 0;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-chats > row:hover {{ background-color: {hover}; }}\n\
         .{root} .nuncio-chats > row:selected {{\n  \
           background-color: {active};\n  \
           color: {text};\n  \
           box-shadow: inset 2px 0 {accent};\n\
         }}\n\
         .{root} .nuncio-unread-mark {{ background-color: {accent}; }}\n\
         .{root} .nuncio-name {{ font-size: 13px; font-weight: 600; color: {muted}; }}\n\
         .{root} .nuncio-name.nuncio-name-unread {{ color: {text}; }}\n\
         .{root} .nuncio-preview {{ font-size: 13px; color: {muted}; }}\n\
         .{root} .nuncio-preview-author {{ font-size: 13px; color: {text}; }}\n\
         .{root} .nuncio-ticks {{ font-size: 12px; color: {faint}; }}\n\
         .{root} .nuncio-faint {{ color: {faint}; }}\n\
         .{root} .nuncio-chip {{\n  \
           min-width: 16px;\n  \
           min-height: 16px;\n  \
           padding: 0 3px;\n  \
           font-size: 10px;\n  \
           background-color: {accent};\n  \
           color: {accent_on};\n\
         }}\n\
         .{root} .nuncio-transcript > row {{\n  \
           padding: 0;\n  \
           background-color: transparent;\n  \
           box-shadow: none;\n\
         }}\n\
         .{root} .nuncio-day {{\n  \
           margin-top: 16px;\n  \
           margin-bottom: 8px;\n  \
           font-size: 11px;\n  \
           letter-spacing: 0.06em;\n  \
           color: {faint};\n\
         }}\n\
         .{root} .nuncio-author-line {{ font-size: 11px; font-weight: 600; color: {text}; }}\n\
         .{root} .nuncio-bubble {{\n  \
           padding: 8px 12px;\n  \
           color: {text};\n\
         }}\n\
         .{root} .nuncio-bubble.nuncio-mine {{ background-color: {active}; }}\n\
         .{root} .nuncio-bubble.nuncio-theirs {{\n  \
           background-color: {chrome};\n  \
           border: 1px solid {border};\n\
         }}\n\
         .{root} .nuncio-bubble.nuncio-sticker {{\n  \
           padding: 0;\n  \
           background-color: transparent;\n  \
           border: none;\n\
         }}\n\
         .{root} .nuncio-text {{ font-size: 13px; }}\n\
         .{root} .nuncio-caption {{ font-size: 11px; color: {muted}; }}\n\
         .{root} .nuncio-media {{ font-size: 11px; color: {muted}; }}\n\
         .{root} .nuncio-meta-line {{ font-size: 11px; color: {faint}; }}\n\
         .{root} .nuncio-meta-line.nuncio-warning {{ color: {warning}; }}\n\
         .{root} .nuncio-quote {{\n  \
           border-left: 2px solid {accent};\n  \
           padding-left: 8px;\n\
         }}\n\
         .{root} .nuncio-quote-author {{ font-size: 11px; font-weight: 600; }}\n\
         .{root} .nuncio-quote-text {{ font-size: 11px; color: {muted}; }}\n\
         .{root} .nuncio-reaction {{\n  \
           padding: 0 4px;\n  \
           background-color: {surface};\n  \
           border: 1px solid {border};\n\
         }}\n\
         .{root} .nuncio-reaction.nuncio-reaction-mine {{ background-color: {active}; }}\n\
         .{root} .nuncio-reaction-emoji {{ font-size: 13px; }}\n\
         .{root} .nuncio-frame {{ border: 1px solid {border}; }}\n\
         .{root} .nuncio-document {{ min-height: 32px; }}\n\
         .{root} row.nuncio-message:focus-visible .nuncio-bubble {{\n  \
           outline: 1px solid {accent};\n  \
           outline-offset: 0;\n\
         }}\n",
        chrome = palette.surface_chrome().to_hex(),
        surface = palette.surface().to_hex(),
        border = palette.border().to_hex(),
        text = palette.text().to_hex(),
        muted = palette.text_muted().to_hex(),
        faint = palette.text_faint().to_hex(),
        hover = palette.surface_hover().to_hex(),
        active = palette.surface_active().to_hex(),
        accent = palette.accent().to_hex(),
        accent_on = palette.accent_on().to_hex(),
        warning = palette.warning().to_hex(),
    );
    // libadwaita picks one of fourteen saturated colours by name; they fold into six muted tones
    // of the palette, with the text on them.
    let tones = [
        palette.red,
        palette.green,
        palette.yellow,
        palette.blue,
        palette.magenta,
        palette.cyan,
    ];
    for n in 1..=14 {
        css.push_str(&format!(
            ".{root} avatar.color{n} {{ background-image: none; \
             background-color: mix({chrome}, {tone}, {AVATAR_TONE}); color: {text}; }}\n",
            chrome = palette.surface_chrome().to_hex(),
            tone = tones[(n - 1) % tones.len()].to_hex(),
            text = palette.text().to_hex(),
        ));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::{
        COMPOSER_LINES, COMPOSER_PAD, DIALOG_WIDTH, Layout, PALETTE_WIDTH, SETTINGS_WIDTH,
        THREE_PANES_FROM, TRANSCRIPT_MEASURE_CHARS, TWO_PANES_FROM, breakpoints,
        composer_max_height, dialog_width, layout_css, layout_for, palette_width, transcript_width,
    };
    use crate::{Look, ROOT_CLASS, WINDOW_CLASS, css_for};
    use nuncio_client::{Palette, Rgb};

    fn gruvbox() -> Palette {
        Palette {
            background: Rgb::new(0x28, 0x28, 0x28),
            foreground: Rgb::new(0xd4, 0xbe, 0x98),
            accent: Rgb::new(0x7d, 0xae, 0xa3),
            lighter_background: Some(Rgb::new(0x3c, 0x38, 0x36)),
            ..Palette::default()
        }
    }

    /// The body of the first rule whose selector is exactly `selector`.
    fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
        let start = css
            .find(&format!("{selector} {{"))
            .unwrap_or_else(|| panic!("no rule for {selector} in: {css}"));
        let body = &css[start + selector.len() + 2..];
        &body[..body.find('}').expect("a closed rule")]
    }

    /// Every `border-radius` value in `css`.
    fn radii(css: &str) -> Vec<&str> {
        css.match_indices("border-radius:")
            .map(|(at, key)| {
                let value = &css[at + key.len()..];
                value[..value.find(';').expect("a value")].trim()
            })
            .collect()
    }

    #[test]
    fn each_pane_class_is_painted_in_its_palette_role() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        for (selector, property, color) in [
            (
                "nuncio-chrome",
                "background-color",
                palette.surface_chrome(),
            ),
            ("nuncio-content", "background-color", palette.surface()),
            ("paned > separator", "background-color", palette.border()),
            ("headerbar.nuncio-header", "border-bottom", palette.border()),
            ("nuncio-meta", "color", palette.text_muted()),
            (
                "button.nuncio-primary",
                "background-color",
                palette.accent(),
            ),
            ("button.nuncio-primary", "color", palette.accent_on()),
            ("button.nuncio-filter", "color", palette.text_muted()),
            (
                "button.nuncio-filter:hover",
                "background-color",
                palette.surface_hover(),
            ),
            (
                "button.nuncio-filter:checked",
                "background-color",
                palette.surface_active(),
            ),
            (
                "button.nuncio-filter:checked",
                "box-shadow",
                palette.accent(),
            ),
            ("nuncio-linked-mark", "background-color", palette.accent()),
            ("nuncio-strip", "color", palette.warning()),
            // The chat list (plan §6.2).
            (
                "nuncio-chats > row:hover",
                "background-color",
                palette.surface_hover(),
            ),
            (
                "nuncio-chats > row:selected",
                "background-color",
                palette.surface_active(),
            ),
            (
                "nuncio-chats > row:selected",
                "box-shadow",
                palette.accent(),
            ),
            ("nuncio-unread-mark", "background-color", palette.accent()),
            ("nuncio-name", "color", palette.text_muted()),
            ("nuncio-name.nuncio-name-unread", "color", palette.text()),
            ("nuncio-preview", "color", palette.text_muted()),
            ("nuncio-preview-author", "color", palette.text()),
            ("nuncio-ticks", "color", palette.text_faint()),
            ("nuncio-faint", "color", palette.text_faint()),
            ("nuncio-chip", "background-color", palette.accent()),
            ("nuncio-chip", "color", palette.accent_on()),
            // The transcript (plan §6.3).
            ("nuncio-day", "color", palette.text_faint()),
            (
                "nuncio-bubble.nuncio-mine",
                "background-color",
                palette.surface_active(),
            ),
            (
                "nuncio-bubble.nuncio-theirs",
                "background-color",
                palette.surface_chrome(),
            ),
            ("nuncio-bubble.nuncio-theirs", "border", palette.border()),
            ("nuncio-bubble", "color", palette.text()),
            ("nuncio-meta-line", "color", palette.text_faint()),
            (
                "nuncio-meta-line.nuncio-warning",
                "color",
                palette.warning(),
            ),
            ("nuncio-quote", "border-left", palette.accent()),
            ("nuncio-quote-text", "color", palette.text_muted()),
            ("nuncio-reaction", "background-color", palette.surface()),
            ("nuncio-reaction", "border", palette.border()),
            ("nuncio-frame", "border", palette.border()),
            ("nuncio-caption", "color", palette.text_muted()),
        ] {
            let selector = if selector.starts_with("nuncio-") {
                format!("{root} .{selector}")
            } else if selector.contains(['.', ' ']) {
                format!("{root} {selector}")
            } else {
                format!("{root} .{selector}")
            };
            let body = rule(&css, &selector);
            let declaration = body
                .split(';')
                .find(|d| d.trim_start().starts_with(&format!("{property}:")))
                .unwrap_or_else(|| panic!("{selector} has no {property}: {body}"));
            assert!(
                declaration.contains(&color.to_hex()),
                "{selector} {property} is not {}: {declaration}",
                color.to_hex()
            );
        }
    }

    #[test]
    fn rows_are_72_px_bubbles_square_padded_and_stickers_bare() {
        let css = layout_css(&gruvbox(), 0);
        let root = format!(".{ROOT_CLASS}");
        let row = rule(&css, &format!("{root} .nuncio-chats > row"));
        assert!(row.contains("min-height: 72px;"), "{row}");
        let bubble = rule(&css, &format!("{root} .nuncio-bubble"));
        assert!(bubble.contains("padding: 8px 12px;"), "{bubble}");
        let chip = rule(&css, &format!("{root} .nuncio-chip"));
        assert!(
            chip.contains("min-width: 16px;") && chip.contains("min-height: 16px;"),
            "{chip}"
        );
        assert!(chip.contains("font-size: 10px;"), "{chip}");
        let sticker = rule(&css, &format!("{root} .nuncio-bubble.nuncio-sticker"));
        for bare in [
            "background-color: transparent;",
            "border: none;",
            "padding: 0;",
        ] {
            assert!(sticker.contains(bare), "{bare}: {sticker}");
        }
        assert!(
            !css.contains("box-shadow: 0") && !css.contains("gradient"),
            "no shadows or gradients: {css}"
        );
    }

    #[test]
    fn avatars_take_six_muted_tones_of_the_palette() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        for n in 1..=14 {
            let body = rule(&css, &format!("{root} avatar.color{n}"));
            assert!(body.contains("background-image: none;"), "{body}");
            assert!(body.contains(&palette.surface_chrome().to_hex()), "{body}");
            assert!(body.contains(&palette.text().to_hex()), "{body}");
        }
        assert_eq!(
            rule(&css, &format!("{root} avatar.color1")),
            rule(&css, &format!("{root} avatar.color7")),
            "fourteen libadwaita colours fold into six tones"
        );
    }

    #[test]
    fn headers_are_44_px_and_separators_1_px() {
        let css = layout_css(&gruvbox(), 0);
        let root = format!(".{ROOT_CLASS}");
        assert!(
            rule(&css, &format!("{root} headerbar.nuncio-header")).contains("min-height: 44px;")
        );
        let separator = rule(&css, &format!("{root} paned > separator"));
        assert!(separator.contains("min-width: 1px;"), "{separator}");
        assert!(separator.contains("min-height: 1px;"), "{separator}");
    }

    #[test]
    fn a_rounding_of_0_leaves_no_radius_anywhere_the_window_included() {
        let look = Look {
            corner_radius: Some(0),
            ..Look::default()
        };
        let css = css_for(&gruvbox(), &look);
        let radii = radii(&css);
        assert!(!radii.is_empty(), "{css}");
        assert!(radii.iter().all(|&r| r == "0px"), "{radii:?} in: {css}");
        assert!(
            rule(&css, &format!(".{ROOT_CLASS}, .{ROOT_CLASS} *")).contains("border-radius: 0px;"),
            "every element inside the app is square: {css}"
        );
        assert!(
            css.contains(&format!("window.{WINDOW_CLASS}")),
            "the window itself is squared too: {css}"
        );
    }

    #[test]
    fn every_radius_is_hyprlands_rounding() {
        let look = Look {
            corner_radius: Some(10),
            ..Look::default()
        };
        let css = css_for(&gruvbox(), &look);
        let radii = radii(&css);
        assert!(radii.iter().all(|&r| r == "10px"), "{radii:?} in: {css}");
    }

    #[test]
    fn without_hyprland_the_elements_and_the_window_are_square() {
        let css = css_for(&gruvbox(), &Look::default());
        assert!(radii(&css).iter().all(|&r| r == "0px"), "{css}");
        assert!(
            css.contains(&format!("window.{WINDOW_CLASS}")),
            "the window is squared too, not left to libadwaita: {css}"
        );
    }

    #[test]
    fn a_theme_switch_produces_new_css() {
        let light = Palette {
            background: Rgb::new(0xff, 0xfc, 0xf0),
            foreground: Rgb::new(0x10, 0x0f, 0x0f),
            accent: Rgb::new(0x20, 0x5e, 0xa6),
            ..Palette::default()
        };
        let dark = layout_css(&gruvbox(), 0);
        let switched = layout_css(&light, 0);
        assert_ne!(dark, switched);
        assert!(
            switched.contains(&light.surface_chrome().to_hex()),
            "{switched}"
        );
        assert!(
            !switched.contains(&gruvbox().surface_chrome().to_hex()),
            "{switched}"
        );
    }

    #[test]
    fn the_layout_follows_the_tile_width() {
        for (width, layout) in [
            (1920, Layout::ThreePanes),
            (THREE_PANES_FROM, Layout::ThreePanes),
            (THREE_PANES_FROM - 1, Layout::CompactRail),
            (960, Layout::CompactRail),
            (TWO_PANES_FROM, Layout::CompactRail),
            (TWO_PANES_FROM - 1, Layout::SinglePane),
            (600, Layout::SinglePane),
        ] {
            assert_eq!(layout_for(width), layout, "at {width} px");
        }
        assert_eq!((THREE_PANES_FROM, TWO_PANES_FROM), (1100, 760));
    }

    #[test]
    fn the_breakpoints_switch_where_the_layout_does() {
        let [(compact, first), (single, second)] = breakpoints();
        assert_eq!(
            (first, second),
            (Layout::CompactRail, Layout::SinglePane),
            "widest first: libadwaita applies the last match"
        );
        let max = |condition: &str| -> i32 {
            condition
                .strip_prefix("max-width: ")
                .and_then(|px| px.strip_suffix("px"))
                .and_then(|px| px.parse().ok())
                .unwrap_or_else(|| panic!("{condition}"))
        };
        assert_eq!(layout_for(max(&compact)), Layout::CompactRail);
        assert_eq!(layout_for(max(&compact) + 1), Layout::ThreePanes);
        assert_eq!(layout_for(max(&single)), Layout::SinglePane);
        assert_eq!(layout_for(max(&single) + 1), Layout::CompactRail);
    }

    #[test]
    fn the_composer_grows_to_six_lines_then_scrolls() {
        assert_eq!(COMPOSER_LINES, 6);
        for line in [15, 17, 20] {
            let max = composer_max_height(line);
            assert_eq!(
                max,
                6 * line + 2 * COMPOSER_PAD,
                "six lines and the padding"
            );
            assert!(max < 7 * line + 2 * COMPOSER_PAD, "a seventh line scrolls");
        }
        assert_eq!(
            composer_max_height(0),
            6 + 2 * COMPOSER_PAD,
            "no font measured yet: at least a pixel a line"
        );
    }

    #[test]
    fn the_composer_is_a_48_px_bar_with_a_line_that_turns_accent_on_focus() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        let bar = rule(&css, &format!("{root} .nuncio-composer"));
        assert!(bar.contains("min-height: 48px;"), "{bar}");
        assert!(
            bar.contains(&format!(
                "border-top: 1px solid {};",
                palette.border().to_hex()
            )),
            "{bar}"
        );
        let focused = rule(&css, &format!("{root} .nuncio-composer:focus-within"));
        assert!(
            focused.contains(&format!("border-top-color: {};", palette.accent().to_hex())),
            "{focused}"
        );
        let send = rule(&css, &format!("{root} button.nuncio-send"));
        for declaration in [
            "min-height: 32px;".to_owned(),
            format!("background-color: {};", palette.accent().to_hex()),
            format!("color: {};", palette.accent_on().to_hex()),
            "font-size: 12px;".to_owned(),
            "font-weight: 600;".to_owned(),
        ] {
            assert!(send.contains(&declaration), "{declaration} in {send}");
        }
        assert!(
            !send.contains("border-radius"),
            "the radius is Hyprland's: {send}"
        );
        let strip = rule(&css, &format!("{root} .nuncio-reply-strip"));
        assert!(
            strip.contains(&format!(
                "border-bottom: 1px solid {};",
                palette.border().to_hex()
            )),
            "{strip}"
        );
        let quote = rule(&css, &format!("{root} .nuncio-reply-quote"));
        assert!(quote.contains("font-size: 11px;"), "{quote}");
        assert!(
            quote.contains(&format!("color: {};", palette.text_muted().to_hex())),
            "{quote}"
        );
        let close = rule(&css, &format!("{root} button.nuncio-icon-button"));
        assert!(close.contains("min-width: 28px;"), "{close}");
        assert!(close.contains("min-height: 28px;"), "{close}");
    }

    /// Every declaration of `wanted` is in the rule for `selector`.
    fn has(css: &str, selector: &str, wanted: &[String]) {
        let body = rule(css, selector);
        for declaration in wanted {
            assert!(
                body.contains(declaration.as_str()),
                "{declaration} in {selector}: {body}"
            );
        }
    }

    #[test]
    fn the_palette_is_a_560_px_chrome_panel_over_a_dark_scrim() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        let (chrome, border) = (palette.surface_chrome().to_hex(), palette.border().to_hex());
        has(
            &css,
            &format!("{root} .nuncio-under"),
            &["background-color: #000000;".to_owned()],
        );
        has(
            &css,
            &format!("{root} .nuncio-panes.nuncio-scrim-shown"),
            &["opacity: 0.55;".to_owned()],
        );
        has(
            &css,
            &format!("popover{root}.nuncio-palette > contents"),
            &[
                format!("background-color: {chrome};"),
                format!("border: 1px solid {border};"),
                "padding: 0;".to_owned(),
                "box-shadow: none;".to_owned(),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-palette-input"),
            &[
                "min-height: 44px;".to_owned(),
                format!("border-bottom: 1px solid {border};"),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-palette-list > row"),
            &[
                "min-height: 36px;".to_owned(),
                "font-size: 13px;".to_owned(),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-palette-list > row:selected"),
            &[format!(
                "background-color: {};",
                palette.surface_active().to_hex()
            )],
        );
        let faint = palette.text_faint().to_hex();
        for meta in [".nuncio-palette-key", ".nuncio-palette-header"] {
            has(
                &css,
                &format!("{root} {meta}"),
                &["font-size: 11px;".to_owned(), format!("color: {faint};")],
            );
        }
        assert_eq!(PALETTE_WIDTH, 560);
        assert_eq!(palette_width(1920), 560);
        assert_eq!(palette_width(0), 560, "a window not laid out yet");
        assert_eq!(palette_width(400), 368, "a 16 px gutter on a narrow window");
    }

    #[test]
    fn the_search_input_is_32_px_with_a_1_px_line_that_turns_accent_on_focus() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        has(
            &css,
            &format!("{root} .nuncio-search"),
            &[
                "min-height: 32px;".to_owned(),
                format!("border: 1px solid {};", palette.border().to_hex()),
                "box-shadow: none;".to_owned(),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-search:focus-within"),
            &[format!("border-color: {};", palette.accent().to_hex())],
        );
    }

    #[test]
    fn the_settings_are_a_480_px_column_of_40_px_rows_under_meta_headers() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        let (muted, border) = (palette.text_muted().to_hex(), palette.border().to_hex());
        has(
            &css,
            &format!("{root}.nuncio-settings"),
            &[format!("background-color: {};", palette.surface().to_hex())],
        );
        has(
            &css,
            &format!("{root} .nuncio-settings-header"),
            &[
                "font-size: 11px;".to_owned(),
                "letter-spacing: 0.06em;".to_owned(),
                format!("color: {muted};"),
                format!("border-bottom: 1px solid {border};"),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-settings-list > row"),
            &[
                "min-height: 40px;".to_owned(),
                "box-shadow: none;".to_owned(),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-settings-list > row .title"),
            &["font-size: 13px;".to_owned()],
        );
        has(
            &css,
            &format!("{root} .nuncio-settings-list > row .subtitle"),
            &["font-size: 11px;".to_owned(), format!("color: {muted};")],
        );
        assert_eq!(SETTINGS_WIDTH, 480);
    }

    #[test]
    fn a_dialog_is_a_480_px_surface_with_a_1_px_border_and_an_accent_text_button() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        has(
            &css,
            &format!("popover{root}.nuncio-dialog > contents"),
            &[
                format!("background-color: {};", palette.surface().to_hex()),
                format!("color: {};", palette.text().to_hex()),
                format!("border: 1px solid {};", palette.border().to_hex()),
                "box-shadow: none;".to_owned(),
            ],
        );
        has(
            &css,
            &format!("{root} .nuncio-dialog-body"),
            &["font-size: 13px;".to_owned()],
        );
        // The primary is a text button in `accent`, not filled (plan §6.8).
        let primary = rule(&css, &format!("{root} button.nuncio-dialog-primary"));
        assert!(
            primary.contains(&format!("color: {};", palette.accent().to_hex())),
            "{primary}"
        );
        assert!(
            primary.contains("background-color: transparent;"),
            "{primary}"
        );
        has(
            &css,
            &format!("{root} .nuncio-dialog-error"),
            &[
                "font-size: 11px;".to_owned(),
                format!("color: {};", palette.warning().to_hex()),
            ],
        );
        assert_eq!(DIALOG_WIDTH, 480);
        assert_eq!(dialog_width(1920), 480);
        assert_eq!(dialog_width(0), 480, "a window not laid out yet");
        assert_eq!(dialog_width(400), 368, "a 16 px gutter on a narrow window");
    }

    /// Every `transition` value in `css`.
    fn transitions(css: &str) -> Vec<&str> {
        css.match_indices("transition:")
            .map(|(at, key)| {
                let value = &css[at + key.len()..];
                value[..value.find(';').expect("a value")].trim()
            })
            .collect()
    }

    #[test]
    fn the_hover_fill_and_the_scrim_fade_in_120_ms_and_nothing_else_moves() {
        let look = Look {
            animations: true,
            ..Look::default()
        };
        let css = css_for(&gruvbox(), &look);
        let root = format!(".{ROOT_CLASS}");
        assert!(
            css.contains(&format!("{root}, {root} * {{ transition: none; }}")),
            "libadwaita's own transitions are off: {css}"
        );
        for (selector, transition) in [
            ("nuncio-chats > row", "background-color 120ms ease-out"),
            ("nuncio-results > row", "background-color 120ms ease-out"),
            (
                "nuncio-palette-list > row",
                "background-color 120ms ease-out",
            ),
            ("button.nuncio-filter", "background-color 120ms ease-out"),
            (
                "button.nuncio-icon-button",
                "background-color 120ms ease-out",
            ),
            ("nuncio-panes", "opacity 120ms ease-out"),
        ] {
            let selector = if selector.starts_with("nuncio-") {
                format!("{root} .{selector}")
            } else {
                format!("{root} {selector}")
            };
            let at = css
                .find(&format!("{selector},"))
                .or_else(|| css.find(&format!("{selector} {{ transition")))
                .unwrap_or_else(|| panic!("{selector} has no transition: {css}"));
            let body = &css[at..];
            let body = &body[body.find('{').expect("a rule")..body.find('}').expect("a rule")];
            assert!(
                body.contains(&format!("transition: {transition};")),
                "{selector}: {body}"
            );
        }
        for value in transitions(&css) {
            let ms = value
                .split_whitespace()
                .filter_map(|word| word.strip_suffix("ms"))
                .map(|ms| ms.parse::<u32>().expect("a duration in ms"))
                .max()
                .unwrap_or(0);
            assert!(ms <= 120, "{value}: at most 120 ms");
            assert!(!value.contains("ms") || ms > 0, "{value}");
        }
        assert!(
            !css.contains("transition: all") && !css.contains("animation:"),
            "{css}"
        );
    }

    #[test]
    fn with_animations_off_nothing_moves() {
        let css = css_for(&gruvbox(), &Look::default());
        let all = transitions(&css);
        assert!(
            !all.is_empty(),
            "libadwaita's transitions are turned off: {css}"
        );
        assert!(all.iter().all(|&t| t == "none"), "{all:?} in: {css}");
    }

    #[test]
    fn the_scrim_is_shown_by_its_opacity_so_it_can_fade() {
        let css = layout_css(&gruvbox(), 0);
        let root = format!(".{ROOT_CLASS}");
        has(
            &css,
            &format!("{root} .nuncio-panes.nuncio-scrim-shown"),
            &["opacity: 0.55;".to_owned()],
        );
    }

    /// The scrim must darken what is behind it on **both** surfaces: with a colour equal to
    /// the content surface it is invisible over the conversation, which is what the first run on
    /// a real screen showed.
    #[test]
    fn the_scrim_darkens_the_content_and_the_chrome() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let body = rule(
            &css,
            &format!(".{ROOT_CLASS} .nuncio-panes.nuncio-scrim-shown"),
        );
        let start = body.find("opacity: ").expect("an opacity") + "opacity: ".len();
        let alpha: f64 = body[start..start + body[start..].find(';').expect("a value")]
            .parse()
            .expect("a number");
        // The panes are drawn at that opacity over the black backdrop, so each tone lands at
        // `tone * alpha` over black.
        let over = |base: Rgb| {
            let mix = |b: u8| (f64::from(b) * alpha).round() as u8;
            Rgb::new(mix(base.r), mix(base.g), mix(base.b))
        };
        let luminance =
            |c: Rgb| 0.2126 * f64::from(c.r) + 0.7152 * f64::from(c.g) + 0.0722 * f64::from(c.b);
        for (name, base) in [
            ("content", palette.surface()),
            ("chrome", palette.surface_chrome()),
        ] {
            let dimmed = over(base);
            assert!(
                luminance(dimmed) < luminance(base) * 0.95,
                "the scrim must darken {name}: {} over {} at alpha {alpha}",
                dimmed.to_hex(),
                base.to_hex()
            );
        }
    }

    #[test]
    fn focus_is_a_1_px_accent_ring_never_a_glow() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        let ring = format!("outline: 1px solid {};", palette.accent().to_hex());
        assert!(
            css.contains(&format!(
                "{root} button:focus-visible, {root} row:focus-visible {{"
            )),
            "every button and row: {css}"
        );
        has(&css, &format!("{root} row:focus-visible"), &[ring]);
        // On a filled accent button the ring goes outside it, where it shows.
        has(
            &css,
            &format!("{root} button.nuncio-send:focus-visible"),
            &["outline-offset: 1px;".to_owned()],
        );
        assert!(
            css.contains(&format!(
                "{root} button.nuncio-primary:focus-visible, {root} button.nuncio-send:focus-visible"
            )),
            "{css}"
        );
        // A message shows its focus on its bubble, not around the whole row.
        has(
            &css,
            &format!("{root} row.nuncio-message:focus-visible"),
            &["outline: none;".to_owned()],
        );
    }

    #[test]
    fn icon_buttons_fill_on_hover_and_while_checked_and_send_lightens() {
        let palette = gruvbox();
        let css = layout_css(&palette, 0);
        let root = format!(".{ROOT_CLASS}");
        has(
            &css,
            &format!("{root} button.nuncio-icon-button:checked"),
            &[format!(
                "background-color: {};",
                palette.surface_active().to_hex()
            )],
        );
        assert!(
            css.contains(&format!(
                "{root} menubutton.nuncio-icon-button > button, {root} button.nuncio-icon-button {{"
            )),
            "the menu's button is a 28 px icon button too: {css}"
        );
        assert!(
            css.contains(&format!(
                "{root} menubutton.nuncio-icon-button > button:hover, {root} button.nuncio-icon-button:hover {{"
            )),
            "{css}"
        );
        has(
            &css,
            &format!("{root} button.nuncio-send:hover"),
            &["filter: brightness(1.08);".to_owned()],
        );
    }

    #[test]
    fn the_transcript_is_72_characters_wide_at_most() {
        assert_eq!(TRANSCRIPT_MEASURE_CHARS, 72);
        assert_eq!(transcript_width(1311, 8, 100), 576, "72 characters of 8 px");
        assert_eq!(
            transcript_width(500, 8, 100),
            500,
            "a narrower pane: all of it"
        );
        assert_eq!(
            transcript_width(500, 8, 600),
            600,
            "never under what it needs"
        );
        assert_eq!(transcript_width(1311, 0, 0), 72, "no font measured yet");
    }
}
