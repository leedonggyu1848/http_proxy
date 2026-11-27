//! Request detail panel, headers, body preview, image viewer, tree view, and search UI text catalog.

use http_inspector_content::format::{BodyFormat, FormatError, Notice};
use http_inspector_content::image_file;
use http_inspector_content::view;
use http_inspector_exchange::capture::{IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES};
use http_inspector_exchange::{BodyLifecycle, BodySnapshot, ImageFormat};

use super::{Language, domain};

define_catalog! {
    pub(crate) struct DetailText {
        request_details,
        headers,
        body,
        timing,
        no_request_selected,
        hls_media,
        query,
        headers_omitted,
        tunnel_state,
        tunnel_sent,
        tunnel_received,
        duration,
        request_body,
        response_body,
        updating,
        preview_omitted,
        previous,
        next,
        no_matches,
        filter_keys,
        filter,
        filter_regex,
        filter_requests,
        invalid_filter,
        invalid_condition,
        exclude,
        phrase,
        image,
        source,
        show_svg_source,
        show_bytes_as_hex,
        search_matches_in_text_views,
        notice_first_8kib_retained,
        notice_first_8kib_decoded,
        notice_incomplete_decoded,
        notice_complete_decoded,
        notice_css_incomplete,
        svg_text_note,
        svg_external_note,
    }
    tooltips: pub(crate) struct DetailTooltips {
        preview_omitted_hint,
        tree_unavailable,
        clear_filter,
        filter_syntax,
        use_regex,
        match_case,
        search_scope,
    }
    en = {
        request_details: "Request details",
        headers: "Headers",
        body: "Body",
        timing: "Timing",
        no_request_selected: "No request selected",
        hls_media: "HLS media…",
        query: "Query",
        headers_omitted: "More headers omitted",
        tunnel_state: "State",
        tunnel_sent: "Sent",
        tunnel_received: "Received",
        duration: "Duration",
        request_body: "Request body",
        response_body: "Response body",
        updating: "Updating…",
        preview_omitted: "Preview omitted",
        previous: "Previous",
        next: "Next",
        no_matches: "No matches",
        filter_keys: "Filter keys",
        filter: "method:POST url:/api status:4xx",
        filter_regex: "Filter with regex",
        filter_requests: "Filter requests",
        invalid_filter: "Invalid pattern",
        invalid_condition: "Invalid condition",
        exclude: "Exclude",
        phrase: "Phrase",
        image: "Image",
        source: "Source",
        show_svg_source: "Show SVG source",
        show_bytes_as_hex: "Show bytes as hex",
        search_matches_in_text_views: "Search matches are marked in Pretty and Raw.",
        notice_first_8kib_retained: "first 8 KiB of retained bytes",
        notice_first_8kib_decoded: "first 8 KiB of decoded bytes",
        notice_incomplete_decoded: "decoded prefix; compressed capture is incomplete",
        notice_complete_decoded: "complete decoded preview",
        notice_css_incomplete: "Incomplete or invalid CSS; showing the available prefix. CSS property values are not validated.",
        svg_text_note: "text is not drawn; use Source to read it",
        svg_external_note: "embedded images and foreign content are not loaded",
    }
    en_tooltips = {
        preview_omitted_hint: "The body was forwarded without keeping a preview, so forwarding never waited.",
        tree_unavailable: "Needs a JSON or XML body",
        clear_filter: "Clear filter",
        filter_syntax: "Filter syntax",
        use_regex: "Use a regular expression",
        match_case: "Match free-text case",
        search_scope: "Search scope for free text",
    }
    ko = {
        request_details: "요청 상세",
        headers: "헤더",
        body: "본문",
        timing: "타이밍",
        no_request_selected: "선택된 요청 없음",
        hls_media: "HLS 미디어…",
        query: "쿼리",
        headers_omitted: "추가 헤더 생략됨",
        tunnel_state: "상태",
        tunnel_sent: "보냄",
        tunnel_received: "받음",
        duration: "소요 시간",
        request_body: "요청 본문",
        response_body: "응답 본문",
        updating: "업데이트 중…",
        preview_omitted: "미리보기 생략됨",
        previous: "이전",
        next: "다음",
        no_matches: "일치 없음",
        filter_keys: "키 필터",
        filter: "method:POST url:/api status:4xx",
        filter_regex: "정규식으로 필터링",
        filter_requests: "요청 필터",
        invalid_filter: "잘못된 패턴",
        invalid_condition: "잘못된 조건",
        exclude: "제외",
        phrase: "구문",
        image: "이미지",
        source: "소스",
        show_svg_source: "SVG 소스 보기",
        show_bytes_as_hex: "바이트를 Hex로 보기",
        search_matches_in_text_views: "검색 일치 항목은 서식 및 원문 보기에서 표시됩니다.",
        notice_first_8kib_retained: "보관된 바이트의 처음 8 KiB",
        notice_first_8kib_decoded: "디코딩된 바이트의 처음 8 KiB",
        notice_incomplete_decoded: "앞부분만 디코딩됨 (압축 캡처 불완전)",
        notice_complete_decoded: "전체 디코딩 미리보기",
        notice_css_incomplete: "불완전하거나 잘못된 CSS입니다. 해석 가능한 앞부분만 표시하며 속성값은 검증하지 않습니다.",
        svg_text_note: "텍스트는 그려지지 않습니다. 소스 보기에서 확인하세요",
        svg_external_note: "외부 이미지와 외부 콘텐츠는 불러오지 않습니다",
    }
    ko_tooltips = {
        preview_omitted_hint: "전달이 지연되지 않도록 미리보기를 보관하지 않고 본문을 전달했습니다.",
        tree_unavailable: "JSON 또는 XML 본문이 필요합니다",
        clear_filter: "필터 비우기",
        filter_syntax: "필터 문법",
        use_regex: "정규식 사용",
        match_case: "자유 텍스트 대소문자 구분",
        search_scope: "자유 텍스트 검색 범위",
    }
}

/// Link from a resent request to the one it was sent from.
#[must_use]
pub(crate) fn replay_of(lang: Language, original: u64) -> String {
    match lang {
        Language::English => format!("Replay of #{original} — show original"),
        Language::Korean => format!("#{original}의 재전송 — 원본 보기"),
    }
}

/// Tooltip of the request filter: its keyboard shortcut.
#[must_use]
pub(crate) fn focus_filter(lang: Language, shortcut: &str) -> String {
    match lang {
        Language::English => format!("Focus filter ({shortcut})"),
        Language::Korean => format!("필터에 포커스 ({shortcut})"),
    }
}

/// Status bar tooltip when background search falls back to the UI thread.
#[must_use]
pub(crate) fn background_search_unavailable(lang: Language, reason: &str) -> String {
    match lang {
        Language::English => {
            format!("Background search is unavailable ({reason}); searching on the UI thread.")
        }
        Language::Korean => {
            format!("백그라운드 검색을 사용할 수 없어({reason}) UI 스레드에서 검색합니다.")
        }
    }
}

/// Warning notice when Tree view falls back to Pretty because parsing failed.
#[must_use]
pub(crate) fn tree_fallback_error(
    lang: Language,
    format: BodyFormat,
    reason: &FormatError,
) -> Notice {
    match lang {
        Language::English => view::tree_fallback_notice(format, Some(reason)),
        Language::Korean => {
            let detail = domain::format_error(lang, reason);
            Notice::warning(format!(
                "서식 보기로 표시: 트리를 읽을 수 없습니다({detail})."
            ))
        }
    }
}

/// Warning notice when Tree view is unavailable for `format`.
#[must_use]
pub(crate) fn tree_fallback_unsupported(lang: Language, format: BodyFormat) -> Notice {
    match lang {
        Language::English => view::tree_fallback_notice(format, None),
        Language::Korean => {
            let format_name = domain::body_format(lang, format);
            Notice::warning(format!(
                "{format_name} 형식에는 트리 보기를 사용할 수 없어 서식 보기로 표시합니다."
            ))
        }
    }
}

/// Context-menu copy label for a tree node path or value.
#[must_use]
pub(crate) fn tree_copy_label(lang: Language, raw: &'static str) -> &'static str {
    match lang {
        Language::English => raw,
        Language::Korean => match raw {
            "Copy JSONPath" => "JSONPath 복사",
            "Copy XPath" => "XPath 복사",
            "Copy path" => "경로 복사",
            "Copy value" => "값 복사",
            other => other,
        },
    }
}

/// Error when an image exceeds the display's maximum GPU texture dimension.
#[must_use]
pub(crate) fn image_texture_too_large(
    lang: Language,
    width: u32,
    height: u32,
    max_side: usize,
) -> String {
    match lang {
        Language::English => format!(
            "The image is {width}×{height} px, larger than this display's {max_side} px texture limit."
        ),
        Language::Korean => format!(
            "이미지 크기가 {width}×{height} px로 이 디스플레이의 최대 텍스처 한도({max_side} px)를 초과합니다."
        ),
    }
}

/// Notice when the declared image `Content-Type` disagrees with the decoded header.
#[must_use]
pub(crate) fn image_mismatch(
    lang: Language,
    declared: ImageFormat,
    format: ImageFormat,
) -> Option<String> {
    match lang {
        Language::English => image_file::mismatch_text(declared, format),
        Language::Korean => (declared != format).then(|| {
            format!(
                "Content-Type은 {}(으)로 선언되었지만 본문 시작은 {} 형식이어서 {}(으)로 표시합니다.",
                declared.name(),
                format.name(),
                format.name(),
            )
        }),
    }
}

/// Header summary for the SVG text source tab.
#[must_use]
pub(crate) fn svg_source_summary(
    lang: Language,
    file_bytes: usize,
    source_limit_bytes: usize,
) -> String {
    if file_bytes > source_limit_bytes {
        match lang {
            Language::English => format!(
                "First {} KiB of {file_bytes} bytes of SVG source, shown as text and never run.",
                source_limit_bytes / 1024,
            ),
            Language::Korean => format!(
                "SVG 소스 {file_bytes}바이트 중 처음 {} KiB를 텍스트로 표시합니다(실행하지 않음).",
                source_limit_bytes / 1024,
            ),
        }
    } else {
        match lang {
            Language::English => {
                format!("{file_bytes} bytes of SVG source, shown as text and never run.")
            }
            Language::Korean => {
                format!("SVG 소스 {file_bytes}바이트를 텍스트로 표시합니다(실행하지 않음).")
            }
        }
    }
}

/// Image format, pixel dimensions, and byte size summary line.
#[must_use]
pub(crate) fn image_meta_summary(
    lang: Language,
    format_name: &str,
    width: u32,
    height: u32,
    file_bytes: usize,
) -> String {
    match lang {
        Language::English => format!("{format_name} · {width}×{height} px · {file_bytes} bytes"),
        Language::Korean => format!("{format_name} · {width}×{height} px · {file_bytes}바이트"),
    }
}

/// Additional compressed wire-size note when `received_bytes` differs from decoded `file_bytes`.
#[must_use]
pub(crate) fn image_received_text(
    lang: Language,
    file_bytes: usize,
    received_bytes: u64,
) -> String {
    match lang {
        Language::English => image_file::received_text(file_bytes, received_bytes),
        Language::Korean => {
            if u64::try_from(file_bytes).unwrap_or(u64::MAX) == received_bytes {
                String::new()
            } else {
                format!(" · Content-Encoding 적용 수신 크기 {received_bytes}바이트")
            }
        }
    }
}

/// Explains why an image body cannot be drawn yet, or `None` when it can be decoded.
#[must_use]
pub(crate) fn image_unavailable_reason(lang: Language, snapshot: &BodySnapshot) -> Option<String> {
    match lang {
        Language::English => image_file::unavailable_reason(snapshot),
        Language::Korean => {
            if snapshot.lifecycle() == BodyLifecycle::Streaming {
                return Some(format!(
                    "본문 수신이 끝나면 이미지를 표시합니다(현재 {}바이트 수신됨).",
                    snapshot.observed_bytes()
                ));
            }
            if matches!(
                snapshot.lifecycle(),
                BodyLifecycle::Interrupted | BodyLifecycle::Cancelled
            ) {
                return Some(
                    "이미지 본문이 중간에 종료되었습니다. 보관된 바이트를 보려면 Hex를 사용하세요."
                        .to_owned(),
                );
            }
            (!snapshot.is_complete()).then(|| {
                format!(
                    "{}바이트 중 {}바이트만 보관되어 이미지를 표시할 수 없습니다. 이미지 미리보기는 응답당 최대 {} MiB, 전체 응답 합계 최대 {} MiB까지 보관합니다. 보관된 바이트는 Hex에서 확인하세요.",
                    snapshot.observed_bytes(),
                    snapshot.bytes().len(),
                    IMAGE_PREVIEW_BYTES / (1024 * 1024),
                    IMAGE_PREVIEW_BUDGET_BYTES / (1024 * 1024),
                )
            })
        }
    }
}

/// Header summary for the raw hex view of an image body.
#[must_use]
pub(crate) fn hex_view_summary(lang: Language, shown_bytes: usize, total_bytes: usize) -> String {
    match lang {
        Language::English => format!(
            "First {shown_bytes} of {total_bytes} kept bytes, as received (before Content-Encoding is removed)."
        ),
        Language::Korean => format!(
            "보관된 {total_bytes}바이트 중 처음 {shown_bytes}바이트(Content-Encoding 제거 전 수신 원본)."
        ),
    }
}

/// Tooltip detailing bodies that could not be fully searched, or `None` if all were covered.
#[must_use]
pub(crate) fn search_coverage_detail(
    lang: Language,
    partial: usize,
    binary: usize,
    undecodable: usize,
    preview_kib: usize,
) -> Option<String> {
    let mut parts = Vec::new();
    if partial > 0 {
        parts.push(match lang {
            Language::English => format!("{partial} only in their first {preview_kib} KiB"),
            Language::Korean => format!("{partial}건은 처음 {preview_kib} KiB만 검색"),
        });
    }
    if binary > 0 {
        parts.push(match lang {
            Language::English => format!("{binary} binary, skipped"),
            Language::Korean => format!("{binary}건은 바이너리여서 건너뜀"),
        });
    }
    if undecodable > 0 {
        parts.push(match lang {
            Language::English => format!("{undecodable} could not be decoded"),
            Language::Korean => format!("{undecodable}건은 디코딩할 수 없음"),
        });
    }
    (!parts.is_empty()).then(|| {
        let joined = parts.join(", ");
        match lang {
            Language::English => format!(
                "Captured previews were searched after decompression ({joined}); the full-session log keeps the rest."
            ),
            Language::Korean => format!(
                "압축 해제 후 캡처된 미리보기에서 검색했으며({joined}), 나머지는 전체 세션 로그에 보관됩니다."
            ),
        }
    })
}
