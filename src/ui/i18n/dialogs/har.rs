//! Export HAR modal dialog UI text catalog.

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct HarText {
        window_title,
        records,
        scope_note,
        include_heading,
        include_bodies,
        include_sensitive_headers,
        file_label,
        path_hint,
    }
    tooltips: pub(crate) struct HarTooltips {
        include_bodies,
        include_sensitive_headers,
        path_no_overwrite,
    }
    en = {
        window_title: "Export HAR",
        records: "Records",
        scope_note: "Only requests kept in the list since the last Clear are exported.",
        include_heading: "Include (off by default)",
        include_bodies: "Request and response bodies",
        include_sensitive_headers: "Authorization and cookie header names",
        file_label: "File",
        path_hint: "Path of a new .har file",
    }
    en_tooltips = {
        include_bodies: "Includes in-memory previews (8 KiB per direction, 8 MiB for images; truncated bodies are noted in HAR comments and may contain secrets).",
        include_sensitive_headers: "Adds Authorization, Proxy-Authorization, Cookie, and Set-Cookie as [REDACTED] without keeping original values.",
        path_no_overwrite: "An existing file is never replaced.",
    }
    ko = {
        window_title: "HAR 내보내기",
        records: "내보낼 기록",
        scope_note: "마지막 비우기 이후 목록에 유지된 요청만 내보냅니다.",
        include_heading: "포함 항목 (기본 꺼짐)",
        include_bodies: "요청 및 응답 본문",
        include_sensitive_headers: "Authorization 및 Cookie 헤더 이름",
        file_label: "파일",
        path_hint: "새 .har 파일 경로",
    }
    ko_tooltips = {
        include_bodies: "메모리 미리보기(방향당 8 KiB, 이미지 최대 8 MiB)를 포함하며, 잘린 본문은 HAR 주석으로 표시되고 비밀번호·토큰이 포함될 수 있습니다.",
        include_sensitive_headers: "Authorization, Proxy-Authorization, Cookie, Set-Cookie 헤더를 원본 값 없이 [REDACTED]로 추가합니다.",
        path_no_overwrite: "이미 존재하는 파일은 덮어쓰지 않습니다.",
    }
}

/// Radio label for exporting every record kept in the list.
#[must_use]
pub(crate) fn scope_all(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("All requests in the list ({count})"),
        Language::Korean => format!("목록의 모든 요청 ({count}건)"),
    }
}

/// Radio label for exporting only filtered records.
#[must_use]
pub(crate) fn scope_filtered(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("Requests matching the filter ({count})"),
        Language::Korean => format!("현재 필터와 일치하는 요청 ({count}건)"),
    }
}

/// Confirm button label with the number of requests to export.
#[must_use]
pub(crate) fn export_button(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("Export {count} requests"),
        Language::Korean => format!("요청 {count}건 내보내기"),
    }
}
