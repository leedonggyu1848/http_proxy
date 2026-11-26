//! Parameterized UI text formatters with exhaustive [`Language`] matching.

use super::{Language, SettingsSaveError};

#[must_use]
pub(crate) fn language_save_error(lang: Language, error: &SettingsSaveError) -> String {
    match error {
        SettingsSaveError::DirectoryUnavailable => match lang {
            Language::English => {
                "Language setting could not be saved: settings directory is unavailable.".to_owned()
            }
            Language::Korean => {
                "설정 폴더를 찾을 수 없어 언어 설정을 저장하지 못했습니다.".to_owned()
            }
        },
        SettingsSaveError::Write(source) => match lang {
            Language::English => format!("Language setting could not be saved: {source}"),
            Language::Korean => format!("언어 설정을 저장하지 못했습니다: {source}"),
        },
    }
}

#[must_use]
pub(crate) fn theme_save_error(lang: Language, error: &SettingsSaveError) -> String {
    match error {
        SettingsSaveError::DirectoryUnavailable => match lang {
            Language::English => {
                "Theme setting could not be saved: settings directory is unavailable.".to_owned()
            }
            Language::Korean => {
                "설정 폴더를 찾을 수 없어 테마 설정을 저장하지 못했습니다.".to_owned()
            }
        },
        SettingsSaveError::Write(source) => match lang {
            Language::English => format!("Theme setting could not be saved: {source}"),
            Language::Korean => format!("테마 설정을 저장하지 못했습니다: {source}"),
        },
    }
}
