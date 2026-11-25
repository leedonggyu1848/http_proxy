//! Theme selection, accessible status colors, and settings at the filesystem boundary.
use eframe::egui::{self, Color32};
pub(crate) use egui::Theme;
use http_inspector_file::{ExistingFile, publish};

use super::tokens::{Layout, Motion, Radius, Space, ThemeTokens, Tone, TypeScale};

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
    visuals.selection.stroke = egui::Stroke::new(1.0, tokens.text.primary);
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
    visuals.window_stroke = egui::Stroke::new(1.0, tokens.borders.structural);
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
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, borders.structural);
    // Resting controls are outline buttons; `bg_fill` also colors scrollbar
    // thumbs and checkbox boxes.
    widgets.inactive.weak_bg_fill = surfaces.elevated;
    widgets.inactive.bg_fill = borders.control;
    for widget in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        widget.bg_fill = surfaces.hover;
        widget.weak_bg_fill = surfaces.hover;
    }
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.fg_stroke = egui::Stroke::new(1.0, tokens.text.primary);
        widget.corner_radius = egui::CornerRadius::same(Radius::CONTROL);
        widget.expansion = 0.0;
    }
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.bg_stroke = egui::Stroke::new(1.0, borders.control);
    }
}

/// Applies the spacing, type, scrollbar, and motion scales shared by both
/// themes.
fn apply_scales(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(Space::SM, Space::XS);
    spacing.button_padding = egui::vec2(Space::SM, Space::XXS);
    spacing.menu_margin = egui::Margin::from(Space::SM);
    spacing.window_margin = egui::Margin::from(Space::MD);
    spacing.indent = Space::LG;
    spacing.scroll = egui::style::ScrollStyle {
        bar_width: Layout::SCROLLBAR_WIDTH,
        ..egui::style::ScrollStyle::solid()
    };
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

pub(crate) struct ThemeSettings {
    preference: egui::ThemePreference,
    path: Option<std::path::PathBuf>,
    error: Option<String>,
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
    pub(crate) fn menu(&mut self, ui: &mut egui::Ui) {
        let previous = self.preference;
        ui.label("Theme");
        for (value, label) in [
            (egui::ThemePreference::System, "System"),
            (egui::ThemePreference::Light, "Light"),
            (egui::ThemePreference::Dark, "Dark"),
        ] {
            ui.radio_value(&mut self.preference, value, label);
        }
        if previous != self.preference {
            ui.ctx().set_theme(self.preference);
            ui.ctx().request_repaint();
            self.error = self
                .save()
                .err()
                .map(|error| format!("Could not save theme: {error}"));
        }
        if self.preference == egui::ThemePreference::System && ui.ctx().system_theme().is_none() {
            ui.colored_label(
                Palette::of(ui).secondary,
                "System appearance unavailable; using Dark.",
            );
        }
    }

    /// Shows a theme save error in the status bar.
    pub(crate) fn status(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            super::indicator::dot_label(ui, Tone::Danger, error);
        }
    }

    fn save(&self) -> std::io::Result<()> {
        use std::io::Write as _;
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| std::io::Error::other("settings directory unavailable"))?;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("invalid settings path"))?;
        std::fs::create_dir_all(parent)?;
        publish(path, ExistingFile::Replace, |file| {
            writeln!(file, "{}", serialize(self.preference))
        })
    }
}

/// Asserts WCAG AA contrast (4.5:1) of `foreground` on every background a
/// text widget of `theme` can have. Use it for `text.primary` and
/// `text.secondary`.
#[cfg(test)]
pub(crate) fn assert_readable(theme: Theme, foreground: Color32) {
    assert_contrast(theme, foreground, &text_backgrounds(theme), 4.5);
}

/// Asserts WCAG AA contrast (4.5:1) of `foreground` on the data surfaces of
/// `theme`. Use it for status, category, and syntax text, which design rule 25
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
        pane.blend(visuals.widgets.hovered.bg_fill),
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
            for foreground in [
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
        // Dots, ticks, bars, and focus rings are non-text contrast (WCAG 1.4.11).
        for theme in [Theme::Light, Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            for mark in [
                tokens.accent.primary,
                tokens.accent.secondary,
                tokens.status.ok,
                tokens.status.warn,
                tokens.status.error,
                tokens.category(Category::Gray),
            ] {
                assert_contrast(theme, mark, &text_backgrounds(theme), 3.0);
            }
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
            let settings = ThemeSettings {
                preference,
                path: Some(path.clone()),
                error: None,
            };
            settings.save()?;
            let saved = std::fs::read_to_string(&path)?;
            assert_eq!(saved, expected);
            assert_eq!(parse(&saved), preference);
        }
        Ok(())
    }
}
