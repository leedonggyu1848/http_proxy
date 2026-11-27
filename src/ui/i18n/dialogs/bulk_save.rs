//! Bulk body save dialog and status bar progress UI text catalog.

use http_inspector_core::recording::BulkSummary;
use http_inspector_exchange::ByteSize;

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct BulkSaveText {
        unavailable,
        heading,
        cancelling,
        requests_label,
        selected_request,
        bodies_label,
        responses,
        requests,
        both,
        format_label,
        decoded,
        as_received,
        folders_label,
        one_folder,
        host_path_folders,
        manifest_note,
        cancelled,
    }
    tooltips: pub(crate) struct BulkSaveTooltips {
        decoded_hint,
        as_received_hint,
    }
    en = {
        unavailable: "Saving bodies is unavailable",
        heading: "Save bodies",
        cancelling: "Cancelling…",
        requests_label: "Requests",
        selected_request: "Selected request",
        bodies_label: "Bodies",
        responses: "Responses",
        requests: "Requests",
        both: "Both",
        format_label: "Format",
        decoded: "Decoded",
        as_received: "As received",
        folders_label: "Folders",
        one_folder: "One folder",
        host_path_folders: "host/path folders",
        manifest_note: "Files go into a new folder inside the Save directory, with a manifest.json that lists each URL, status, size and SHA-256. Empty bodies are skipped.",
        cancelled: "cancelled",
    }
    en_tooltips = {
        decoded_hint: "Remove gzip, deflate, br and zstd Content-Encoding.",
        as_received_hint: "Keep the exact bytes that were received.",
    }
    ko = {
        unavailable: "본문 저장을 사용할 수 없습니다",
        heading: "본문 일괄 저장",
        cancelling: "취소 중…",
        requests_label: "요청 범위",
        selected_request: "선택한 요청",
        bodies_label: "대상 본문",
        responses: "응답",
        requests: "요청",
        both: "모두",
        format_label: "저장 형식",
        decoded: "디코딩됨",
        as_received: "수신 원본 그대로",
        folders_label: "폴더 구조",
        one_folder: "단일 폴더",
        host_path_folders: "host/path 폴더 구조",
        manifest_note: "저장 폴더 안의 새 폴더에 파일이 저장되며, 각 URL·상태·크기·SHA-256을 기록한 manifest.json이 함께 생성됩니다. 빈 본문은 건너뜁니다.",
        cancelled: "취소됨",
    }
    ko_tooltips = {
        decoded_hint: "gzip, deflate, br, zstd Content-Encoding을 해제해 저장합니다.",
        as_received_hint: "수신한 바이트 그대로 저장합니다.",
    }
}

/// Progress label while a bulk save is running.
#[must_use]
pub(crate) fn saving_progress(lang: Language, bytes: u64) -> String {
    let size = ByteSize::new(bytes);
    match lang {
        Language::English => format!("Saving bodies · {size}"),
        Language::Korean => format!("본문 저장 중 · {size}"),
    }
}

/// Error label when a bulk save fails.
#[must_use]
pub(crate) fn failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Saving bodies failed: {detail}"),
        Language::Korean => format!("본문 일괄 저장 실패: {detail}"),
    }
}

/// Radio label for saving bodies of filtered requests.
#[must_use]
pub(crate) fn scope_filtered(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("Filtered list ({count})"),
        Language::Korean => format!("필터된 목록 ({count}건)"),
    }
}

/// Radio label for saving bodies of all requests.
#[must_use]
pub(crate) fn scope_all(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("All ({count})"),
        Language::Korean => format!("전체 ({count}건)"),
    }
}

/// Summary line after a bulk save finishes.
#[must_use]
pub(crate) fn summary_text(lang: Language, summary: &BulkSummary) -> String {
    let mut parts = vec![match lang {
        Language::English => format!(
            "Saved {} bodies to {}",
            summary.saved(),
            summary.directory().display()
        ),
        Language::Korean => format!(
            "본문 {}건을 {}에 저장했습니다",
            summary.saved(),
            summary.directory().display()
        ),
    }];
    if summary.incomplete() > 0 {
        parts.push(match lang {
            Language::English => format!("{} incomplete", summary.incomplete()),
            Language::Korean => format!("불완전 {}건", summary.incomplete()),
        });
    }
    if summary.failed() > 0 {
        parts.push(match lang {
            Language::English => format!("{} failed (see manifest.json)", summary.failed()),
            Language::Korean => format!("실패 {}건 (manifest.json 확인)", summary.failed()),
        });
    }
    if summary.cancelled() {
        parts.push(lang.bulk_save().cancelled.to_owned());
    }
    parts.join(" · ")
}
