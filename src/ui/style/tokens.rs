//! Design tokens of the docked workbench design system (Slate Navy & Muted
//! Teal): the spacing, corner, type, layout, and color scales every screen
//! draws from. Values follow `docs/ui-design-system.md`.
//!
//! Colors are raw values per theme. [`super::theme`] turns them into egui
//! visuals and the text [`super::theme::Palette`]; widgets pick a [`Tone`]
//! or a [`Category`] instead of a color so that meaning, not a hue, is
//! chosen at the call site.

use eframe::egui::{Color32, Theme};

/// Spacing scale in points. Layouts use these six steps only.
pub(crate) struct Space;

impl Space {
    /// Hairline gaps such as the vertical padding of a compact button.
    pub(crate) const XXS: f32 = 2.0;
    /// Gap between the lines of a group and inside compact items.
    pub(crate) const XS: f32 = 4.0;
    /// Gap between an indicator (dot, tick) and its text.
    pub(crate) const S: f32 = 6.0;
    /// Default gap between neighboring controls and the largest padding
    /// inside data areas.
    pub(crate) const SM: f32 = 8.0;
    /// Padding inside dialogs.
    pub(crate) const MD: f32 = 12.0;
    /// Separation between groups of controls and nested indentation.
    pub(crate) const LG: f32 = 16.0;
}

/// Corner radii in points.
pub(crate) struct Radius;

impl Radius {
    /// Panels, toolbars, tables, splits, and the details panel.
    pub(crate) const PANE: u8 = 0;
    /// Buttons, inputs, drop-downs, popups, menus, and dialogs.
    pub(crate) const CONTROL: u8 = 4;
    /// Shortcut chips and scrollbar thumbs.
    #[expect(
        dead_code,
        reason = "Shortcut chips arrive with the toolbar and pane header step of the redesign"
    )]
    pub(crate) const MICRO: u8 = 2;
}

/// Text sizes in points. UI text uses the proportional (sans) font and data
/// (URLs, header values, numbers, bodies) uses the monospace font.
pub(crate) struct TypeScale;

impl TypeScale {
    /// Panel and toolbar titles.
    pub(crate) const PANEL_TITLE: f32 = 12.0;
    /// Letter spacing of panel titles: 0.01 em of [`Self::PANEL_TITLE`].
    #[expect(
        dead_code,
        reason = "Panel headers arrive with the toolbar and pane header step of the redesign"
    )]
    pub(crate) const PANEL_TITLE_TRACKING: f32 = 0.12;
    /// Controls, buttons, and tabs.
    pub(crate) const CONTROL: f32 = 12.0;
    /// Table column headers, drawn in `text.secondary`.
    pub(crate) const COLUMN_HEADER: f32 = 11.0;
    /// Body text and labels.
    pub(crate) const BODY: f32 = 12.5;
    /// Monospace data cells, numbers, and code.
    pub(crate) const DATA: f32 = 12.0;
    /// Key figures that the user compares or judges.
    #[expect(
        dead_code,
        reason = "Key figures are used only by the auxiliary strip of a later redesign step"
    )]
    pub(crate) const KPI: f32 = 16.0;
    /// Status bar and secondary metadata.
    pub(crate) const STATUS: f32 = 11.0;
}

/// Fixed sizes of the workbench layout in points.
pub(crate) struct Layout;

impl Layout {
    /// Height of a table data row.
    pub(crate) const TABLE_ROW_HEIGHT: f32 = 26.0;
    /// Height of a sticky table header.
    pub(crate) const TABLE_HEADER_HEIGHT: f32 = 28.0;
    /// Width of the scrollbar.
    pub(crate) const SCROLLBAR_WIDTH: f32 = 6.0;
    /// Diameter of a status or category dot.
    pub(crate) const DOT_SIZE: f32 = 6.0;
    /// Width of the left tick that marks a failed status.
    pub(crate) const TICK_WIDTH: f32 = 2.0;
    /// Height of the left tick that marks a failed status.
    pub(crate) const TICK_HEIGHT: f32 = 12.0;
    /// Width of the bar at the left edge of a selected row.
    pub(crate) const SELECTION_BAR_WIDTH: f32 = 2.0;
}

/// Durations of state transitions in seconds.
pub(crate) struct Motion;

impl Motion {
    /// Hover, selection, and focus transitions.
    pub(crate) const STATE: f32 = 0.08;
}

/// Surface colors from the base canvas up to popups.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Surfaces {
    /// The lowest canvas and inactive areas.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "The base canvas shows only between docked panes, which the shell step adds"
        )
    )]
    pub(crate) base: Color32,
    /// Main work panes: the request table, details, raw viewers, graphs.
    pub(crate) pane: Color32,
    /// Toolbar, panel headers, table headers, and the status bar.
    pub(crate) chrome: Color32,
    /// Drop-downs, tooltips, popups, and outline buttons.
    pub(crate) elevated: Color32,
    /// Text inputs: `base` in the dark theme and `elevated` in the light one.
    pub(crate) input: Color32,
    /// Rows, buttons, and tree items under the pointer (translucent).
    pub(crate) hover: Color32,
    /// Selected rows and toggles that are on (translucent).
    pub(crate) selected: Color32,
    /// Items related to the selection in another pane, and ranges in raw
    /// viewers (translucent).
    pub(crate) cross_highlight: Color32,
}

/// Hairline colors.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Borders {
    /// Pane borders, toolbar and status bar edges, and the line under table
    /// headers.
    pub(crate) structural: Color32,
    /// Table row lines, tree guides, and gutter edges.
    pub(crate) subtle: Color32,
    /// Inputs, selects, outline buttons, and scrollbar thumbs.
    pub(crate) control: Color32,
}

/// Text colors from the strongest to the weakest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextColors {
    /// Data values, active tabs, body text, and selected rows.
    pub(crate) primary: Color32,
    /// Column headers, labels, hosts, and status bar metadata.
    pub(crate) secondary: Color32,
    /// Below 4.5:1 on some surfaces: inactive icons, key-value colons, line
    /// number gutters, and hints of inputs that have their own label.
    #[expect(
        dead_code,
        reason = "Line number gutters and key-value colons are restyled with the details panel"
    )]
    pub(crate) muted: Color32,
    /// Text of disabled controls.
    #[expect(
        dead_code,
        reason = "Disabled control text is restyled with the toolbar controls"
    )]
    pub(crate) disabled: Color32,
}

/// Point colors. Never used for text: see `docs/ui-design-system.md`, "대비".
#[derive(Clone, Copy, Debug)]
pub(crate) struct Accents {
    /// Selection bars, bars of toggles that are on, active paths, chart lines.
    pub(crate) primary: Color32,
    /// Focus rings, cross highlights, and splitter hover.
    pub(crate) secondary: Color32,
}

/// State colors.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StatusColors {
    /// Connected or running. Dots only, never text.
    pub(crate) ok: Color32,
    /// 4xx responses, cancellations, and delays.
    pub(crate) warn: Color32,
    /// 5xx responses, failed connections, and a stopped proxy.
    pub(crate) error: Color32,
}

/// Colors of the one primary action button of a screen.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "The primary action button is styled with the toolbar step of the redesign"
    )
)]
pub(crate) struct PrimaryAction {
    pub(crate) fill: Color32,
    pub(crate) text: Color32,
    pub(crate) stroke: Color32,
}

/// Every raw color of one theme.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThemeTokens {
    pub(crate) surfaces: Surfaces,
    pub(crate) borders: Borders,
    pub(crate) text: TextColors,
    pub(crate) accent: Accents,
    pub(crate) status: StatusColors,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "The primary action button is styled with the toolbar step of the redesign"
        )
    )]
    pub(crate) primary_action: PrimaryAction,
    /// Colors of [`Category`], in declaration order.
    categories: [Color32; 5],
}

const DARK: ThemeTokens = ThemeTokens {
    surfaces: Surfaces {
        base: Color32::from_rgb(0x0B, 0x10, 0x17),
        pane: Color32::from_rgb(0x0F, 0x16, 0x20),
        chrome: Color32::from_rgb(0x14, 0x1D, 0x29),
        elevated: Color32::from_rgb(0x1A, 0x25, 0x34),
        input: Color32::from_rgb(0x0B, 0x10, 0x17),
        // rgba(148,163,184,0.08), rgba(45,212,191,0.10), rgba(56,189,248,0.18)
        hover: Color32::from_rgba_unmultiplied_const(148, 163, 184, 20),
        selected: Color32::from_rgba_unmultiplied_const(45, 212, 191, 26),
        cross_highlight: Color32::from_rgba_unmultiplied_const(56, 189, 248, 46),
    },
    borders: Borders {
        structural: Color32::from_rgb(0x1E, 0x2A, 0x3B),
        subtle: Color32::from_rgb(0x16, 0x20, 0x2E),
        control: Color32::from_rgb(0x27, 0x36, 0x4B),
    },
    text: TextColors {
        primary: Color32::from_rgb(0xE2, 0xE8, 0xF0),
        secondary: Color32::from_rgb(0x94, 0xA3, 0xB8),
        muted: Color32::from_rgb(0x64, 0x74, 0x8B),
        disabled: Color32::from_rgb(0x47, 0x55, 0x69),
    },
    accent: Accents {
        primary: Color32::from_rgb(0x2D, 0xD4, 0xBF),
        secondary: Color32::from_rgb(0x38, 0xBD, 0xF8),
    },
    status: StatusColors {
        ok: Color32::from_rgb(0x34, 0xD3, 0x99),
        warn: Color32::from_rgb(0xD9, 0xA0, 0x5B),
        error: Color32::from_rgb(0xE0, 0x6C, 0x75),
    },
    primary_action: PrimaryAction {
        fill: Color32::from_rgb(0x13, 0x4E, 0x4A),
        text: Color32::from_rgb(0x5E, 0xEA, 0xD4),
        // #2DD4BF40
        stroke: Color32::from_rgba_unmultiplied_const(0x2D, 0xD4, 0xBF, 0x40),
    },
    categories: [
        Color32::from_rgb(0x7D, 0xD3, 0xFC),
        Color32::from_rgb(0x5E, 0xEA, 0xD4),
        Color32::from_rgb(0xA5, 0xB4, 0xFC),
        Color32::from_rgb(0xD4, 0xA3, 0x73),
        Color32::from_rgb(0x94, 0xA3, 0xB8),
    ],
};

const LIGHT: ThemeTokens = ThemeTokens {
    surfaces: Surfaces {
        base: Color32::from_rgb(0xF4, 0xF6, 0xF9),
        pane: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        chrome: Color32::from_rgb(0xEE, 0xF2, 0xF6),
        elevated: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        input: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        // rgba(15,23,42,0.04), rgba(13,148,136,0.08), rgba(2,132,199,0.14)
        hover: Color32::from_rgba_unmultiplied_const(15, 23, 42, 10),
        selected: Color32::from_rgba_unmultiplied_const(13, 148, 136, 20),
        cross_highlight: Color32::from_rgba_unmultiplied_const(2, 132, 199, 36),
    },
    borders: Borders {
        structural: Color32::from_rgb(0xD5, 0xDD, 0xE7),
        subtle: Color32::from_rgb(0xE6, 0xEC, 0xF2),
        control: Color32::from_rgb(0xC4, 0xCF, 0xDC),
    },
    text: TextColors {
        primary: Color32::from_rgb(0x0F, 0x17, 0x2A),
        secondary: Color32::from_rgb(0x47, 0x55, 0x69),
        muted: Color32::from_rgb(0x84, 0x94, 0xA7),
        disabled: Color32::from_rgb(0xB0, 0xBC, 0xCB),
    },
    accent: Accents {
        primary: Color32::from_rgb(0x0D, 0x94, 0x88),
        secondary: Color32::from_rgb(0x02, 0x84, 0xC7),
    },
    status: StatusColors {
        ok: Color32::from_rgb(0x05, 0x96, 0x69),
        warn: Color32::from_rgb(0xB4, 0x53, 0x09),
        error: Color32::from_rgb(0xBE, 0x12, 0x3C),
    },
    primary_action: PrimaryAction {
        fill: Color32::from_rgb(0x0F, 0x76, 0x6E),
        text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        stroke: Color32::TRANSPARENT,
    },
    categories: [
        Color32::from_rgb(0x03, 0x69, 0xA1),
        Color32::from_rgb(0x0F, 0x76, 0x6E),
        Color32::from_rgb(0x43, 0x38, 0xCA),
        Color32::from_rgb(0x9A, 0x63, 0x24),
        Color32::from_rgb(0x64, 0x74, 0x8B),
    ],
};

impl ThemeTokens {
    pub(crate) const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => LIGHT,
            Theme::Dark => DARK,
        }
    }

    /// Color of `category`, for text or a 6px dot only.
    pub(crate) const fn category(&self, category: Category) -> Color32 {
        let [blue_slate, teal, indigo, ochre, gray] = self.categories;
        match category {
            Category::BlueSlate => blue_slate,
            Category::Teal => teal,
            Category::Indigo => indigo,
            Category::Ochre => ochre,
            Category::Gray => gray,
        }
    }
}

/// A data category without a good or bad meaning, such as an edge kind of
/// the relationship graph. `Gray` is for dots only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Category {
    BlueSlate,
    Teal,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "No screen shows a fourth category until the details panel step"
        )
    )]
    Indigo,
    Ochre,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Cool gray is reserved for dots, which arrive with the shell step"
        )
    )]
    Gray,
}

/// Meaning of an exception or state change. Normal states have no tone and
/// use the default text color. Text always states the same meaning, so the
/// tone only reinforces it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    /// Connected or running: dots only.
    Success,
    /// Needs attention: 4xx, cancelled, incomplete.
    Warning,
    /// Failure: 5xx, failed connections and tunnels, a stopped proxy.
    Danger,
}

impl Tone {
    pub(crate) const fn color(self, theme: Theme) -> Color32 {
        let status = ThemeTokens::for_theme(theme).status;
        match self {
            Self::Success => status.ok,
            Self::Warning => status.warn,
            Self::Danger => status.error,
        }
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
    fn surfaces_step_up_from_base_to_elevated_in_the_dark_theme() {
        // Dark surfaces get lighter as they rise, so each tier is visibly
        // distinct from the one below.
        let dark = ThemeTokens::for_theme(Theme::Dark).surfaces;
        let tiers = [dark.base, dark.pane, dark.chrome, dark.elevated];
        assert!(tiers.windows(2).all(|pair| match pair {
            [lower, upper] => lower.g() < upper.g(),
            _ => false,
        }));
    }

    #[test]
    fn translucent_surfaces_keep_the_specified_opacity() {
        // round(opacity × 255), from `docs/ui-design-system.md`.
        for (theme, expected) in [(Theme::Dark, [20, 26, 46]), (Theme::Light, [10, 20, 36])] {
            let surfaces = ThemeTokens::for_theme(theme).surfaces;
            let alphas = [
                surfaces.hover.a(),
                surfaces.selected.a(),
                surfaces.cross_highlight.a(),
            ];
            assert_eq!(alphas, expected, "{theme:?}");
        }
    }

    #[test]
    fn primary_action_text_meets_aa_on_its_fill() {
        for theme in [Theme::Light, Theme::Dark] {
            let action = ThemeTokens::for_theme(theme).primary_action;
            let contrast = contrast_ratio(action.text, action.fill);
            assert!(contrast >= 4.5, "{theme:?}: {contrast}");
        }
        // The light button has no outline; the dark one has a faint teal one.
        let dark = ThemeTokens::for_theme(Theme::Dark).primary_action.stroke;
        let light = ThemeTokens::for_theme(Theme::Light).primary_action.stroke;
        assert_eq!((dark.a(), light.a()), (0x40, 0));
    }
}
