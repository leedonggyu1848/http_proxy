//! Copy as cURL action and redaction settings UI text catalog.

use http_inspector_interop::curl::CURL_REDACTED_PLACEHOLDER;

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct CopyCurlText {
        copy_as_curl,
        copied,
        redacted_placeholders,
    }
    tooltips: pub(crate) struct CopyCurlTooltips {
        copy_button_hint,
    }
    en = {
        copy_as_curl: "Copy as cURL",
        copied: "Copied as cURL.",
        redacted_placeholders: "Redacted placeholders",
    }
    en_tooltips = {
        copy_button_hint: "Copy this request as a curl command for bash or zsh.",
    }
    ko = {
        copy_as_curl: "cURL로 복사",
        copied: "cURL로 복사했습니다.",
        redacted_placeholders: "가려진 헤더 자리표시자",
    }
    ko_tooltips = {
        copy_button_hint: "이 요청을 bash 또는 zsh용 curl 명령으로 복사합니다.",
    }
}

/// Tooltip for the redacted-placeholders setting checkbox.
#[must_use]
pub(crate) fn redacted_placeholders_hint(lang: Language) -> String {
    match lang {
        Language::English => format!(
            "Copied commands carry {CURL_REDACTED_PLACEHOLDER} for Authorization and Cookie values, which are never stored."
        ),
        Language::Korean => format!(
            "저장되지 않는 Authorization 및 Cookie 값 자리에 {CURL_REDACTED_PLACEHOLDER} 자리표시자를 넣어 복사합니다."
        ),
    }
}

/// Notice when a request was copied with placeholder values for redacted headers.
#[must_use]
pub(crate) fn copied_with_placeholders(lang: Language, names: &str) -> String {
    match lang {
        Language::English => format!(
            "Copied as cURL with {CURL_REDACTED_PLACEHOLDER} placeholders for redacted headers \
             ({names}). Replace them with real values before running."
        ),
        Language::Korean => format!(
            "가려진 헤더({names})에 {CURL_REDACTED_PLACEHOLDER} 자리표시자를 넣어 cURL로 복사했습니다. \
             실행하기 전에 실제 값으로 바꾸세요."
        ),
    }
}

/// Notice when a request was copied omitting redacted headers.
#[must_use]
pub(crate) fn copied_without_redacted(lang: Language, names: &str) -> String {
    match lang {
        Language::English => format!(
            "Copied as cURL without redacted headers ({names}). Their values are not stored; \
             add them before running if the server needs them."
        ),
        Language::Korean => format!(
            "가려진 헤더({names})를 제외하고 cURL로 복사했습니다. 원래 값은 저장되지 않으므로 \
             서버에 필요하면 실행 전에 직접 추가하세요."
        ),
    }
}

/// Notice when copying as cURL was refused.
#[must_use]
pub(crate) fn refused(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Not copied as cURL: {detail}."),
        Language::Korean => format!("cURL로 복사하지 못했습니다: {detail}."),
    }
}
