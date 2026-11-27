//! Single-body saving, privacy confirmation, and body store settings UI text catalog.

use std::path::Path;

use http_inspector_exchange::ByteSize;

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct BodySavingText {
        privacy_notice,
        init_no_cache_dir,
        not_stored,
        no_response,
        worker_stopped,
        limit_dir_unavailable,
        limit_invalid_path,
        confirm_heading,
        body_store_limit,
        saving_body,
        decoded,
        as_received,
    }
    tooltips: pub(crate) struct BodySavingTooltips {
        busy_saving,
        not_stored_hint,
        limit_hint,
        decoded_hint,
        as_received_hint,
    }
    en = {
        privacy_notice: "Saved bodies can contain cookies, tokens and personal data. Files are created readable only by you; keep them private.",
        init_no_cache_dir: "Saving bodies is unavailable: no cache directory; check HOME",
        not_stored: "This body was not stored.",
        no_response: "No response was received.",
        worker_stopped: "The save worker stopped unexpectedly",
        limit_dir_unavailable: "settings directory unavailable",
        limit_invalid_path: "invalid settings path",
        confirm_heading: "Save body",
        body_store_limit: "Body store limit",
        saving_body: "Saving body…",
        decoded: "Decoded",
        as_received: "As received",
    }
    en_tooltips = {
        busy_saving: "Another body is being saved.",
        not_stored_hint: "This body was not stored.",
        limit_hint: "Maximum body bytes kept for saving in one session; later bodies are not stored once reached.",
        decoded_hint: "Remove gzip, deflate, br and zstd Content-Encoding so the file holds the actual content.",
        as_received_hint: "Keep the exact bytes that were received, including any Content-Encoding.",
    }
    ko = {
        privacy_notice: "저장된 본문에는 쿠키, 토큰, 개인정보가 포함될 수 있습니다. 파일은 현재 사용자만 읽을 수 있도록 생성됩니다. 안전하게 보관하세요.",
        init_no_cache_dir: "본문 저장을 사용할 수 없습니다: 캐시 폴더를 찾을 수 없습니다(HOME을 확인하세요)",
        not_stored: "이 본문은 저장되지 않았습니다.",
        no_response: "수신된 응답이 없습니다.",
        worker_stopped: "저장 작업자가 예기치 않게 중지되었습니다",
        limit_dir_unavailable: "설정 폴더를 사용할 수 없습니다",
        limit_invalid_path: "유효하지 않은 설정 경로입니다",
        confirm_heading: "본문 저장",
        body_store_limit: "본문 저장소 한도",
        saving_body: "본문 저장 중…",
        decoded: "디코딩됨",
        as_received: "수신 원본 그대로",
    }
    ko_tooltips = {
        busy_saving: "다른 본문을 저장하는 중입니다.",
        not_stored_hint: "이 본문은 저장되지 않았습니다.",
        limit_hint: "한 세션에서 저장용으로 보관할 최대 본문 용량이며, 한도에 도달하면 이후 본문은 저장되지 않습니다.",
        decoded_hint: "gzip, deflate, br, zstd Content-Encoding을 해제해 실제 콘텐츠를 파일에 저장합니다.",
        as_received_hint: "Content-Encoding을 포함해 수신한 바이트 그대로 저장합니다.",
    }
}

/// Error when opening the session body store fails.
#[must_use]
pub(crate) fn init_open_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Saving bodies is unavailable: {detail}"),
        Language::Korean => format!("본문 저장을 사용할 수 없습니다: {detail}"),
    }
}

/// Confirmation warning when the stored body is incomplete.
#[must_use]
pub(crate) fn incomplete_confirmation(lang: Language, reason: &str) -> String {
    match lang {
        Language::English => {
            format!("This body is incomplete. {reason} The file name will end in .partial.")
        }
        Language::Korean => {
            format!("이 본문은 불완전합니다. {reason} 파일 이름 끝에 .partial이 붙습니다.")
        }
    }
}

/// Summary line inside Settings showing current body store disk usage and path.
#[must_use]
pub(crate) fn store_usage_summary(lang: Language, used_bytes: u64, dir: &Path) -> String {
    let used = ByteSize::new(used_bytes);
    let dir = dir.display();
    match lang {
        Language::English => format!("{used} used · stored in {dir}"),
        Language::Korean => format!("{used} 사용 중 · 저장 위치: {dir}"),
    }
}

/// Error when saving the body store limit preference fails.
#[must_use]
pub(crate) fn limit_save_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Could not save the body store limit: {detail}"),
        Language::Korean => format!("본문 저장소 한도를 저장하지 못했습니다: {detail}"),
    }
}

/// Status bar notice after saving a single body succeeds.
#[must_use]
pub(crate) fn saved_body(lang: Language, path: &Path, bytes: u64, complete: bool) -> String {
    let path = path.display();
    let size = ByteSize::new(bytes);
    match (lang, complete) {
        (Language::English, true) => format!("Saved body: {path} ({size})"),
        (Language::English, false) => format!("Saved body (incomplete): {path} ({size})"),
        (Language::Korean, true) => format!("본문 저장됨: {path} ({size})"),
        (Language::Korean, false) => format!("본문 저장됨(불완전): {path} ({size})"),
    }
}

/// Status bar error after saving a single body fails.
#[must_use]
pub(crate) fn save_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Body save failed: {detail}"),
        Language::Korean => format!("본문 저장 실패: {detail}"),
    }
}

/// Warning when the body store encountered a write error.
#[must_use]
pub(crate) fn store_error_warning(lang: Language, error: &str) -> String {
    match lang {
        Language::English => format!("{error}. Later bodies are not stored."),
        Language::Korean => format!("{error}. 이후 본문은 저장되지 않습니다."),
    }
}

/// Warning when the body store reached its configured byte limit.
#[must_use]
pub(crate) fn limit_reached_warning(lang: Language, limit_bytes: u64) -> String {
    let limit = ByteSize::new(limit_bytes);
    match lang {
        Language::English => format!(
            "The body store reached its {limit} limit. Later bodies are not stored; raise the limit in Settings."
        ),
        Language::Korean => format!(
            "본문 저장소가 {limit} 한도에 도달했습니다. 이후 본문은 저장되지 않으므로 설정에서 한도를 높이세요."
        ),
    }
}

/// Warning when free disk space drops below the safety threshold.
#[must_use]
pub(crate) fn low_disk_warning(lang: Language, min_free_bytes: u64) -> String {
    let min_free = ByteSize::new(min_free_bytes);
    match lang {
        Language::English => format!(
            "Free disk space is below {min_free}. Bodies are not stored until space is freed."
        ),
        Language::Korean => format!(
            "디스크 여유 공간이 {min_free} 미만입니다. 공간을 확보할 때까지 본문이 저장되지 않습니다."
        ),
    }
}

/// Summary line inside the Body tab Save menu showing stored bytes and completeness.
#[must_use]
pub(crate) fn stored_summary(
    lang: Language,
    stored_bytes: u64,
    incomplete: Option<&str>,
) -> String {
    let size = ByteSize::new(stored_bytes);
    match (lang, incomplete) {
        (Language::English, None) => format!("{size} stored · complete"),
        (Language::English, Some(reason)) => format!("{size} stored · incomplete: {reason}"),
        (Language::Korean, None) => format!("{size} 저장됨 · 완료"),
        (Language::Korean, Some(reason)) => format!("{size} 저장됨 · 불완전: {reason}"),
    }
}
