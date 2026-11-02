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

    /// An outgoing message's bubble: `surface` 18 % toward `accent`, the faint accent tint that
    /// tells our messages from theirs without a second accent-coloured surface.
    pub fn bubble_outgoing(&self) -> Rgb {
        mix(self.surface(), self.accent(), 0.18)
    }

    /// The ear of an incoming message's slab: the theme's cyan, the one hue that says "them".
    /// When cyan sits too close to the chrome to read as a graphic (under 1.5:1), the accent is
    /// used instead, so the ear is never invisible.
    pub fn bubble_ear_incoming(&self) -> Rgb {
        const MIN_CONTRAST: f64 = 1.5;
        if contrast(self.cyan, self.surface_chrome()) >= MIN_CONTRAST {
            self.cyan
        } else {
            self.accent()
        }
    }

    /// The ear of an outgoing message's slab: the accent, the same mark the focus uses.
    pub fn bubble_ear_outgoing(&self) -> Rgb {
        self.accent()
    }

    /// A quote's inset inside a bubble of `tint`: 30 % of the way toward the surface behind it, so
    /// it reads as a recess on a dark theme and as a lighter panel on a light one.
    pub fn quote_inset(&self, tint: Rgb) -> Rgb {
        mix(tint, self.surface(), 0.30)
    }

    /// An avatar's tint for `seed`: 35 % of one of four palette hues into the chrome, so every
    /// chat gets a stable colour and no tone is invented.
    pub fn avatar_tint(&self, seed: u8) -> Rgb {
        let hue = match seed % 4 {
            0 => self.accent,
            1 => self.cyan,
            2 => self.green,
            _ => self.yellow,
        };
        mix(self.surface_chrome(), hue, 0.35)
    }

    /// The letter on [`Palette::avatar_tint`]: whichever of `text` and `surface` contrasts more
    /// with it, so an avatar stays readable on every theme.
    pub fn avatar_ink(&self, seed: u8) -> Rgb {
        let tint = self.avatar_tint(seed);
        if contrast(tint, self.text()) >= contrast(tint, self.surface()) {
            self.text()
        } else {
            self.surface()
        }
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
        // The outgoing bubble is a tint of the accent, on the surface's side of the text.
        let outgoing = palette.bubble_outgoing();
        assert_ne!(outgoing, palette.surface());
        assert_ne!(outgoing, palette.surface_chrome());
        assert!(outgoing.b > outgoing.r, "the accent is the cooler hue");
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

    /// Owner's decision (F1, 2026-10-24): `text` keeps 7:1 on the content surface and takes the
    /// WCAG AA floor, 4.5:1, on the chrome. The plan's 7:1 on `surface_chrome` does not hold on
    /// two real Omarchy themes with the plan's own mapping (`foreground` on
    /// `lighter_background`): Gruvbox 6.42:1, Catppuccin Latte 6.04:1, and `text` may never
    /// change (S4). AA is the threshold for normal text, so the chrome keeps it.
    #[test]
    fn text_is_7_to_1_on_the_content_and_aa_on_the_chrome() {
        for fixture in [GRUVBOX, CATPPUCCIN_LATTE] {
            let palette = parse(fixture);
            let content = contrast(palette.text(), palette.surface());
            assert!(content >= 7.0, "text on surface {content:.2}");
            let chrome = contrast(palette.text(), palette.surface_chrome());
            assert!(
                chrome >= 4.5,
                "text on surface_chrome {chrome:.2} is under WCAG AA"
            );
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

    /// Direction C's roles keep their recipes on both themes, and the text that sits on them
    /// stays readable.
    #[test]
    fn the_direction_c_roles_hold_on_dark_and_light_themes() {
        for fixture in [GRUVBOX, FLEXOKI_LIGHT] {
            let palette = parse(fixture);
            // The incoming ear is a graphic: it reads against the chrome it sits on.
            let ear = palette.bubble_ear_incoming();
            let ear_on_chrome = contrast(ear, palette.surface_chrome());
            assert!(
                ear_on_chrome >= 1.5,
                "the ear reads on the chrome: {ear_on_chrome:.2}"
            );
            // The outgoing ear is the accent, the same mark the focus uses.
            assert_eq!(palette.bubble_ear_outgoing(), palette.accent());
            // A quote stays readable inside a bubble of either tone.
            for tint in [palette.surface_chrome(), palette.bubble_outgoing()] {
                let inset = palette.quote_inset(tint);
                assert_ne!(inset, tint, "the inset is its own tone");
                let on_inset = contrast(palette.text(), inset);
                assert!(on_inset >= 4.5, "text on the quote inset: {on_inset:.2}");
            }
            // Four distinct, stable avatar tints, each with an ink that reads on it.
            let mut tints = Vec::new();
            for seed in 0..4 {
                let tint = palette.avatar_tint(seed);
                assert_eq!(tint, palette.avatar_tint(seed), "stable for a seed");
                assert!(!tints.contains(&tint), "distinct for the four seeds");
                let ink = contrast(palette.avatar_ink(seed), tint);
                assert!(ink >= 3.0, "the initials read on the tint: {ink:.2}");
                tints.push(tint);
            }
        }
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
