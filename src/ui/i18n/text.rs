//! Shared static UI text catalog.
//!
//! Every [`Text`] variant defines both English and Korean strings via
//! [`Language::pick`], guaranteeing compile-time coverage for all languages.

use super::Language;

/// Shared static UI string key used across multiple modules.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Text {
    // Shared controls and options
    OptionSystem,
    ThemeHeading,
    ThemeLight,
    ThemeDark,
    LanguageHeading,
    Copy,
    Save,
    Cancel,
    Dismiss,
    Clear,
    Raw,
    Pretty,
    Tree,
    Open,
    Closed,

    // Request table & context menu
    ColMethod,
    ColStatus,
    ColPath,
    ColType,
    ColSize,
    CtxCopyAsCurl,

    // Detail body notices & image notes
    DetailBodyNoticeFirst8KiBRetained,
    DetailBodyNoticeFirst8KiBDecoded,
    DetailBodyNoticeIncompleteDecoded,
    DetailBodyNoticeCompleteDecoded,
    DetailBodyNoticeCssIncomplete,
    DetailBodyImageSvgTextNote,
    DetailBodyImageSvgExternalNote,
}

impl Text {
    /// Resolves this key to a static string in `lang`.
    #[must_use]
    pub(crate) const fn resolve(self, lang: Language) -> &'static str {
        match self {
            Self::OptionSystem => lang.pick("System", "시스템"),
            Self::ThemeHeading => lang.pick("Theme", "테마"),
            Self::ThemeLight => lang.pick("Light", "밝게"),
            Self::ThemeDark => lang.pick("Dark", "어둡게"),
            Self::LanguageHeading => lang.pick("Language", "언어"),
            Self::Copy => lang.pick("Copy", "복사"),
            Self::Save => lang.pick("Save", "저장"),
            Self::Cancel => lang.pick("Cancel", "취소"),
            Self::Dismiss => lang.pick("Dismiss", "닫기"),
            Self::Clear => lang.pick("Clear", "비우기"),
            Self::Raw => lang.pick("Raw", "원문"),
            Self::Pretty => lang.pick("Pretty", "서식"),
            Self::Tree => lang.pick("Tree", "트리"),
            Self::Open => lang.pick("Open", "열림"),
            Self::Closed => lang.pick("Closed", "닫힘"),

            Self::ColMethod => lang.pick("Method", "메서드"),
            Self::ColStatus => lang.pick("Status", "상태"),
            Self::ColPath => lang.pick("Path", "경로"),
            Self::ColType => lang.pick("Type", "형식"),
            Self::ColSize => lang.pick("Size", "크기"),
            Self::CtxCopyAsCurl => lang.pick("Copy as cURL", "cURL로 복사"),

            Self::DetailBodyNoticeFirst8KiBRetained => lang.pick(
                "first 8 KiB of retained bytes",
                "보관된 바이트의 처음 8 KiB",
            ),
            Self::DetailBodyNoticeFirst8KiBDecoded => lang.pick(
                "first 8 KiB of decoded bytes",
                "디코딩된 바이트의 처음 8 KiB",
            ),
            Self::DetailBodyNoticeIncompleteDecoded => lang.pick(
                "decoded prefix; compressed capture is incomplete",
                "앞부분만 디코딩됨 (압축 캡처 불완전)",
            ),
            Self::DetailBodyNoticeCompleteDecoded => {
                lang.pick("complete decoded preview", "전체 디코딩 미리보기")
            }
            Self::DetailBodyNoticeCssIncomplete => lang.pick(
                "Incomplete or invalid CSS; showing the available prefix. CSS property values are not validated.",
                "불완전하거나 잘못된 CSS입니다. 해석 가능한 앞부분만 표시하며 속성값은 검증하지 않습니다.",
            ),
            Self::DetailBodyImageSvgTextNote => lang.pick(
                "text is not drawn; use Source to read it",
                "텍스트는 그려지지 않습니다. 소스 보기에서 확인하세요",
            ),
            Self::DetailBodyImageSvgExternalNote => lang.pick(
                "embedded images and foreign content are not loaded",
                "외부 이미지와 외부 콘텐츠는 불러오지 않습니다",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_TEXTS: [Text; 28] = [
        Text::OptionSystem,
        Text::ThemeHeading,
        Text::ThemeLight,
        Text::ThemeDark,
        Text::LanguageHeading,
        Text::Copy,
        Text::Save,
        Text::Cancel,
        Text::Dismiss,
        Text::Clear,
        Text::Raw,
        Text::Pretty,
        Text::Tree,
        Text::Open,
        Text::Closed,
        Text::ColMethod,
        Text::ColStatus,
        Text::ColPath,
        Text::ColType,
        Text::ColSize,
        Text::CtxCopyAsCurl,
        Text::DetailBodyNoticeFirst8KiBRetained,
        Text::DetailBodyNoticeFirst8KiBDecoded,
        Text::DetailBodyNoticeIncompleteDecoded,
        Text::DetailBodyNoticeCompleteDecoded,
        Text::DetailBodyNoticeCssIncomplete,
        Text::DetailBodyImageSvgTextNote,
        Text::DetailBodyImageSvgExternalNote,
    ];

    #[test]
    fn all_text_variants_have_non_empty_translations() {
        for key in ALL_TEXTS {
            for lang in [Language::English, Language::Korean] {
                let value = key.resolve(lang);
                assert!(
                    !value.trim().is_empty(),
                    "{key:?} resolved to empty text for {lang:?}"
                );
            }
        }
    }
}
