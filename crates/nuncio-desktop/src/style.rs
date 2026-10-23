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
    )
}

#[cfg(test)]
mod tests {
    use super::{Layout, THREE_PANES_FROM, TWO_PANES_FROM, breakpoints, layout_css, layout_for};
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
        ] {
            let selector = if selector.contains(['.', ' ']) {
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
}
