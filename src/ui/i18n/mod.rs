//! UI language selection, persistence, and screen-scoped translation catalogs.
//!
//! Domain crates under `crates/` remain language-agnostic; all user-facing UI
//! text and localized domain error messages live inside this module.
//!
//! Screen modules outside `src/ui/i18n/` must never call [`Language::pick`] or
//! branch on [`Language`] directly. Instead, they read static strings through
//! the screen catalog accessors on [`Language`] (such as [`Language::shared`],
//! [`Language::toolbar`], [`Language::requests`], [`Language::detail`],
//! [`Language::map`], and the dialog accessors) or call typed formatting
//! functions defined inside the corresponding catalog module.

/// Defines a zero-allocation static catalog struct with English and Korean
/// constant tables, a [`Language`] lookup method, and exhaustive test entry
/// iteration so adding a new field is checked in both languages and in policy
/// tests at compile time.
macro_rules! define_catalog {
    (
        pub(crate) struct $name:ident {
            $( $field:ident ),* $(,)?
        }
        en = {
            $( $en_field:ident : $en_val:expr ),* $(,)?
        }
        ko = {
            $( $ko_field:ident : $ko_val:expr ),* $(,)?
        }
    ) => {
        pub(crate) struct $name {
            $( pub(crate) $field: &'static str, )*
        }

        impl $name {
            pub(crate) const EN: Self = Self {
                $( $en_field: $en_val, )*
            };

            pub(crate) const KO: Self = Self {
                $( $ko_field: $ko_val, )*
            };

            #[must_use]
            pub(crate) const fn for_language(lang: $crate::ui::i18n::Language) -> &'static Self {
                match lang {
                    $crate::ui::i18n::Language::English => &Self::EN,
                    $crate::ui::i18n::Language::Korean => &Self::KO,
                }
            }

            #[cfg(test)]
            pub(crate) fn entries(&self) -> Vec<(&'static str, &'static str)> {
                vec![
                    $( (stringify!($field), self.$field), )*
                ]
            }
        }
    };
    (
        pub(crate) struct $name:ident {
            $( $field:ident ),* $(,)?
        }
        tooltips: pub(crate) struct $tip_name:ident {
            $( $tip_field:ident ),* $(,)?
        }
        en = {
            $( $en_field:ident : $en_val:expr ),* $(,)?
        }
        en_tooltips = {
            $( $en_tip_field:ident : $en_tip_val:expr ),* $(,)?
        }
        ko = {
            $( $ko_field:ident : $ko_val:expr ),* $(,)?
        }
        ko_tooltips = {
            $( $ko_tip_field:ident : $ko_tip_val:expr ),* $(,)?
        }
    ) => {
        pub(crate) struct $name {
            $( pub(crate) $field: &'static str, )*
            pub(crate) tooltips: $tip_name,
        }

        pub(crate) struct $tip_name {
            $( pub(crate) $tip_field: &'static str, )*
        }

        impl $tip_name {
            #[cfg(test)]
            pub(crate) fn entries(&self) -> Vec<(&'static str, &'static str)> {
                vec![
                    $( (stringify!($tip_field), self.$tip_field), )*
                ]
            }
        }

        impl $name {
            pub(crate) const EN: Self = Self {
                $( $en_field: $en_val, )*
                tooltips: $tip_name {
                    $( $en_tip_field: $en_tip_val, )*
                },
            };

            pub(crate) const KO: Self = Self {
                $( $ko_field: $ko_val, )*
                tooltips: $tip_name {
                    $( $ko_tip_field: $ko_tip_val, )*
                },
            };

            #[must_use]
            pub(crate) const fn for_language(lang: $crate::ui::i18n::Language) -> &'static Self {
                match lang {
                    $crate::ui::i18n::Language::English => &Self::EN,
                    $crate::ui::i18n::Language::Korean => &Self::KO,
                }
            }

            #[cfg(test)]
            pub(crate) fn entries(&self) -> Vec<(&'static str, &'static str)> {
                vec![
                    $( (stringify!($field), self.$field), )*
                ]
            }
        }
    };
}

pub(crate) mod detail;
pub(crate) mod dialogs;
pub(crate) mod domain;
pub(crate) mod fmt;
pub(crate) mod map;
pub(crate) mod requests;
pub(crate) mod shared;
pub(crate) mod toolbar;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use eframe::egui;
use http_inspector_core::settings::paths::settings_directory;
use http_inspector_file::{ExistingFile, publish};

use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

const LANGUAGE_FILE: &str = "language";

/// Resolved UI language used for rendering text.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Language {
    #[default]
    English,
    Korean,
}

impl Language {
    /// Picks the localized static string for this language inside `src/ui/i18n/`.
    ///
    /// Restricted to `i18n` submodules so screen code outside `src/ui/i18n/`
    /// cannot add inline translations.
    #[must_use]
    const fn pick(self, en: &'static str, ko: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::Korean => ko,
        }
    }

    /// Returns the shared static UI text catalog for this language.
    #[must_use]
    pub(crate) const fn shared(self) -> &'static shared::SharedText {
        shared::SharedText::for_language(self)
    }

    /// Returns the toolbar and status bar UI text catalog for this language.
    #[must_use]
    pub(crate) const fn toolbar(self) -> &'static toolbar::ToolbarText {
        toolbar::ToolbarText::for_language(self)
    }

    /// Returns the request list, filter, and timing UI text catalog for this language.
    #[must_use]
    pub(crate) const fn requests(self) -> &'static requests::RequestsText {
        requests::RequestsText::for_language(self)
    }

    /// Returns the request detail panel UI text catalog for this language.
    #[must_use]
    pub(crate) const fn detail(self) -> &'static detail::DetailText {
        detail::DetailText::for_language(self)
    }

    /// Returns the URL map and relationship graph UI text catalog for this language.
    #[must_use]
    pub(crate) const fn map(self) -> &'static map::MapText {
        map::MapText::for_language(self)
    }

    /// Returns the browser launcher menu UI text catalog for this language.
    #[must_use]
    pub(crate) const fn browser(self) -> &'static dialogs::browser::BrowserText {
        dialogs::browser::BrowserText::for_language(self)
    }

    /// Returns the inspection terminal button UI text catalog for this language.
    #[must_use]
    pub(crate) const fn terminal(self) -> &'static dialogs::terminal::TerminalText {
        dialogs::terminal::TerminalText::for_language(self)
    }

    /// Returns the CA registration panel UI text catalog for this language.
    #[must_use]
    pub(crate) const fn ca(self) -> &'static dialogs::ca::CaText {
        dialogs::ca::CaText::for_language(self)
    }

    /// Returns the Copy as cURL UI text catalog for this language.
    #[must_use]
    pub(crate) const fn copy_curl(self) -> &'static dialogs::copy_curl::CopyCurlText {
        dialogs::copy_curl::CopyCurlText::for_language(self)
    }

    /// Returns the Export HAR dialog UI text catalog for this language.
    #[must_use]
    pub(crate) const fn har(self) -> &'static dialogs::har::HarText {
        dialogs::har::HarText::for_language(self)
    }

    /// Returns the capture scope help popover UI text catalog for this language.
    #[must_use]
    pub(crate) const fn scope_help(self) -> &'static dialogs::scope_help::ScopeHelpText {
        dialogs::scope_help::ScopeHelpText::for_language(self)
    }

    /// Returns the Save menu UI text catalog for this language.
    #[must_use]
    pub(crate) const fn save(self) -> &'static dialogs::save::SaveText {
        dialogs::save::SaveText::for_language(self)
    }

    /// Returns the bulk body save dialog UI text catalog for this language.
    #[must_use]
    pub(crate) const fn bulk_save(self) -> &'static dialogs::bulk_save::BulkSaveText {
        dialogs::bulk_save::BulkSaveText::for_language(self)
    }

    /// Returns the single-body saving UI text catalog for this language.
    #[must_use]
    pub(crate) const fn body_saving(self) -> &'static dialogs::body_saving::BodySavingText {
        dialogs::body_saving::BodySavingText::for_language(self)
    }

    /// Returns the range merge dialog UI text catalog for this language.
    #[must_use]
    pub(crate) const fn range_merge(self) -> &'static dialogs::range_merge::RangeMergeText {
        dialogs::range_merge::RangeMergeText::for_language(self)
    }

    /// Returns the manual replay editor UI text catalog for this language.
    #[must_use]
    pub(crate) const fn replay(self) -> &'static dialogs::replay::ReplayText {
        dialogs::replay::ReplayText::for_language(self)
    }

    /// Returns the HLS media dialog UI text catalog for this language.
    #[must_use]
    pub(crate) const fn hls(self) -> &'static dialogs::hls::HlsText {
        dialogs::hls::HlsText::for_language(self)
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
            Self::System => active.shared().system,
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
        ui.label(active.shared().language);
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
