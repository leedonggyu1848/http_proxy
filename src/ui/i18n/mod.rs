//! UI language selection, persistence, and translation catalogs.
//!
//! Domain crates under `crates/` remain language-agnostic; all user-facing UI
//! text and localized domain error messages live inside this module.

pub(crate) mod domain;
pub(crate) mod fmt;
pub(crate) mod text;

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use eframe::egui;
use http_inspector_core::settings::paths::settings_directory;
use http_inspector_file::{ExistingFile, publish};

use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

pub(crate) use text::Text;

const LANGUAGE_FILE: &str = "language";

/// Resolved UI language used for rendering text.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Language {
    #[default]
    English,
    Korean,
}

impl Language {
    /// Picks the localized static string for this language.
    ///
    /// Keeping every language branch explicit here ensures adding a future
    /// language triggers compile-time exhaustiveness checks across the UI.
    #[must_use]
    pub(crate) const fn pick(self, en: &'static str, ko: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::Korean => ko,
        }
    }

    /// Resolves a static UI text key into this language.
    #[must_use]
    pub(crate) const fn text(self, key: Text) -> &'static str {
        key.resolve(self)
    }
}

/// User preference for UI language.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LanguagePreference {
    /// Follow the process locale environment (`LC_ALL`, `LC_MESSAGES`, `LANG`).
    #[default]
    System,
    English,
    Korean,
}

impl LanguagePreference {
    const ALL: [Self; 3] = [Self::System, Self::English, Self::Korean];

    const fn storage_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::Korean => "ko",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "en" | "english" => Some(Self::English),
            "ko" | "korean" => Some(Self::Korean),
            _ => None,
        }
    }

    /// Menu label for this preference.
    ///
    /// Language names keep their native endonyms (`English`, `한국어`)
    /// regardless of the active UI language so users can always recognize
    /// and switch back to their preferred language.
    const fn menu_label(self, active: Language) -> &'static str {
        match self {
            Self::System => active.text(Text::OptionSystem),
            Self::English => "English",
            Self::Korean => "한국어",
        }
    }

    const fn resolve(self, system: Language) -> Language {
        match self {
            Self::System => system,
            Self::English => Language::English,
            Self::Korean => Language::Korean,
        }
    }
}

/// Reason a UI settings file could not be persisted.
#[derive(Debug)]
pub(crate) enum SettingsSaveError {
    DirectoryUnavailable,
    Write(io::Error),
}

/// Saved UI language preference and its resolved active language.
pub(crate) struct LanguageSettings {
    preference: LanguagePreference,
    system: Language,
    path: Option<PathBuf>,
    error: Option<SettingsSaveError>,
}

impl LanguageSettings {
    /// Loads the saved language preference and resolves the system locale once.
    pub(crate) fn new() -> Self {
        let env = |name: &str| std::env::var_os(name);
        let path = settings_directory(env).map(|dir| dir.join(LANGUAGE_FILE));
        let system = detect_system_language(env);
        let preference = path
            .as_deref()
            .map_or(LanguagePreference::System, load_from);
        Self {
            preference,
            system,
            path,
            error: None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn fixed(language: Language) -> Self {
        let preference = match language {
            Language::English => LanguagePreference::English,
            Language::Korean => LanguagePreference::Korean,
        };
        Self {
            preference,
            system: language,
            path: None,
            error: None,
        }
    }

    /// Returns the currently active UI language.
    #[must_use]
    pub(crate) const fn language(&self) -> Language {
        self.preference.resolve(self.system)
    }

    /// Shows the language radio options inside the Settings menu.
    pub(crate) fn menu(&self, ui: &mut egui::Ui) -> Option<LanguagePreference> {
        let active = self.language();
        ui.label(active.text(Text::LanguageHeading));
        let mut chosen = None;
        for option in LanguagePreference::ALL {
            if ui
                .radio(self.preference == option, option.menu_label(active))
                .clicked()
                && option != self.preference
            {
                chosen = Some(option);
            }
        }
        chosen
    }

    /// Updates the language preference, persists it, and repaints immediately.
    pub(crate) fn set_preference(
        &mut self,
        context: &egui::Context,
        preference: LanguagePreference,
    ) {
        self.preference = preference;
        context.request_repaint();
        self.error =
            self.path
                .as_deref()
                .map_or(Some(SettingsSaveError::DirectoryUnavailable), |path| {
                    save_to(path, preference)
                        .err()
                        .map(SettingsSaveError::Write)
                });
    }

    pub(crate) const fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Shows a persistence failure in the status bar, matching [`crate::ui::style::theme::ThemeSettings`].
    pub(crate) fn status(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            indicator::dot_label(
                ui,
                Tone::Danger,
                fmt::language_save_error(self.language(), error),
            );
        }
    }
}

/// Detects the UI language from standard POSIX locale environment variables.
///
/// Precedence follows POSIX (`LC_ALL`, then `LC_MESSAGES`, then `LANG`).
/// Empty values are skipped. Unrecognized locales and `C` / `POSIX` fall back
/// to [`Language::English`].
#[must_use]
pub(crate) fn detect_system_language(mut env: impl FnMut(&str) -> Option<OsString>) -> Language {
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Some(raw) = env(variable) else {
            continue;
        };
        let value = raw.to_string_lossy();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        return language_from_locale(trimmed);
    }
    Language::English
}

/// Maps a single locale tag such as `ko_KR.UTF-8` or `ko-KR` to a supported [`Language`].
fn language_from_locale(locale: &str) -> Language {
    // Strip encoding (`.UTF-8`) and modifier (`@euro`) first, then split language subtag.
    let base = locale.split(['.', '@']).next().unwrap_or_default().trim();
    let primary = base.split(['_', '-']).next().unwrap_or_default();
    if primary.eq_ignore_ascii_case("ko") {
        Language::Korean
    } else {
        Language::English
    }
}

fn load_from(path: &Path) -> LanguagePreference {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| LanguagePreference::parse(&text))
        .unwrap_or(LanguagePreference::System)
}

fn save_to(path: &Path, preference: LanguagePreference) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    publish(path, ExistingFile::Replace, |file| {
        writeln!(file, "{}", preference.storage_value())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn parses_known_preferences_and_rejects_unknown_values() {
        assert_eq!(
            LanguagePreference::parse("system"),
            Some(LanguagePreference::System)
        );
        assert_eq!(
            LanguagePreference::parse("  EN \n"),
            Some(LanguagePreference::English)
        );
        assert_eq!(
            LanguagePreference::parse("english"),
            Some(LanguagePreference::English)
        );
        assert_eq!(
            LanguagePreference::parse("ko\n"),
            Some(LanguagePreference::Korean)
        );
        assert_eq!(
            LanguagePreference::parse("KO"),
            Some(LanguagePreference::Korean)
        );
        assert_eq!(
            LanguagePreference::parse("korean"),
            Some(LanguagePreference::Korean)
        );
        assert_eq!(LanguagePreference::parse(""), None);
        assert_eq!(LanguagePreference::parse("ja"), None);
        assert_eq!(LanguagePreference::parse("ko_KR"), None);
    }

    #[test]
    fn saves_and_loads_language_preference_and_falls_back_on_invalid_file() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested").join(LANGUAGE_FILE);

        assert_eq!(load_from(&path), LanguagePreference::System);

        for preference in LanguagePreference::ALL {
            save_to(&path, preference)?;
            assert_eq!(load_from(&path), preference);
        }

        fs::write(&path, "unexpected-value\n")?;
        assert_eq!(load_from(&path), LanguagePreference::System);

        fs::write(&path, "")?;
        assert_eq!(load_from(&path), LanguagePreference::System);
        Ok(())
    }

    #[test]
    fn detects_system_language_following_posix_precedence() {
        // LC_ALL overrides LC_MESSAGES and LANG.
        assert_eq!(
            detect_system_language(env_from(&[
                ("LC_ALL", "ko_KR.UTF-8"),
                ("LC_MESSAGES", "en_US.UTF-8"),
                ("LANG", "en_US.UTF-8"),
            ])),
            Language::Korean
        );
        assert_eq!(
            detect_system_language(env_from(&[
                ("LC_ALL", "C"),
                ("LC_MESSAGES", "ko_KR.UTF-8"),
                ("LANG", "ko_KR.UTF-8"),
            ])),
            Language::English
        );
        // Empty LC_ALL falls through to LC_MESSAGES.
        assert_eq!(
            detect_system_language(env_from(&[
                ("LC_ALL", "   "),
                ("LC_MESSAGES", "ko-KR"),
                ("LANG", "en_US.UTF-8"),
            ])),
            Language::Korean
        );
        // Empty LC_ALL and LC_MESSAGES fall through to LANG.
        assert_eq!(
            detect_system_language(env_from(&[
                ("LC_ALL", ""),
                ("LC_MESSAGES", ""),
                ("LANG", "ko"),
            ])),
            Language::Korean
        );
        // Missing environment falls back to English.
        assert_eq!(detect_system_language(env_from(&[])), Language::English);
    }

    #[test]
    fn parses_locale_variants_without_false_positives() {
        for korean in [
            "ko",
            "KO",
            "ko_KR",
            "ko-KR",
            "ko_KR.UTF-8",
            "ko_KR.utf8@modifier",
        ] {
            assert_eq!(language_from_locale(korean), Language::Korean, "{korean}");
        }
        for english in [
            "en_US.UTF-8",
            "C",
            "POSIX",
            "ja_JP.UTF-8",
            "kok_IN.UTF-8",
            "koi8-r",
            "",
        ] {
            assert_eq!(
                language_from_locale(english),
                Language::English,
                "{english}"
            );
        }
    }

    #[test]
    fn language_menu_labels_preserve_native_endonyms() {
        for active in [Language::English, Language::Korean] {
            assert_eq!(LanguagePreference::English.menu_label(active), "English");
            assert_eq!(LanguagePreference::Korean.menu_label(active), "한국어");
        }
        assert_eq!(
            LanguagePreference::System.menu_label(Language::English),
            "System"
        );
        assert_eq!(
            LanguagePreference::System.menu_label(Language::Korean),
            "시스템"
        );
    }
}
