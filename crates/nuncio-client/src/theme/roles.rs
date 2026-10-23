//! Semantic colour roles over the Omarchy palette (design plan §4.1).
//!
//! Both clients colour themselves by role (`surface`, `text_muted`, `accent_on`…), never by raw
//! ANSI name. Derived tones are mixes of two palette colours in sRGB.

use super::{Palette, Rgb};

/// The lowest contrast `text_muted` may have on either surface (WCAG AA, plan §4.1).
const MUTED_MIN_CONTRAST: f64 = 4.5;

impl Palette {
    /// The content pane (transcript, reader): `background`.
    pub fn surface(&self) -> Rgb {
        self.background
    }

    /// The sidebar and the chat list: the other background tone, one step toward the text.
    ///
    /// That is `lighter_background` in a dark theme, but Omarchy's light themes write a
    /// *darker* tone under that name, so the tone is picked by luminance: `lighter_background`
    /// is the chrome only when it sits on the text's side of `background`. Otherwise (absent,
    /// equal to `background`, or on the wrong side) the chrome is `surface` 8 % toward `text`.
    pub fn surface_chrome(&self) -> Rgb {
        let surface = luminance(self.surface());
        let toward_text = |tone: Rgb| {
            (luminance(tone) > surface) == (luminance(self.text()) > surface)
                && luminance(tone) != surface
        };
        match self.lighter_background {
            Some(tone) if toward_text(tone) => tone,
            _ => mix(self.surface(), self.text(), 0.08),
        }
    }

    /// A hovered row: `surface_chrome` 6 % toward `text`.
    pub fn surface_hover(&self) -> Rgb {
        mix(self.surface_chrome(), self.text(), 0.06)
    }

    /// The selected row and the active tab: `surface_chrome` 10 % toward `text`.
    pub fn surface_active(&self) -> Rgb {
        mix(self.surface_chrome(), self.text(), 0.10)
    }

    /// 1 px lines: `darker_background`, or `surface` 10 % toward `text`.
    pub fn border(&self) -> Rgb {
        self.darker_background
            .unwrap_or_else(|| mix(self.surface(), self.text(), 0.10))
    }

    /// Names, subjects and message text: `foreground`, never adjusted.
    pub fn text(&self) -> Rgb {
        self.foreground
    }

    /// Previews, times and meta: `light_foreground`, or `text` at 70 % over `surface`.
    ///
    /// When that tone is under 4.5:1 on `surface` or `surface_chrome`, it moves toward `text`
    /// just far enough to reach 4.5:1 (or becomes `text` if even that cannot).
    pub fn text_muted(&self) -> Rgb {
        let given = self
            .light_foreground
            .unwrap_or_else(|| mix(self.surface(), self.text(), 0.70));
        let (surface, chrome) = (self.surface(), self.surface_chrome());
        (0..=100)
            .map(|step| mix(given, self.text(), f64::from(step) / 100.0))
            .find(|tone| {
                contrast(*tone, surface) >= MUTED_MIN_CONTRAST
                    && contrast(*tone, chrome) >= MUTED_MIN_CONTRAST
            })
            .unwrap_or_else(|| self.text())
    }

    /// Disabled items, tick glyphs and placeholders: `dark_foreground`, or `text` at 50 % over
    /// `surface`.
    pub fn text_faint(&self) -> Rgb {
        self.dark_foreground
            .unwrap_or_else(|| mix(self.surface(), self.text(), 0.50))
    }

    /// The primary button, the unread dot and the selection bar.
    pub fn accent(&self) -> Rgb {
        self.accent
    }

    /// The label on a filled accent button: `surface` or `text`, whichever contrasts more with
    /// `accent`.
    pub fn accent_on(&self) -> Rgb {
        let (surface, text) = (self.surface(), self.text());
        if contrast(surface, self.accent) >= contrast(text, self.accent) {
            surface
        } else {
            text
        }
    }

    /// Destructive confirmations only.
    pub fn danger(&self) -> Rgb {
        self.red
    }

    /// "Not confirmed" and link warnings.
    pub fn warning(&self) -> Rgb {
        self.yellow
    }

    /// Sent ticks, when a colour is needed at all.
    pub fn success(&self) -> Rgb {
        self.green
    }
}

/// `from` moved `amount` (0.0–1.0) of the way to `to`, per sRGB channel.
fn mix(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    let channel = |a: u8, b: u8| {
        let (a, b) = (f64::from(a), f64::from(b));
        // In 0..=255 by construction, so the cast cannot truncate.
        (a + (b - a) * amount).round() as u8
    };
    Rgb::new(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
    )
}

/// WCAG 2 relative luminance.
fn luminance(colour: Rgb) -> f64 {
    let channel = |value: u8| {
        let c = f64::from(value) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(colour.r) + 0.7152 * channel(colour.g) + 0.0722 * channel(colour.b)
}

/// WCAG 2 contrast ratio, from 1.0 (identical) to 21.0 (black on white).
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::{Palette, Rgb, contrast, luminance};

    /// Omarchy's Gruvbox (dark) `colors.toml`, verbatim (basecamp/omarchy, themes/gruvbox).
    const GRUVBOX: &str = r##"
mode = "dark"

accent = "#7daea3"
selection = "#504945"
muted = "#665c54"

background = "#282828"
dark_background = "#1e1e1e"
darker_background = "#161616"
lighter_background = "#3c3836"

foreground = "#d4be98"
dark_foreground = "#7c6f64"
light_foreground = "#bdae93"
bright_foreground = "#d4be98"

red = "#ea6962"
yellow = "#d8a657"
orange = "#e1875c"
green = "#a9b665"
cyan = "#89b482"
blue = "#7daea3"
magenta = "#d3869b"
brown = "#70432e"

bright_red = "#ea6962"
bright_yellow = "#d8a657"
bright_green = "#a9b665"
bright_cyan = "#89b482"
bright_blue = "#7daea3"
bright_magenta = "#d3869b"
"##;

    /// Omarchy's Flexoki Light `colors.toml`, verbatim (basecamp/omarchy, themes/flexoki-light).
    const FLEXOKI_LIGHT: &str = r##"
mode = "light"

accent = "#205EA6"
selection = "#CECDC3"
muted = "#B7B5AC"

background = "#FFFCF0"
dark_background = "#f2efe4"
darker_background = "#e5e2d8"
lighter_background = "#E6E4D9"

foreground = "#100F0F"
dark_foreground = "#878580"
light_foreground = "#403E3C"
bright_foreground = "#100F0F"

red = "#D14D41"
yellow = "#D0A215"
orange = "#d0772b"
green = "#879A39"
cyan = "#3AA99F"
blue = "#205EA6"
magenta = "#CE5D97"
brown = "#683b15"

bright_red = "#D14D41"
bright_yellow = "#D0A215"
bright_green = "#879A39"
bright_cyan = "#3AA99F"
bright_blue = "#4385BE"
bright_magenta = "#CE5D97"
"##;

    /// Omarchy's Miasma `colors.toml`, verbatim (basecamp/omarchy, themes/miasma): its
    /// `light_foreground` is only 4.00:1 on its `lighter_background`.
    const MIASMA: &str = r##"
mode = "dark"

accent = "#78824b"
selection = "#383838"
muted = "#666666"

background = "#222222"
dark_background = "#191919"
darker_background = "#121212"
lighter_background = "#2c2c2c"

foreground = "#c2c2b0"
dark_foreground = "#555555"
light_foreground = "#8a8a7e"
bright_foreground = "#c2c2b0"

red = "#685742"
yellow = "#b36d43"
orange = "#8d6242"
green = "#5f875f"
cyan = "#c9a554"
blue = "#78824b"
magenta = "#bb7744"
brown = "#463121"

bright_red = "#685742"
bright_yellow = "#b36d43"
bright_green = "#5f875f"
bright_cyan = "#c9a554"
bright_blue = "#78824b"
bright_magenta = "#bb7744"
"##;

    /// Omarchy's Catppuccin Latte `colors.toml`, the keys the roles read (basecamp/omarchy,
    /// themes/catppuccin-latte).
    const CATPPUCCIN_LATTE: &str = r##"
mode = "light"
accent = "#1e66f5"
background = "#eff1f5"
darker_background = "#d7d8dc"
lighter_background = "#dce0e8"
foreground = "#4c4f69"
dark_foreground = "#9ca0b0"
light_foreground = "#5c5f77"
"##;

    fn parse(text: &str) -> Palette {
        Palette::from_toml_str(text).expect("the fixture parses")
    }

    fn hex(text: &str) -> Rgb {
        Rgb::from_hex(text).expect("a fixture colour")
    }

    /// The §4.1 contract, every pair but `text` on `surface_chrome` (see
    /// [`text_reaches_7_to_1_on_the_chrome_of_gruvbox_and_latte`]).
    fn assert_contract_without_text_on_chrome(palette: &Palette) {
        let text_on_surface = contrast(palette.text(), palette.surface());
        assert!(
            text_on_surface >= 7.0,
            "text on surface {text_on_surface:.2}"
        );
        for (name, surface) in [
            ("surface", palette.surface()),
            ("surface_chrome", palette.surface_chrome()),
        ] {
            let muted = contrast(palette.text_muted(), surface);
            assert!(muted >= 4.5, "text_muted on {name} {muted:.2}");
        }
        let label = contrast(palette.accent_on(), palette.accent());
        assert!(label >= 4.5, "accent_on on accent {label:.2}");
    }

    #[test]
    fn the_new_optional_keys_are_read() {
        let palette = parse(GRUVBOX);
        assert_eq!(palette.lighter_background, Some(hex("#3c3836")));
        assert_eq!(palette.darker_background, Some(hex("#161616")));
        assert_eq!(palette.dark_foreground, Some(hex("#7c6f64")));
        assert_eq!(palette.light_foreground, Some(hex("#bdae93")));
        assert_eq!(palette.bright_foreground, Some(hex("#d4be98")));
    }

    #[test]
    fn gruvbox_dark_maps_to_the_roles_of_the_plan() {
        let palette = parse(GRUVBOX);
        assert_eq!(palette.surface(), hex("#282828"));
        assert_eq!(palette.surface_chrome(), hex("#3c3836"));
        assert_eq!(palette.border(), hex("#161616"));
        assert_eq!(palette.text(), hex("#d4be98"));
        assert_eq!(palette.text_muted(), hex("#bdae93"));
        assert_eq!(palette.text_faint(), hex("#7c6f64"));
        assert_eq!(palette.accent(), hex("#7daea3"));
        assert_eq!(palette.danger(), hex("#ea6962"));
        assert_eq!(palette.warning(), hex("#d8a657"));
        assert_eq!(palette.success(), hex("#a9b665"));
        // Hover and active are steps from the chrome toward the text, active the larger one.
        let chrome = luminance(palette.surface_chrome());
        let hover = luminance(palette.surface_hover());
        let active = luminance(palette.surface_active());
        let text = luminance(palette.text());
        assert!(chrome < hover && hover < active && active < text);
    }

    #[test]
    fn a_dark_palette_meets_the_contrast_contract() {
        assert_contract_without_text_on_chrome(&parse(GRUVBOX));
    }

    #[test]
    fn a_light_palette_meets_the_contrast_contract() {
        let palette = parse(FLEXOKI_LIGHT);
        assert_contract_without_text_on_chrome(&palette);
        let text_on_chrome = contrast(palette.text(), palette.surface_chrome());
        assert!(text_on_chrome >= 7.0, "text on chrome {text_on_chrome:.2}");
    }

    /// The plan's `text` ≥ 7:1 on `surface_chrome` does not hold on two real Omarchy themes
    /// with the plan's own mapping (`foreground` on `lighter_background`): Gruvbox 6.42:1,
    /// Catppuccin Latte 6.04:1. `text` may never change (S4), so this waits for a decision.
    #[test]
    #[ignore = "open decision: Gruvbox and Latte foreground is under 7:1 on lighter_background"]
    fn text_reaches_7_to_1_on_the_chrome_of_gruvbox_and_latte() {
        for fixture in [GRUVBOX, CATPPUCCIN_LATTE] {
            let palette = parse(fixture);
            let ratio = contrast(palette.text(), palette.surface_chrome());
            assert!(ratio >= 7.0, "text on chrome {ratio:.2}");
        }
    }

    #[test]
    fn a_light_palette_takes_its_chrome_from_the_darker_background() {
        // In Omarchy's light themes `lighter_background` is the darker tone, so it is the chrome.
        let palette = parse(FLEXOKI_LIGHT);
        assert_eq!(palette.surface_chrome(), hex("#e6e4d9"));

        // A light palette whose `lighter_background` really is lighter than the background:
        // picked by luminance, the chrome must still be darker than the surface.
        let palette = parse(
            "mode = \"light\"\nbackground = \"#f0f0f0\"\nlighter_background = \"#ffffff\"\n\
             foreground = \"#202020\"\n",
        );
        let chrome = palette.surface_chrome();
        assert_ne!(chrome, hex("#ffffff"));
        assert_ne!(chrome, palette.surface());
        assert!(luminance(chrome) < luminance(palette.surface()));
    }

    #[test]
    fn a_palette_without_the_optional_keys_still_yields_usable_roles() {
        for fixture in [
            "background = \"#282828\"\nforeground = \"#d4be98\"\naccent = \"#7daea3\"\n",
            "mode = \"light\"\nbackground = \"#FFFCF0\"\nforeground = \"#100F0F\"\n\
             accent = \"#205EA6\"\n",
        ] {
            let palette = parse(fixture);
            assert_contract_without_text_on_chrome(&palette);
            let surface = palette.surface();
            let text = palette.text();
            // Every derived tone is distinct and sits between the surface and the text.
            let tones = [
                palette.surface_chrome(),
                palette.surface_hover(),
                palette.surface_active(),
                palette.border(),
                palette.text_faint(),
                palette.text_muted(),
            ];
            for tone in tones {
                assert_ne!(tone, surface, "{} equals the surface", tone.to_hex());
                assert_ne!(tone, text, "{} equals the text", tone.to_hex());
                let (low, high) = if luminance(surface) < luminance(text) {
                    (luminance(surface), luminance(text))
                } else {
                    (luminance(text), luminance(surface))
                };
                assert!((low..=high).contains(&luminance(tone)), "{}", tone.to_hex());
            }
            assert_ne!(palette.surface_hover(), palette.surface_chrome());
            assert_ne!(palette.surface_active(), palette.surface_hover());
        }
    }

    #[test]
    fn a_muted_tone_under_4_5_to_1_moves_toward_the_text() {
        let palette = parse(MIASMA);
        let given = hex("#8a8a7e");
        assert!(
            contrast(given, palette.surface_chrome()) < 4.5,
            "the fixture is too low"
        );

        let muted = palette.text_muted();
        assert_ne!(muted, given);
        assert_ne!(
            muted,
            palette.text(),
            "only as far as 4.5:1, not all the way"
        );
        assert!(contrast(muted, palette.surface()) >= 4.5);
        assert!(contrast(muted, palette.surface_chrome()) >= 4.5);
        // Moved toward the text, and the text itself is untouched.
        assert!(contrast(muted, palette.text()) < contrast(given, palette.text()));
        assert_eq!(palette.text(), hex("#c2c2b0"));
    }

    #[test]
    fn contrast_matches_the_plans_measurements() {
        // §4.1: Sealwax's preview is 3.52:1, `#bdae93` in the same slot is 5.32:1.
        let chrome = hex("#3c3836");
        assert!((contrast(hex("#9a8c72"), chrome) - 3.52).abs() < 0.01);
        assert!((contrast(hex("#bdae93"), chrome) - 5.32).abs() < 0.01);
        assert!((contrast(Rgb::new(0, 0, 0), Rgb::new(255, 255, 255)) - 21.0).abs() < 1e-9);
    }
}
