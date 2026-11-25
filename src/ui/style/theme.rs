//! Theme selection, accessible status colors, and settings at the filesystem boundary.
use eframe::egui::{self, Color32};
pub(crate) use egui::Theme;
use http_inspector_file::{ExistingFile, publish};

pub(crate) use super::tokens::{MatchColors, SyntaxPalette};
use super::tokens::{Radius, Space, StrokeWidth, ThemeTokens, TypeScale};
use http_inspector_content::format::TokenKind;

impl SyntaxPalette {
    pub(crate) const fn for_theme(theme: Theme) -> Self {
        ThemeTokens::for_theme(theme).syntax
    }

    pub(crate) const fn color(&self, kind: TokenKind, text: Color32) -> Color32 {
        match kind {
            TokenKind::Text => text,
            TokenKind::Key | TokenKind::AttributeName => self.key,
            TokenKind::String | TokenKind::AttributeValue | TokenKind::CData => self.string,
            TokenKind::Number | TokenKind::Entity => self.number,
            TokenKind::Literal | TokenKind::Namespace => self.literal,
            TokenKind::Punctuation => self.punctuation,
            TokenKind::Tag => self.tag,
            TokenKind::Comment | TokenKind::Instruction => self.comment,
        }
    }
}

impl MatchColors {
    pub(crate) const fn for_theme(theme: Theme) -> Self {
        ThemeTokens::for_theme(theme).search
    }
}

/// Semantic text colors of the current theme, from its design tokens.
pub(crate) struct Palette {
    pub(crate) success: Color32,
    pub(crate) error: Color32,
    pub(crate) warning: Color32,
    pub(crate) info: Color32,
    pub(crate) muted: Color32,
    pub(crate) text: Color32,
    pub(crate) surfaces: super::tokens::Surfaces,
}

impl Palette {
    pub(crate) fn of(ui: &egui::Ui) -> Self {
        Self::for_theme(ui.ctx().theme())
    }

    pub(crate) const fn for_theme(theme: Theme) -> Self {
        let tokens = ThemeTokens::for_theme(theme);
        Self {
            success: tokens.success,
            error: tokens.error,
            warning: tokens.warning,
            info: tokens.info,
            muted: tokens.muted,
            text: tokens.text,
            surfaces: tokens.surfaces,
        }
    }
}

/// Resolves the text color of the body's theme even when the context is
/// still painting with the previous frame's style.
pub(crate) fn body_text(context: &egui::Context, theme: Theme) -> Color32 {
    context.style_of(theme).visuals.text_color()
}

pub(crate) fn new_map_node_fill(ui: &egui::Ui, alpha: u8) -> Color32 {
    ThemeTokens::for_theme(ui.ctx().theme()).new_map_node_fill(alpha)
}

pub(crate) fn divider_stroke(ui: &egui::Ui, response: &egui::Response) -> egui::Stroke {
    let widgets = &ui.visuals().widgets;
    if response.dragged() {
        widgets.active.fg_stroke
    } else if response.hovered() {
        widgets.hovered.fg_stroke
    } else {
        structural_stroke(ui)
    }
}

pub(crate) fn structural_stroke(ui: &egui::Ui) -> egui::Stroke {
    ui.visuals().widgets.noninteractive.bg_stroke
}

pub(crate) fn image_border(ui: &egui::Ui) -> egui::Stroke {
    ui.visuals().window_stroke
}

pub(crate) fn image_checker_colors(ui: &egui::Ui) -> [Color32; 2] {
    let surfaces = Palette::of(ui).surfaces;
    [surfaces.inset, surfaces.hover]
}

/// egui visuals of `theme`, built from its design tokens.
fn visuals(theme: Theme) -> egui::Visuals {
    let tokens = ThemeTokens::for_theme(theme);
    let surfaces = tokens.surfaces;
    let mut visuals = theme.default_visuals();
    visuals.override_text_color = Some(tokens.text);
    visuals.weak_text_alpha = 1.0;
    visuals.weak_text_color = Some(tokens.muted);
    visuals.warn_fg_color = tokens.warning;
    visuals.error_fg_color = tokens.error;
    visuals.hyperlink_color = tokens.info;

    visuals.panel_fill = surfaces.bg;
    visuals.window_fill = surfaces.raised;
    visuals.extreme_bg_color = surfaces.inset;
    visuals.text_edit_bg_color = Some(surfaces.inset);
    visuals.code_bg_color = surfaces.stripe;
    visuals.faint_bg_color = surfaces.stripe;
    visuals.window_stroke.color = surfaces.border;
    visuals.window_corner_radius = egui::CornerRadius::same(Radius::CARD);
    visuals.menu_corner_radius = egui::CornerRadius::same(Radius::CARD);

    let control = egui::CornerRadius::same(Radius::CONTROL);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = surfaces.panel;
    widgets.noninteractive.weak_bg_fill = surfaces.panel;
    widgets.noninteractive.bg_stroke.color = surfaces.border;
    for (widget, fill) in [
        (&mut widgets.inactive, surfaces.raised),
        (&mut widgets.hovered, surfaces.hover),
        (&mut widgets.active, surfaces.hover),
        (&mut widgets.open, surfaces.hover),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
    }
    // Outline resting controls so that white buttons and checkboxes stay
    // visible on light surfaces.
    widgets.inactive.bg_stroke = egui::Stroke::new(StrokeWidth::CONTROL, surfaces.border);
    widgets.hovered.bg_stroke.color = tokens.muted;
    widgets.active.bg_stroke.color = tokens.accent;
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.fg_stroke.color = tokens.text;
        widget.corner_radius = control;
    }
    visuals.selection.bg_fill = surfaces.selection;
    visuals.selection.stroke.color = tokens.accent;
    visuals
}

/// Applies the spacing and type scales shared by both themes.
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
    for (text_style, size, family) in [
        (
            TextStyle::Heading,
            TypeScale::HEADING,
            FontFamily::Proportional,
        ),
        (TextStyle::Body, TypeScale::BODY, FontFamily::Proportional),
        (TextStyle::Button, TypeScale::BODY, FontFamily::Proportional),
        (TextStyle::Monospace, TypeScale::DATA, FontFamily::Monospace),
        (TextStyle::Small, TypeScale::SMALL, FontFamily::Proportional),
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
    pub(crate) fn menu(&self, ui: &mut egui::Ui) -> Option<egui::ThemePreference> {
        let mut preference = self.preference;
        ui.label("Theme");
        for (value, label) in [
            (egui::ThemePreference::System, "System"),
            (egui::ThemePreference::Light, "Light"),
            (egui::ThemePreference::Dark, "Dark"),
        ] {
            ui.radio_value(&mut preference, value, label);
        }
        if preference == egui::ThemePreference::System && ui.ctx().system_theme().is_none() {
            ui.colored_label(
                Palette::of(ui).muted,
                "System appearance unavailable; using Dark.",
            );
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
        self.error = self
            .save()
            .err()
            .map(|error| format!("Could not save theme: {error}"));
    }

    /// Shows a theme save error in the status bar.
    pub(crate) fn status(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(Palette::of(ui).error, error);
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
/// text widget of `theme` can have.
#[cfg(test)]
pub(crate) fn assert_readable(theme: Theme, foreground: Color32) {
    for background in text_backgrounds(theme) {
        let contrast = super::tokens::contrast_ratio(foreground, background);
        assert!(
            contrast >= 4.5,
            "{theme:?}: {foreground:?} on {background:?}: {contrast}"
        );
    }
}

/// Every surface that text of `theme` is drawn on.
#[cfg(test)]
fn text_backgrounds(theme: Theme) -> [Color32; 9] {
    let visuals = visuals(theme);
    [
        visuals.panel_fill,
        ThemeTokens::for_theme(theme).surfaces.panel,
        visuals.window_fill,
        visuals.extreme_bg_color,
        visuals.faint_bg_color,
        visuals.widgets.inactive.weak_bg_fill,
        visuals.widgets.hovered.bg_fill,
        visuals.widgets.hovered.weak_bg_fill,
        visuals.selection.bg_fill,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_meets_aa_on_all_widget_backgrounds() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = Palette::for_theme(theme);
            let syntax = SyntaxPalette::for_theme(theme);
            for foreground in [
                syntax.key,
                syntax.string,
                syntax.number,
                syntax.literal,
                syntax.punctuation,
                syntax.tag,
                syntax.comment,
                palette.success,
                palette.error,
                palette.warning,
                palette.info,
                palette.muted,
                visuals(theme).text_color(),
                // Selected rows draw their text in the selection stroke color.
                visuals(theme).selection.stroke.color,
            ] {
                assert_readable(theme, foreground);
            }
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
