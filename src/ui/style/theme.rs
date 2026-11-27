//! Theme selection, accessible status colors, and settings at the filesystem boundary.
use eframe::egui::{self, Color32};
pub(crate) use egui::Theme;
use http_inspector_file::{ExistingFile, publish};

use super::tokens::{
    Borders, Layout, Motion, Radius, Space, StrokeWidth, Surfaces, ThemeTokens, Tone, TypeScale,
};

pub(crate) use super::tokens::{MatchColors, SyntaxPalette};
use http_inspector_content::format::TokenKind;

impl SyntaxPalette {
    pub(crate) const fn color(&self, kind: TokenKind, text: Color32) -> Color32 {
        match kind {
            TokenKind::Text => text,
            TokenKind::Key => self.key,
            TokenKind::String | TokenKind::AttributeValue | TokenKind::CData => self.string,
            TokenKind::Number | TokenKind::Entity => self.number,
            // Attribute names differ from the tag they sit in.
            TokenKind::Literal | TokenKind::Namespace | TokenKind::AttributeName => self.literal,
            TokenKind::Punctuation => self.punctuation,
            TokenKind::Tag => self.tag,
            TokenKind::Comment | TokenKind::Instruction => self.comment,
        }
    }
}

/// Text colors of the current theme, from its design tokens. Point colors
/// (`accent.*`, `status.ok`) are not text colors and are read from
/// [`ThemeTokens`] where a dot, tick, or bar is painted.
pub(crate) struct Palette {
    /// `text.primary`: values and every normal state.
    pub(crate) text: Color32,
    /// `text.secondary`: labels, keys, hosts, and metadata.
    pub(crate) secondary: Color32,
    /// `status.warn` text, on data surfaces only.
    pub(crate) warning: Color32,
    /// `status.error` text, on data surfaces only.
    pub(crate) error: Color32,
    pub(crate) surfaces: Surfaces,
    pub(crate) borders: Borders,
}

impl Palette {
    pub(crate) fn of(ui: &egui::Ui) -> Self {
        Self::for_theme(ui.ctx().theme())
    }

    pub(crate) const fn for_theme(theme: Theme) -> Self {
        let tokens = ThemeTokens::for_theme(theme);
        Self {
            text: tokens.text.primary,
            secondary: tokens.text.secondary,
            warning: tokens.status.warn,
            error: tokens.status.error,
            surfaces: tokens.surfaces,
            borders: tokens.borders,
        }
    }

    /// Text color of a state: the default text color unless `tone` marks an
    /// exception. `status.ok` is never a text color, so success stays default.
    pub(crate) const fn status(&self, tone: Option<Tone>) -> Color32 {
        match tone {
            None | Some(Tone::Success) => self.text,
            Some(Tone::Warning) => self.warning,
            Some(Tone::Danger) => self.error,
        }
    }
}

/// Resolves the text color of the body's theme even when the context is
/// still painting with the previous frame's style.
pub(crate) fn text_in_theme(context: &egui::Context, theme: Theme) -> Color32 {
    context.style_of(theme).visuals.text_color()
}

pub(crate) fn clear_color(visuals: &egui::Visuals) -> [f32; 4] {
    visuals.panel_fill.to_normalized_gamma_f32()
}

pub(crate) fn divider_stroke(ui: &egui::Ui, response: &egui::Response) -> egui::Stroke {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let color = if response.hovered() || response.dragged() {
        tokens.accent.secondary
    } else {
        tokens.borders.structural
    };
    egui::Stroke::new(StrokeWidth::STRUCTURAL, color)
}

pub(crate) fn image_border(ui: &egui::Ui) -> egui::Stroke {
    ui.visuals().window_stroke
}

pub(crate) fn image_checker_colors(ui: &egui::Ui) -> [Color32; 2] {
    let surfaces = Palette::of(ui).surfaces;
    [surfaces.input, surfaces.hover]
}

/// egui visuals of `theme`, built from its design tokens.
fn visuals(theme: Theme) -> egui::Visuals {
    let tokens = ThemeTokens::for_theme(theme);
    let mut visuals = theme.default_visuals();
    apply_text_colors(&mut visuals, &tokens);
    apply_surfaces(&mut visuals, &tokens);
    apply_widgets(&mut visuals.widgets, &tokens);
    visuals
}

fn apply_text_colors(visuals: &mut egui::Visuals, tokens: &ThemeTokens) {
    visuals.override_text_color = Some(tokens.text.primary);
    visuals.weak_text_alpha = 1.0;
    visuals.weak_text_color = Some(tokens.text.secondary);
    visuals.warn_fg_color = tokens.status.warn;
    visuals.error_fg_color = tokens.status.error;
    // Links are told apart by their underline, not by a color.
    visuals.hyperlink_color = tokens.text.primary;
    visuals.text_cursor.stroke.color = tokens.text.primary;
    // egui paints selected text, selected buttons, and selected table rows
    // in the selection stroke color. Accent colors are not text colors, so
    // selection keeps the primary text color and the accent goes to the
    // selection bar that the request table paints.
    visuals.selection.bg_fill = tokens.surfaces.selected;
    visuals.selection.stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.text.primary);
}

fn apply_surfaces(visuals: &mut egui::Visuals, tokens: &ThemeTokens) {
    let surfaces = tokens.surfaces;
    visuals.panel_fill = surfaces.chrome;
    visuals.window_fill = surfaces.elevated;
    visuals.extreme_bg_color = surfaces.input;
    visuals.text_edit_bg_color = Some(surfaces.input);
    visuals.code_bg_color = surfaces.input;
    visuals.faint_bg_color = surfaces.pane;
    visuals.striped = false;
    visuals.window_stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.structural);
    visuals.window_corner_radius = egui::CornerRadius::same(Radius::CONTROL);
    visuals.menu_corner_radius = egui::CornerRadius::same(Radius::CONTROL);
    visuals.window_shadow = egui::Shadow::NONE;
    // Popups and menus keep a faint edge shadow, the only shadow allowed.
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 0],
        blur: 4,
        spread: 0,
        color: tokens.borders.structural,
    };
}

fn apply_widgets(widgets: &mut egui::style::Widgets, tokens: &ThemeTokens) {
    let surfaces = tokens.surfaces;
    let borders = tokens.borders;
    widgets.noninteractive.bg_fill = surfaces.pane;
    widgets.noninteractive.weak_bg_fill = surfaces.pane;
    widgets.noninteractive.bg_stroke =
        egui::Stroke::new(StrokeWidth::STRUCTURAL, borders.structural);
    // Resting controls are outline buttons. `bg_fill` colors scrollbar
    // thumbs and checkbox and radio boxes. It is opaque in every state, so
    // the thumb never vanishes under the pointer: `borders.control` at rest
    // and `text.muted`, the inactive icon color, under the pointer or while
    // pressed, which is the hover feedback of thumbs and boxes.
    widgets.inactive.weak_bg_fill = surfaces.elevated;
    widgets.inactive.bg_fill = borders.control;
    for widget in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        widget.bg_fill = tokens.text.muted;
        widget.weak_bg_fill = surfaces.hover;
    }
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.fg_stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.text.primary);
        widget.corner_radius = egui::CornerRadius::same(Radius::CONTROL);
        widget.expansion = 0.0;
    }
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.bg_stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, borders.control);
    }
}

/// Applies the spacing, type, scrollbar, and motion scales shared by both
/// themes.
#[expect(
    clippy::disallowed_methods,
    reason = "the theme installs design typography"
)]
fn apply_scales(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(Space::SM, Space::XS);
    spacing.button_padding = egui::vec2(Space::SM, Space::XXS);
    spacing.menu_margin = egui::Margin::from(Space::SM);
    spacing.window_margin = egui::Margin::from(Space::MD);
    spacing.indent = Space::LG;
    spacing.scroll = scroll_style();
    style.animation_time = Motion::STATE;
    for (text_style, size, family) in [
        (
            TextStyle::Heading,
            TypeScale::PANEL_TITLE,
            FontFamily::Proportional,
        ),
        (TextStyle::Body, TypeScale::BODY, FontFamily::Proportional),
        (
            TextStyle::Button,
            TypeScale::CONTROL,
            FontFamily::Proportional,
        ),
        (TextStyle::Monospace, TypeScale::DATA, FontFamily::Monospace),
        (
            TextStyle::Small,
            TypeScale::STATUS,
            FontFamily::Proportional,
        ),
    ] {
        style
            .text_styles
            .insert(text_style, FontId::new(size, family));
    }
}

/// Floating 6pt scrollbars: a transparent track and a thumb that is always
/// fully opaque, over a 6pt strip reserved beside the content so the thumb
/// never covers text.
fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        bar_width: Layout::SCROLLBAR_WIDTH,
        floating_width: Layout::SCROLLBAR_WIDTH,
        floating_allocated_width: Layout::SCROLLBAR_WIDTH,
        bar_inner_margin: 0.0,
        bar_outer_margin: 0.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 1.0,
        active_handle_opacity: 1.0,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::solid()
    }
}

pub(crate) struct ThemeSettings {
    preference: egui::ThemePreference,
    path: Option<std::path::PathBuf>,
    error: Option<crate::ui::i18n::SettingsSaveError>,
}

fn parse(value: &str) -> egui::ThemePreference {
    match value.trim() {
        "Light" => egui::ThemePreference::Light,
        "Dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    }
}

const fn serialize(preference: egui::ThemePreference) -> &'static str {
    match preference {
        egui::ThemePreference::System => "System",
        egui::ThemePreference::Light => "Light",
        egui::ThemePreference::Dark => "Dark",
    }
}

impl ThemeSettings {
    pub(crate) fn new(context: &egui::Context) -> Self {
        let path =
            http_inspector_core::settings::paths::settings_directory(|name| std::env::var_os(name))
                .map(|directory| directory.join("theme"));
        let preference = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map_or(egui::ThemePreference::System, |value| parse(&value));
        context.options_mut(|options| options.fallback_theme = Theme::Dark);
        context.all_styles_mut(apply_scales);
        for theme in [Theme::Light, Theme::Dark] {
            let visuals = visuals(theme);
            context.set_visuals_of(theme, visuals);
        }
        context.set_theme(preference);
        Self {
            preference,
            path,
            error: None,
        }
    }

    /// Shows the theme choice inside the settings menu.
    pub(crate) fn menu(
        &self,
        ui: &mut egui::Ui,
        lang: crate::ui::i18n::Language,
    ) -> Option<egui::ThemePreference> {
        let text = lang.shared();
        let mut preference = self.preference;
        ui.label(text.theme);
        for (value, label) in [
            (egui::ThemePreference::System, text.system),
            (egui::ThemePreference::Light, text.theme_light),
            (egui::ThemePreference::Dark, text.theme_dark),
        ] {
            ui.radio_value(&mut preference, value, label);
        }
        if preference == egui::ThemePreference::System && ui.ctx().system_theme().is_none() {
            ui.colored_label(Palette::of(ui).secondary, text.theme_system_unavailable);
        }
        (preference != self.preference).then_some(preference)
    }

    pub(crate) fn set_preference(
        &mut self,
        context: &egui::Context,
        preference: egui::ThemePreference,
    ) {
        self.preference = preference;
        context.set_theme(preference);
        context.request_repaint();
        self.error = self.path.as_deref().map_or(
            Some(crate::ui::i18n::SettingsSaveError::DirectoryUnavailable),
            |path| {
                save_theme(path, preference)
                    .err()
                    .map(crate::ui::i18n::SettingsSaveError::Write)
            },
        );
    }

    /// Whether a theme save error waits in the status bar.
    pub(crate) const fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Shows a theme save error in the status bar.
    pub(crate) fn status(&self, ui: &mut egui::Ui, lang: crate::ui::i18n::Language) {
        if let Some(error) = &self.error {
            super::indicator::dot_label(
                ui,
                Tone::Danger,
                crate::ui::i18n::fmt::theme_save_error(lang, error),
            );
        }
    }
}

fn save_theme(path: &std::path::Path, preference: egui::ThemePreference) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("invalid settings path"))?;
    std::fs::create_dir_all(parent)?;
    publish(path, ExistingFile::Replace, |file| {
        writeln!(file, "{}", serialize(preference))
    })
}

/// Asserts WCAG AA contrast (4.5:1) of `foreground` on every background a
/// text widget of `theme` can have. Use it for `text.primary` and
/// `text.secondary`.
#[cfg(test)]
pub(crate) fn assert_readable(theme: Theme, foreground: Color32) {
    assert_contrast(theme, foreground, &text_backgrounds(theme), 4.5);
}

/// Asserts WCAG AA contrast (4.5:1) of `foreground` on the data surfaces of
/// `theme`. Use it for status, category, and syntax text, which design rule D8
/// keeps off the chrome and cross-highlight surfaces.
#[cfg(test)]
pub(crate) fn assert_readable_on_data(theme: Theme, foreground: Color32) {
    assert_contrast(theme, foreground, &data_backgrounds(theme), 4.5);
}

#[cfg(test)]
fn assert_contrast(theme: Theme, foreground: Color32, backgrounds: &[Color32], minimum: f64) {
    for &background in backgrounds {
        let contrast = super::tokens::contrast_ratio(foreground, background);
        assert!(
            contrast >= minimum,
            "{theme:?}: {foreground:?} on {background:?}: {contrast}"
        );
    }
}

/// Every surface that text of `theme` is drawn on. Translucent surfaces are
/// composited over the surfaces they are drawn on.
#[cfg(test)]
fn text_backgrounds(theme: Theme) -> Vec<Color32> {
    let visuals = visuals(theme);
    let surfaces = ThemeTokens::for_theme(theme).surfaces;
    let mut backgrounds = vec![
        visuals.panel_fill,
        visuals.window_fill,
        visuals.widgets.inactive.weak_bg_fill,
        surfaces.chrome.blend(visuals.widgets.hovered.weak_bg_fill),
        surfaces.chrome.blend(visuals.selection.bg_fill),
        surfaces.pane.blend(surfaces.cross_highlight),
    ];
    backgrounds.extend(data_backgrounds(theme));
    backgrounds
}

/// The data surfaces of `theme`: panes, popups, inputs, and hovered or
/// selected rows on panes.
#[cfg(test)]
fn data_backgrounds(theme: Theme) -> Vec<Color32> {
    let visuals = visuals(theme);
    let pane = ThemeTokens::for_theme(theme).surfaces.pane;
    vec![
        pane,
        visuals.faint_bg_color,
        visuals.window_fill,
        visuals.extreme_bg_color,
        pane.blend(visuals.widgets.hovered.weak_bg_fill),
        pane.blend(visuals.selection.bg_fill),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::style::tokens::Category;

    #[test]
    fn status_text_meets_aa_on_all_widget_backgrounds() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = Palette::for_theme(theme);
            let visuals = visuals(theme);
            for foreground in [
                palette.text,
                palette.secondary,
                visuals.text_color(),
                visuals.weak_text_color(),
                visuals.hyperlink_color,
                // egui draws selected text and selected rows in this color.
                visuals.selection.stroke.color,
            ] {
                assert_readable(theme, foreground);
            }
            let tokens = ThemeTokens::for_theme(theme);
            let syntax = SyntaxPalette::for_theme(theme);
            for foreground in [
                syntax.key,
                syntax.string,
                syntax.number,
                syntax.literal,
                syntax.punctuation,
                syntax.tag,
                syntax.comment,
                palette.warning,
                palette.error,
                tokens.category(Category::BlueSlate),
                tokens.category(Category::Teal),
                tokens.category(Category::Indigo),
                tokens.category(Category::Ochre),
            ] {
                assert_readable_on_data(theme, foreground);
            }
        }
    }

    #[test]
    fn marks_meet_three_to_one_on_every_surface() {
        // Dots, ticks, waterfall bars, and focus rings are non-text contrast (WCAG 1.4.11).
        for theme in [Theme::Light, Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            for mark in [
                tokens.accent.primary,
                tokens.accent.secondary,
                tokens.status.ok,
                tokens.status.warn,
                tokens.status.error,
                tokens.text.primary,
                tokens.text.secondary,
                tokens.category(Category::Gray),
            ] {
                assert_contrast(theme, mark, &text_backgrounds(theme), 3.0);
            }
        }
    }

    #[test]
    fn workbench_shell_combinations_meet_contrast() {
        use super::super::tokens::contrast_ratio;
        for theme in [Theme::Light, Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            let surfaces = tokens.surfaces;
            // The filter magnifier is an inactive icon in `text.muted`
            // (non-text, 3:1) on the input surface.
            let icon = contrast_ratio(tokens.text.muted, surfaces.input);
            assert!(icon >= 3.0, "{theme:?}: magnifier {icon}");
            // The short error at the right end of an input is text.
            let error = contrast_ratio(tokens.status.error, surfaces.input);
            assert!(error >= 4.5, "{theme:?}: input error {error}");
            // Tab and toggle bars on chrome, and on the selected segment.
            for background in [surfaces.chrome, surfaces.chrome.blend(surfaces.selected)] {
                let bar = contrast_ratio(tokens.accent.primary, background);
                assert!(bar >= 3.0, "{theme:?}: active bar {bar}");
            }
            // Focus rings around inputs, tabs, and segments.
            for background in [surfaces.chrome, surfaces.input, surfaces.pane] {
                let ring = contrast_ratio(tokens.accent.secondary, background);
                assert!(ring >= 3.0, "{theme:?}: focus ring {ring}");
            }
            // Text on the selected and hovered segments and Ghost buttons.
            for background in [
                surfaces.chrome.blend(surfaces.selected),
                surfaces.chrome.blend(surfaces.hover),
            ] {
                for text in [tokens.text.primary, tokens.text.secondary] {
                    let ratio = contrast_ratio(text, background);
                    assert!(
                        ratio >= 4.5,
                        "{theme:?}: {text:?} on {background:?}: {ratio}"
                    );
                }
            }
        }
    }

    #[test]
    fn inspector_combinations_meet_contrast() {
        use super::super::tokens::contrast_ratio;
        for theme in [Theme::Light, Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            let pane = tokens.surfaces.pane;
            let matched = pane.blend(tokens.surfaces.cross_highlight);
            let colors = MatchColors::for_theme(theme);
            // Search matches: text on the cross-highlight background, and
            // the underline of the current match (non-text, 3:1).
            let text = contrast_ratio(colors.text, matched);
            assert!(text >= 4.5, "{theme:?}: match text {text}");
            let underline = contrast_ratio(colors.current, matched);
            assert!(underline >= 3.0, "{theme:?}: current match {underline}");
            // Line numbers, chevrons, and inactive graph edges in `text.muted`
            // on the pane and menu (`elevated`), the uses D8 allows it.
            for background in [pane, tokens.surfaces.elevated] {
                let muted = contrast_ratio(tokens.text.muted, background);
                assert!(muted >= 3.0, "{theme:?}: muted on {background:?} {muted}");
            }
            // The active path of the relationship graph.
            let active = contrast_ratio(tokens.accent.primary, pane);
            assert!(active >= 3.0, "{theme:?}: active path {active}");
            // Syntax colors are picked from the category and text tokens.
            let syntax = SyntaxPalette::for_theme(theme);
            assert_eq!(syntax.key, tokens.category(Category::BlueSlate));
            assert_eq!(syntax.string, tokens.category(Category::Indigo));
            assert_eq!(syntax.number, tokens.category(Category::Ochre));
            assert_eq!(syntax.punctuation, tokens.text.secondary);
        }
    }

    #[test]
    fn scrollbars_float_with_an_opaque_thumb_and_no_track() {
        let scroll = scroll_style();
        assert!(scroll.floating);
        assert!((scroll.bar_width - Layout::SCROLLBAR_WIDTH).abs() < f32::EPSILON);
        assert!((scroll.floating_width - Layout::SCROLLBAR_WIDTH).abs() < f32::EPSILON);
        assert!(scroll.dormant_background_opacity.abs() < f32::EPSILON);
        assert!(scroll.interact_background_opacity.abs() < f32::EPSILON);
        assert!((scroll.dormant_handle_opacity - 1.0).abs() < f32::EPSILON);
        assert!((scroll.interact_handle_opacity - 1.0).abs() < f32::EPSILON);
        // The thumb and checkbox boxes are opaque in every state and change
        // under the pointer.
        for theme in [Theme::Light, Theme::Dark] {
            let widgets = visuals(theme).widgets;
            let tokens = ThemeTokens::for_theme(theme);
            assert_eq!(widgets.inactive.bg_fill, tokens.borders.control);
            for state in [widgets.hovered, widgets.active] {
                assert_eq!(state.bg_fill, tokens.text.muted, "{theme:?}");
                assert_eq!(state.bg_fill.a(), u8::MAX);
            }
            // The thumb under the pointer, and the check mark on a hovered
            // box, stay visible (non-text 3:1).
            let pane = tokens.surfaces.pane;
            let thumb = super::super::tokens::contrast_ratio(tokens.text.muted, pane);
            assert!(thumb >= 3.0, "{theme:?}: hovered thumb {thumb}");
            let mark = super::super::tokens::contrast_ratio(
                widgets.hovered.fg_stroke.color,
                tokens.text.muted,
            );
            assert!(mark >= 3.0, "{theme:?}: check mark on hover {mark}");
        }
    }

    #[test]
    fn success_is_never_a_text_color() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = Palette::for_theme(theme);
            assert_eq!(palette.status(Some(Tone::Success)), palette.text);
            assert_eq!(palette.status(None), palette.text);
            assert_eq!(palette.status(Some(Tone::Danger)), palette.error);
        }
    }

    #[test]
    fn invalid_settings_use_system() {
        assert_eq!(parse("invalid"), egui::ThemePreference::System);
        assert_eq!(parse(""), egui::ThemePreference::System);
        assert_eq!(parse("Light\n"), egui::ThemePreference::Light);
    }

    #[test]
    fn selection_survives_restart() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("theme");
        for (preference, expected) in [
            (egui::ThemePreference::System, "System\n"),
            (egui::ThemePreference::Light, "Light\n"),
            (egui::ThemePreference::Dark, "Dark\n"),
        ] {
            save_theme(&path, preference)?;
            let saved = std::fs::read_to_string(&path)?;
            assert_eq!(saved, expected);
            assert_eq!(parse(&saved), preference);
        }
        Ok(())
    }
}
