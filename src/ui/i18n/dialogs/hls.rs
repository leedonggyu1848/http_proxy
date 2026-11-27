//! HLS playlist inspection and MPEG-TS segment download dialog UI text catalog.

use http_inspector_exchange::ByteSize;

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct HlsText {
        worker_disconnected,
        heading,
        inspecting_playlist,
        download_ts,
        refresh,
        close,
        media_playlists,
        col_quality,
        col_bitrate,
        col_playlist,
        status_not_captured,
        status_parse_error,
        status_ready,
        status_play_in_chrome,
        status_unsupported,
        status_no_segments,
        segments,
        col_segment,
        col_duration,
        seg_cached,
        seg_queued,
        seg_downloading,
        seg_done,
        seg_cancelled,
        seg_failed,
        seg_missing,
        phase_downloading,
        phase_merging,
        phase_finishing,
        download_cancelled,
    }
    en = {
        worker_disconnected: "HLS inspection worker stopped unexpectedly",
        heading: "HLS media",
        inspecting_playlist: "Inspecting playlist…",
        download_ts: "Download .ts",
        refresh: "Refresh",
        close: "Close",
        media_playlists: "Media playlists",
        col_quality: "Quality",
        col_bitrate: "Bitrate",
        col_playlist: "Playlist",
        status_not_captured: "Not captured",
        status_parse_error: "Parse error",
        status_ready: "Ready",
        status_play_in_chrome: "Play in Chrome",
        status_unsupported: "Unsupported",
        status_no_segments: "No segments",
        segments: "Segments",
        col_segment: "Segment",
        col_duration: "Duration",
        seg_cached: "Cached",
        seg_queued: "Queued",
        seg_downloading: "Downloading",
        seg_done: "Done",
        seg_cancelled: "Cancelled",
        seg_failed: "Failed",
        seg_missing: "Missing",
        phase_downloading: "Downloading",
        phase_merging: "Merging",
        phase_finishing: "Finishing",
        download_cancelled: "Download cancelled. No file was saved.",
    }
    ko = {
        worker_disconnected: "HLS 검사 작업자가 예기치 않게 중지되었습니다",
        heading: "HLS 미디어",
        inspecting_playlist: "재생목록 검사 중…",
        download_ts: ".ts 다운로드",
        refresh: "새로고침",
        close: "닫기",
        media_playlists: "미디어 재생목록",
        col_quality: "화질",
        col_bitrate: "비트레이트",
        col_playlist: "재생목록",
        status_not_captured: "미캡처",
        status_parse_error: "해석 오류",
        status_ready: "준비됨",
        status_play_in_chrome: "Chrome 재생 필요",
        status_unsupported: "미지원",
        status_no_segments: "세그먼트 없음",
        segments: "세그먼트",
        col_segment: "세그먼트",
        col_duration: "길이",
        seg_cached: "캐시됨",
        seg_queued: "대기 중",
        seg_downloading: "받는 중",
        seg_done: "완료",
        seg_cancelled: "취소됨",
        seg_failed: "실패",
        seg_missing: "누락",
        phase_downloading: "다운로드 중",
        phase_merging: "병합 중",
        phase_finishing: "마무리 중",
        download_cancelled: "다운로드가 취소되어 파일이 저장되지 않았습니다.",
    }
}

/// Pre-download summary of total, cached, and missing segments.
#[must_use]
pub(crate) fn planned_segments_summary(
    lang: Language,
    total: usize,
    cached: usize,
    missing: usize,
) -> String {
    match lang {
        Language::English => format!("{total} total · {cached} cached · {missing} missing"),
        Language::Korean => format!("전체 {total} · 캐시 {cached} · 누락 {missing}"),
    }
}

/// Segment counts used to format the live or finished HLS segment summary.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LiveSegmentCounts {
    pub(crate) total: usize,
    pub(crate) cached: usize,
    pub(crate) fetched: usize,
    pub(crate) failed: usize,
    pub(crate) cancelled: usize,
    pub(crate) pending: usize,
}

/// Live or finished summary of segment counts during/after download.
#[must_use]
pub(crate) fn live_segments_summary(lang: Language, counts: LiveSegmentCounts) -> String {
    let mut parts = vec![match lang {
        Language::English => format!("{} total", counts.total),
        Language::Korean => format!("전체 {}", counts.total),
    }];
    if counts.cached > 0 {
        parts.push(match lang {
            Language::English => format!("{} cached", counts.cached),
            Language::Korean => format!("캐시 {}", counts.cached),
        });
    }
    if counts.fetched > 0 {
        parts.push(match lang {
            Language::English => format!("{} fetched", counts.fetched),
            Language::Korean => format!("수신 {}", counts.fetched),
        });
    }
    if counts.failed > 0 {
        parts.push(match lang {
            Language::English => format!("{} failed", counts.failed),
            Language::Korean => format!("실패 {}", counts.failed),
        });
    }
    if counts.cancelled > 0 {
        parts.push(match lang {
            Language::English => format!("{} cancelled", counts.cancelled),
            Language::Korean => format!("취소 {}", counts.cancelled),
        });
    }
    if counts.pending > 0 {
        parts.push(match lang {
            Language::English => format!("{} remaining", counts.pending),
            Language::Korean => format!("대기 {}", counts.pending),
        });
    }
    parts.join(" · ")
}

/// Completion line after saving the merged `.ts` file.
#[must_use]
pub(crate) fn saved_file(lang: Language, file_name: &str, total_bytes: u64) -> String {
    let size = ByteSize::new(total_bytes);
    match lang {
        Language::English => format!("Saved {file_name} ({size})"),
        Language::Korean => format!("{file_name} 저장됨 ({size})"),
    }
}
