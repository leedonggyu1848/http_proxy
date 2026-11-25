//! Design tokens: the spacing, corner, type, and color scales every screen
//! draws from, so that panels, badges, and controls stay consistent.
//!
//! Colors are raw values per theme. [`super::theme`] turns them into egui
//! visuals and the semantic [`super::theme::Palette`]; widgets pick a
//! [`Tone`] instead of a color so that meaning, not a hue, is chosen at the
//! call site.

use eframe::egui::{Color32, Theme};

/// Spacing scale in points. Layouts use these four steps only.
pub(crate) struct Space;

impl Space {
    /// Gap inside compact items such as badges and between a row's parts.
    pub(crate) const XS: f32 = 4.0;
    /// Default gap between neighboring controls.
    pub(crate) const SM: f32 = 8.0;
    /// Padding inside windows and cards.
    pub(crate) const MD: f32 = 12.0;
    /// Separation between groups of controls and nested indentation.
    pub(crate) const LG: f32 = 16.0;
}

/// Corner radii in points.
pub(crate) struct Radius;

impl Radius {
    /// Buttons, text fields, and other controls.
    pub(crate) const CONTROL: u8 = 6;
    /// Method badges and other small tags.
    pub(crate) const BADGE: u8 = 4;
    /// Cards, menus, and windows.
    pub(crate) const CARD: u8 = 8;
}

/// Text sizes in points. UI text uses the proportional font and data (URLs,
/// header values, bodies) uses the monospace font.
pub(crate) struct TypeScale;

impl TypeScale {
    /// Section titles.
    pub(crate) const HEADING: f32 = 16.0;
    /// Labels, buttons, and other UI text.
    pub(crate) const BODY: f32 = 13.0;
    /// Monospace data such as URLs and header values.
    pub(crate) const DATA: f32 = 12.5;
    /// Secondary text and badges.
    pub(crate) const SMALL: f32 = 11.5;
}

/// Surface colors from the window background up to raised cards and popups.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Surfaces {
    /// Window chrome: toolbar, filters, and the status bar.
    pub(crate) bg: Color32,
    /// Content panes such as the request list and details.
    pub(crate) panel: Color32,
    /// Buttons, cards, menus, and windows above the panes.
    pub(crate) raised: Color32,
    /// Text fields and other inset areas.
    pub(crate) inset: Color32,
    /// Alternate list rows.
    pub(crate) stripe: Color32,
    /// Rows and controls under the pointer.
    pub(crate) hover: Color32,
    /// Selected rows and text selections.
    pub(crate) selection: Color32,
    /// Separators and control borders.
    pub(crate) border: Color32,
}

/// Every raw color of one theme.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThemeTokens {
    pub(crate) surfaces: Surfaces,
    /// Primary text.
    pub(crate) text: Color32,
    /// Secondary text such as header names and hosts.
    pub(crate) muted: Color32,
    /// The single accent, kept for selection, focus, and primary actions.
    pub(crate) accent: Color32,
    /// Text drawn on an `accent` fill, such as a primary button label.
    pub(crate) on_accent: Color32,
    pub(crate) success: Color32,
    pub(crate) error: Color32,
    pub(crate) warning: Color32,
    pub(crate) info: Color32,
    /// Tinted fills behind tone-colored badge text, by [`Tone`].
    fills: ToneFills,
}

#[derive(Clone, Copy, Debug)]
struct ToneFills {
    neutral: Color32,
    info: Color32,
    success: Color32,
    warning: Color32,
    danger: Color32,
}

const LIGHT: ThemeTokens = ThemeTokens {
    surfaces: Surfaces {
        bg: Color32::from_rgb(238, 240, 243),
        panel: Color32::from_rgb(250, 250, 251),
        raised: Color32::from_rgb(255, 255, 255),
        inset: Color32::from_rgb(255, 255, 255),
        stripe: Color32::from_rgb(243, 244, 247),
        hover: Color32::from_rgb(230, 234, 240),
        selection: Color32::from_rgb(216, 230, 250),
        border: Color32::from_rgb(206, 211, 219),
    },
    text: Color32::from_rgb(30, 33, 38),
    muted: Color32::from_rgb(84, 90, 100),
    accent: Color32::from_rgb(20, 85, 190),
    on_accent: Color32::from_rgb(255, 255, 255),
    success: Color32::from_rgb(0, 90, 40),
    error: Color32::from_rgb(160, 15, 25),
    warning: Color32::from_rgb(110, 65, 0),
    info: Color32::from_rgb(0, 65, 150),
    fills: ToneFills {
        neutral: Color32::from_rgb(232, 234, 238),
        info: Color32::from_rgb(222, 234, 252),
        success: Color32::from_rgb(220, 242, 226),
        warning: Color32::from_rgb(252, 236, 208),
        danger: Color32::from_rgb(252, 226, 226),
    },
};

const DARK: ThemeTokens = ThemeTokens {
    surfaces: Surfaces {
        bg: Color32::from_rgb(22, 24, 28),
        panel: Color32::from_rgb(28, 31, 36),
        raised: Color32::from_rgb(40, 44, 51),
        inset: Color32::from_rgb(16, 18, 21),
        stripe: Color32::from_rgb(33, 36, 42),
        hover: Color32::from_rgb(45, 50, 58),
        selection: Color32::from_rgb(30, 54, 90),
        border: Color32::from_rgb(55, 60, 69),
    },
    text: Color32::from_rgb(226, 229, 234),
    muted: Color32::from_rgb(168, 174, 184),
    accent: Color32::from_rgb(125, 178, 255),
    on_accent: Color32::from_rgb(16, 18, 21),
    success: Color32::from_rgb(140, 240, 170),
    error: Color32::from_rgb(255, 170, 170),
    warning: Color32::from_rgb(255, 220, 130),
    info: Color32::from_rgb(160, 210, 255),
    fills: ToneFills {
        neutral: Color32::from_rgb(52, 56, 64),
        info: Color32::from_rgb(24, 48, 80),
        success: Color32::from_rgb(20, 56, 36),
        warning: Color32::from_rgb(66, 48, 12),
        danger: Color32::from_rgb(80, 28, 34),
    },
};

impl ThemeTokens {
    pub(crate) const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => LIGHT,
            Theme::Dark => DARK,
        }
    }
}

/// Meaning of a colored element. Text always states the same meaning, so
/// the tone only reinforces it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    /// No particular state: pending, tunnels, uncommon methods.
    Neutral,
    /// Informational: redirects, `GET`.
    Info,
    /// Success: 2xx, `POST`.
    Success,
    /// Needs attention: 4xx, cancelled, `PUT` and `PATCH`.
    Warning,
    /// Failure: 5xx, failed connections, `DELETE`.
    Danger,
}

/// Foreground and fill of a [`Tone`] in one theme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ToneColors {
    /// Text and marks; also readable on every surface.
    pub(crate) fg: Color32,
    /// Tinted background behind `fg` in badges.
    pub(crate) fill: Color32,
}

impl Tone {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::Neutral,
        Self::Info,
        Self::Success,
        Self::Warning,
        Self::Danger,
    ];

    pub(crate) const fn colors(self, theme: Theme) -> ToneColors {
        let tokens = ThemeTokens::for_theme(theme);
        let (fg, fill) = match self {
            Self::Neutral => (tokens.muted, tokens.fills.neutral),
            Self::Info => (tokens.info, tokens.fills.info),
            Self::Success => (tokens.success, tokens.fills.success),
            Self::Warning => (tokens.warning, tokens.fills.warning),
            Self::Danger => (tokens.error, tokens.fills.danger),
        };
        ToneColors { fg, fill }
    }
}

/// WCAG 2 contrast ratio between two opaque colors.
#[cfg(test)]
pub(crate) fn contrast_ratio(foreground: Color32, background: Color32) -> f64 {
    fn luminance(color: Color32) -> f64 {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.0722_f64.mul_add(
            linear(color.b()),
            0.7152_f64.mul_add(linear(color.g()), 0.2126 * linear(color.r())),
        )
    }
    let a = luminance(foreground);
    let b = luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_text_meets_aa_on_its_fill() {
        for theme in [Theme::Light, Theme::Dark] {
            for tone in Tone::ALL {
                let colors = tone.colors(theme);
                let contrast = contrast_ratio(colors.fg, colors.fill);
                assert!(contrast >= 4.5, "{theme:?} {tone:?}: {contrast}");
            }
        }
    }

    #[test]
    fn primary_action_text_meets_aa_on_the_accent() {
        for theme in [Theme::Light, Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            let contrast = contrast_ratio(tokens.on_accent, tokens.accent);
            assert!(contrast >= 4.5, "{theme:?}: {contrast}");
        }
    }

    #[test]
    fn surfaces_step_up_from_window_to_raised() {
        // Light surfaces get brighter and dark surfaces lighter as they rise,
        // so each tier is visibly distinct from the one below.
        let light = ThemeTokens::for_theme(Theme::Light).surfaces;
        assert!(light.bg.r() < light.panel.r() && light.panel.r() < light.raised.r());
        let dark = ThemeTokens::for_theme(Theme::Dark).surfaces;
        assert!(dark.bg.r() < dark.panel.r() && dark.panel.r() < dark.raised.r());
    }

    #[test]
    fn tone_fills_differ_from_surfaces_and_each_other() {
        for theme in [Theme::Light, Theme::Dark] {
            let surfaces = ThemeTokens::for_theme(theme).surfaces;
            let fills = Tone::ALL.map(|tone| tone.colors(theme).fill);
            for (index, fill) in fills.iter().enumerate() {
                assert_ne!(*fill, surfaces.panel);
                assert_ne!(*fill, surfaces.stripe);
                assert!(!fills.iter().take(index).any(|other| other == fill));
            }
        }
    }
}
