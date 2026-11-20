//! Theme selection, accessible status colors, and settings at the filesystem boundary.
use eframe::egui::{self, Color32};
pub(crate) use egui::Theme;

pub(crate) struct Palette {
    pub(crate) success: Color32,
    pub(crate) error: Color32,
    pub(crate) warning: Color32,
    pub(crate) info: Color32,
    pub(crate) muted: Color32,
}

impl Palette {
    pub(crate) fn of(ui: &egui::Ui) -> Self {
        Self::for_theme(ui.ctx().theme())
    }

    const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                success: Color32::from_rgb(0, 90, 40),
                error: Color32::from_rgb(160, 15, 25),
                warning: Color32::from_rgb(110, 65, 0),
                info: Color32::from_rgb(0, 65, 150),
                muted: Color32::from_gray(70),
            },
            Theme::Dark => Self {
                success: Color32::from_rgb(140, 240, 170),
                error: Color32::from_rgb(255, 170, 170),
                warning: Color32::from_rgb(255, 220, 130),
                info: Color32::from_rgb(160, 210, 255),
                muted: Color32::from_gray(205),
            },
        }
    }
}

fn visuals(theme: Theme) -> egui::Visuals {
    let mut visuals = theme.default_visuals();
    let palette = Palette::for_theme(theme);
    let text = match theme {
        Theme::Light => Color32::from_gray(35),
        Theme::Dark => Color32::from_gray(225),
    };
    visuals.override_text_color = Some(text);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.fg_stroke.color = text;
    }
    visuals.weak_text_alpha = 1.0;
    visuals.weak_text_color = Some(palette.muted);
    visuals.warn_fg_color = palette.warning;
    visuals.error_fg_color = palette.error;
    visuals.hyperlink_color = palette.info;
    let (stripe, hover, selection) = match theme {
        Theme::Light => (242, 232, 218),
        Theme::Dark => (32, 40, 48),
    };
    visuals.faint_bg_color = Color32::from_gray(stripe);
    visuals.widgets.hovered.bg_fill = Color32::from_gray(hover);
    visuals.selection.bg_fill = Color32::from_gray(selection);
    visuals.selection.stroke.color = palette.info;
    visuals
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
        let path = crate::port_settings::default_settings_directory()
            .map(|directory| directory.join("theme"));
        let preference = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map_or(egui::ThemePreference::System, |value| parse(&value));
        context.options_mut(|options| options.fallback_theme = Theme::Dark);
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

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let previous = self.preference;
        ui.horizontal(|ui| {
            ui.label("Theme:");
            for (value, label) in [
                (egui::ThemePreference::System, "System"),
                (egui::ThemePreference::Light, "Light"),
                (egui::ThemePreference::Dark, "Dark"),
            ] {
                ui.selectable_value(&mut self.preference, value, label);
            }
        });
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
                Palette::of(ui).muted,
                "System appearance unavailable; using Dark.",
            );
        }
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
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        writeln!(file, "{}", serialize(self.preference))?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn status_text_meets_aa_on_all_widget_backgrounds() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = Palette::for_theme(theme);
            let visuals = visuals(theme);
            for foreground in [
                palette.success,
                palette.error,
                palette.warning,
                palette.info,
                palette.muted,
                visuals.text_color(),
            ] {
                for background in [
                    visuals.panel_fill,
                    visuals.window_fill,
                    visuals.extreme_bg_color,
                    visuals.faint_bg_color,
                    visuals.widgets.hovered.bg_fill,
                    visuals.selection.bg_fill,
                ] {
                    let a = luminance(foreground);
                    let b = luminance(background);
                    let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(
                        contrast >= 4.5,
                        "{theme:?}: {foreground:?} on {background:?}: {contrast}"
                    );
                }
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
