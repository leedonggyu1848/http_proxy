//! 206 Partial Content range merge modal UI text catalog.

use http_inspector_core::recording::RangePlan;
use http_inspector_exchange::ByteSize;

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct RangeMergeText {
        menu_item,
        heading,
        missing_validator_warning,
        save_incomplete,
        checking_ranges,
    }
    tooltips: pub(crate) struct RangeMergeTooltips {
        menu_item_hint,
    }
    en = {
        menu_item: "Merge ranges…",
        heading: "Merge ranges",
        missing_validator_warning: "Without ETag or Last-Modified, parts of different versions of this URL may be mixed.",
        save_incomplete: "Save incomplete file",
        checking_ranges: "Checking the range responses…",
    }
    en_tooltips = {
        menu_item_hint: "Join the 206 responses for this URL with the same ETag or Last-Modified into one file, in position order.",
    }
    ko = {
        menu_item: "범위 응답 병합…",
        heading: "범위 응답 병합",
        missing_validator_warning: "ETag 또는 Last-Modified가 없으면 이 URL의 서로 다른 버전 조각이 섞일 수 있습니다.",
        save_incomplete: "불완전한 파일 저장",
        checking_ranges: "범위 응답을 확인하는 중…",
    }
    ko_tooltips = {
        menu_item_hint: "같은 ETag 또는 Last-Modified를 가진 이 URL의 206 응답들을 바이트 위치 순서대로 하나의 파일로 합칩니다.",
    }
}

/// Byte range summary shown above "Merge ranges…" in the body Save menu.
#[must_use]
pub(crate) fn range_text(lang: Language, start: u64, end: u64, total: Option<u64>) -> String {
    match lang {
        Language::English => {
            let total = total.map_or_else(|| "unknown".to_owned(), |total| total.to_string());
            format!("206 range: bytes {start}-{end} of {total}")
        }
        Language::Korean => total.map_or_else(
            || format!("206 범위: {start}-{end} / 전체 알 수 없음"),
            |total| format!("206 범위: {start}-{end} / 전체 {total}바이트"),
        ),
    }
}

/// Plan summary paragraph shown at the top of the range merge confirmation modal.
#[must_use]
pub(crate) fn plan_text(lang: Language, plan: &RangePlan) -> String {
    let covered = ByteSize::new(plan.covered_bytes());
    let file_size = ByteSize::new(plan.file_bytes());
    let parts = plan.parts();
    match lang {
        Language::English => {
            let total = plan.total().map_or_else(
                || "unknown length".to_owned(),
                |bytes| ByteSize::new(bytes).to_string(),
            );
            let validator = plan.validator().map_or_else(
                || "no ETag or Last-Modified".to_owned(),
                |value| format!("validator {value}"),
            );
            format!(
                "{parts} responses cover {covered} of {total} · {validator}. The file will be {file_size}."
            )
        }
        Language::Korean => {
            let total = plan.total().map_or_else(
                || "전체 길이 알 수 없음".to_owned(),
                |bytes| ByteSize::new(bytes).to_string(),
            );
            let validator = plan.validator().map_or_else(
                || "ETag 또는 Last-Modified 없음".to_owned(),
                |value| format!("검증자 {value}"),
            );
            format!(
                "응답 {parts}건이 전체 {total} 중 {covered} 범위를 포함 · {validator}. 생성될 파일 크기는 {file_size}입니다."
            )
        }
    }
}

/// One missing byte-range line in the merge confirmation dialog.
#[must_use]
pub(crate) fn missing_gap(lang: Language, first: u64, last: u64, length: u64) -> String {
    let len = ByteSize::new(length);
    match lang {
        Language::English => format!("Missing bytes {first}-{last} ({len})"),
        Language::Korean => format!("누락된 바이트 구간 {first}-{last} ({len})"),
    }
}

/// Overflow line when more missing byte gaps exist than shown.
#[must_use]
pub(crate) fn more_gaps(lang: Language, more: usize) -> String {
    match lang {
        Language::English => format!("… and {more} more gaps"),
        Language::Korean => format!("… 외 누락 구간 {more}개"),
    }
}

/// Incomplete-file `.partial` suffix notice in the range merge confirmation dialog.
#[must_use]
pub(crate) fn partial_file_note(lang: Language, reason: &str) -> String {
    match lang {
        Language::English => format!("{reason} The file name will end in .partial."),
        Language::Korean => format!("{reason} 파일 이름 끝에 .partial이 붙습니다."),
    }
}
