//! Design tokens: the spacing, corner, type, and color scales every screen
//! draws from, so that panels, badges, and controls stay consistent.
//!
//! Colors are raw values per theme. [`super::theme`] turns them into egui
//! visuals and the semantic [`super::theme::Palette`]; widgets pick a
//! [`Tone`] instead of a color so that meaning, not a hue, is chosen at the
//! call site.

#![expect(
    clippy::disallowed_methods,
    reason = "design tokens define raw theme colors"
)]

use eframe::egui::{self, Color32, Theme};

/// Spacing scale in points. Layouts use these four steps only.
pub(crate) struct Space;

impl Space {
    /// Gap inside compact items such as badges and between a row's parts.
    pub(crate) const XS: f32 = 4.0;
    pub(crate) const XXS: f32 = 2.0;
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
    pub(crate) const PANE: u8 = 0;
    pub(crate) const GRAPH_CANVAS: u8 = 4;
    pub(crate) const GRAPH_LABEL: u8 = 2;
    pub(crate) const STATUS_BAR: u8 = 2;
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
    #[cfg(test)]
    pub(crate) const CODE: f32 = 12.0;
    #[cfg(test)]
    pub(crate) const LARGE_CODE: f32 = 13.0;
    pub(crate) const GRAPH_COUNT: f32 = 10.5;
}

/// Bounds a dimension after reserving space for the surrounding UI.
pub(crate) struct SizeLimit {
    margin: f32,
    min: f32,
    max: f32,
}

impl SizeLimit {
    const fn new(margin: f32, min: f32, max: f32) -> Self {
        Self { margin, min, max }
    }

    pub(crate) fn fit(&self, available: f32) -> f32 {
        (available - self.margin).clamp(self.min, self.max)
    }
}

/// Fixed and minimum widths of the request list columns in points.
pub(crate) struct RequestColumns {
    pub(crate) method: f32,
    pub(crate) path_min: f32,
    pub(crate) path_compact: SizeLimit,
    pub(crate) status_wide: f32,
    pub(crate) status_compact: f32,
    pub(crate) kind: f32,
    pub(crate) size: f32,
    pub(crate) duration: f32,
    pub(crate) method_min: f32,
    pub(crate) path_resize_min: f32,
    pub(crate) status_min: f32,
    pub(crate) kind_min: f32,
    pub(crate) size_min: f32,
    pub(crate) duration_min: f32,
}

/// Dimensions of individual components in points.
pub(crate) struct Layout;

impl Layout {
    pub(crate) const WINDOW: [f32; 2] = [1_280.0, 820.0];
    pub(crate) const MIN_WINDOW: [f32; 2] = [640.0, 420.0];
    pub(crate) const MIN_MAIN_HEIGHT: f32 = 280.0;
    pub(crate) const MIN_TABLE_WIDTH: f32 = 320.0;
    pub(crate) const MIN_DETAIL_WIDTH: f32 = 280.0;
    pub(crate) const DEFAULT_DETAIL_SHARE: f32 = 0.4;
    pub(crate) const SPLIT_HANDLE_HALF_WIDTH: f32 = 3.0;
    pub(crate) const SPLIT_GUTTER_HALF_WIDTH: f32 = 1.0;
    pub(crate) const TREE_INDENT: f32 = Space::MD;
    pub(crate) const TREE_LEAF_PREFIX: f32 = 20.0;
    pub(crate) const MAP_TREE_ROW_HEIGHT: f32 = 22.0;
    pub(crate) const MAP_TREE_ROW_MARGIN: egui::Margin = egui::Margin::symmetric(4, 1);
    pub(crate) const MAP_STATUS_WIDTH: f32 = 44.0;
    pub(crate) const MAP_STATUS_HEIGHT: f32 = Space::SM;
    pub(crate) const MAP_STATUS_MIN_SEGMENT: f32 = Space::XXS;
    pub(crate) const GRAPH_CANVAS_MIN: f32 = 120.0;
    pub(crate) const GRAPH_CANVAS_PADDING: f32 = 56.0;
    pub(crate) const GRAPH_AXIS_MIN: f32 = 20.0;
    pub(crate) const GRAPH_MIN_NODE_RADIUS: f32 = 12.0;
    pub(crate) const GRAPH_MAX_NODE_RADIUS: f32 = 28.0;
    pub(crate) const GRAPH_ARROW_LENGTH: f32 = 9.0;
    pub(crate) const GRAPH_ARROW_HALF_WIDTH: f32 = 4.5;
    pub(crate) const GRAPH_LOOP_OFFSET: f32 = Space::MD;
    pub(crate) const GRAPH_LOOP_RADIUS: f32 = 10.0;
    pub(crate) const GRAPH_EDGE_CLEARANCE: f32 = 6.0;
    pub(crate) const GRAPH_EDGE_SEPARATION: f32 = 9.0;
    pub(crate) const GRAPH_ARROW_TIP_PADDING: f32 = 3.0;
    pub(crate) const GRAPH_DASH_LENGTH: f32 = 6.0;
    pub(crate) const GRAPH_LABEL_PADDING: egui::Vec2 = egui::vec2(3.0, 1.0);
    pub(crate) const BODY_MIN_WRAP_TEXT: f32 = 80.0;
    pub(crate) const BODY_LINE_VIEW_HEIGHT: f32 = 480.0;
    pub(crate) const PORT_INPUT_WIDTH: f32 = 56.0;
    pub(crate) const TABLE_ROW_HEIGHT: f32 = 24.0;
    pub(crate) const TABLE_HEADER_HEIGHT: f32 = 24.0;
    pub(crate) const TABLE_ALERT_WIDTH: f32 = 3.0;
    pub(crate) const DETAIL_TREE_HEIGHT: f32 = 350.0;
    pub(crate) const IMAGE_DISPLAY_HEIGHT: f32 = 480.0;
    pub(crate) const IMAGE_CHECKER_TILE: f32 = Space::MD;
    pub(crate) const BULK_PROGRESS_WIDTH: f32 = 120.0;
    pub(crate) const REQUEST_COLUMNS: RequestColumns = RequestColumns {
        method: 78.0,
        path_min: 140.0,
        path_compact: SizeLimit::new(0.0, 84.0, 220.0),
        status_wide: 124.0,
        status_compact: 104.0,
        kind: 48.0,
        size: 68.0,
        duration: 68.0,
        method_min: 40.0,
        path_resize_min: 60.0,
        status_min: 64.0,
        kind_min: 32.0,
        size_min: 40.0,
        duration_min: 52.0,
    };
    pub(crate) const REPLAY_WIDTH: SizeLimit = SizeLimit::new(24.0, 280.0, f32::INFINITY);
    pub(crate) const REPLAY_HEIGHT: SizeLimit = SizeLimit::new(48.0, 220.0, f32::INFINITY);
    pub(crate) const REPLAY_URL_WIDTH: SizeLimit = SizeLimit::new(0.0, 120.0, f32::INFINITY);
    pub(crate) const REPLAY_FILE_WIDTH: SizeLimit = SizeLimit::new(180.0, 140.0, 320.0);
    pub(crate) const REPLAY_PAIR_LABEL_WIDTH: SizeLimit = SizeLimit::new(0.0, 72.0, 200.0);
    pub(crate) const REPLAY_PAIR_CONTROL_WIDTH: f32 = 64.0;
    pub(crate) const REPLAY_PAIR_MIN_WIDTH: f32 = 140.0;
    pub(crate) const REPLAY_PAIR_MIN_VALUE: f32 = 72.0;
    pub(crate) const HAR_WIDTH: SizeLimit = SizeLimit::new(32.0, 240.0, 460.0);
    pub(crate) const HAR_HEIGHT: SizeLimit = SizeLimit::new(48.0, 200.0, f32::INFINITY);
    pub(crate) const HAR_PATH_WIDTH: SizeLimit = SizeLimit::new(0.0, 180.0, 420.0);
    pub(crate) const SAVE_DIRECTORY_WIDTH: SizeLimit = SizeLimit::new(48.0, 160.0, 320.0);
    pub(crate) const BODY_EXPORT_WIDTH: SizeLimit = SizeLimit::new(48.0, 240.0, 460.0);
    pub(crate) const BODY_EXPORT_HEIGHT: SizeLimit = SizeLimit::new(140.0, 120.0, f32::INFINITY);
    pub(crate) const CA_DETAILS_WIDTH: SizeLimit = SizeLimit::new(32.0, 240.0, 560.0);
    pub(crate) const SCOPE_HELP_WIDTH: SizeLimit = SizeLimit::new(32.0, 240.0, 520.0);
    pub(crate) const TOOLBAR_POPOVER_HEIGHT: SizeLimit = SizeLimit::new(80.0, 160.0, f32::INFINITY);
    pub(crate) const BODY_CONFIRM_WIDTH: SizeLimit = SizeLimit::new(48.0, 240.0, 420.0);
    pub(crate) const BODY_SETTINGS_WIDTH: SizeLimit = SizeLimit::new(48.0, 220.0, 380.0);
    pub(crate) const MAP_SEARCH_WIDTH: SizeLimit = SizeLimit::new(0.0, 90.0, 150.0);
    pub(crate) const TREE_SEARCH_WIDTH: SizeLimit = SizeLimit::new(0.0, 100.0, 180.0);
    pub(crate) const REQUEST_SEARCH_WIDTH: SizeLimit = SizeLimit::new(0.0, 120.0, 260.0);
    pub(crate) const DETAIL_KEY_WIDTH: SizeLimit = SizeLimit::new(0.0, 72.0, 220.0);
    pub(crate) const DETAIL_ROW_MIN_WIDTH: f32 = 120.0;
    pub(crate) const REPLAY_DIALOG: egui::Vec2 = egui::vec2(650.0, 600.0);
}

pub(crate) struct StrokeWidth;

impl StrokeWidth {
    pub(crate) const CONTROL: f32 = 1.0;
    pub(crate) const MAP_STATUS: f32 = 0.5;
    pub(crate) const GRAPH_LINK: f32 = 1.6;
    pub(crate) const GRAPH_REDIRECT: f32 = 2.6;
    pub(crate) const GRAPH_NODE_PENDING: f32 = 2.5;
    pub(crate) const GRAPH_NODE: f32 = 2.0;
    pub(crate) const GRAPH_NODE_HOVERED: f32 = 3.0;
}

/// Syntax colors that keep at least 4.5:1 contrast on the panel, text field,
/// stripe, hover, and selection backgrounds of their theme. Color only adds
/// to cues already in the text: keys end with `:` or `=`, strings and
/// attribute values keep their quotes, tags keep their angle brackets,
/// namespace prefixes end with `:`, entities keep `&` and `;`, `CDATA` and
/// instructions keep their `<![CDATA[` and `<?` delimiters, and comments are
/// also italic.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxPalette {
    pub(crate) key: Color32,
    pub(crate) string: Color32,
    pub(crate) number: Color32,
    pub(crate) literal: Color32,
    pub(crate) punctuation: Color32,
    pub(crate) tag: Color32,
    pub(crate) comment: Color32,
}

/// Background of every match, and of the match the user moved to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MatchColors {
    pub(crate) other: Color32,
    pub(crate) current: Color32,
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
    pub(crate) syntax: SyntaxPalette,
    pub(crate) search: MatchColors,
    /// Primary text.
    pub(crate) text: Color32,
    /// Secondary text such as header names and hosts.
    pub(crate) muted: Color32,
    /// The single accent, kept for selection, focus, and primary actions.
    pub(crate) accent: Color32,
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
    syntax: SyntaxPalette {
        key: Color32::from_rgb(120, 30, 125),
        string: Color32::from_rgb(0, 95, 35),
        number: Color32::from_rgb(0, 70, 160),
        literal: Color32::from_rgb(150, 45, 0),
        punctuation: Color32::from_gray(75),
        tag: Color32::from_rgb(140, 20, 55),
        comment: Color32::from_gray(80),
    },
    search: MatchColors {
        other: Color32::from_rgb(255, 236, 150),
        current: Color32::from_rgb(255, 196, 110),
    },
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
    syntax: SyntaxPalette {
        key: Color32::from_rgb(230, 175, 255),
        string: Color32::from_rgb(150, 230, 160),
        number: Color32::from_rgb(150, 205, 255),
        literal: Color32::from_rgb(255, 195, 135),
        punctuation: Color32::from_gray(190),
        tag: Color32::from_rgb(255, 165, 185),
        comment: Color32::from_gray(175),
    },
    search: MatchColors {
        other: Color32::from_rgb(92, 72, 0),
        current: Color32::from_rgb(150, 82, 0),
    },
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "opacity is clamped to [0, 1] before scaling"
    )]
    pub(crate) fn new_map_node_fill(self, opacity: f32) -> Color32 {
        const MAX_ALPHA: f32 = 56.0;
        let alpha = (opacity.clamp(0.0, 1.0) * MAX_ALPHA).round() as u8;
        Color32::from_rgba_unmultiplied(self.info.r(), self.info.g(), self.info.b(), alpha)
    }

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
