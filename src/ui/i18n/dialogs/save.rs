//! Save menu and export status UI text catalog.

use std::path::Path;

use crate::ui::i18n::Language;

/// Sentinel message produced by `SaveControls::directory` when the configured directory does not exist.
pub(crate) const MISSING_SAVE_DIR_MSG: &str = "Choose an existing save directory in the Save menu";

define_catalog! {
    pub(crate) struct SaveText {
        missing_directory,
        worker_stopped,
        save_directory,
        existing_directory_hint,
        save_window_png,
        save_all_logs,
        export_har,
        save_bodies,
    }
    tooltips: pub(crate) struct SaveTooltips {
        save_window_png,
        save_all_logs,
        export_har,
        save_bodies,
    }
    en = {
        missing_directory: MISSING_SAVE_DIR_MSG,
        worker_stopped: "The save worker stopped unexpectedly",
        save_directory: "Save directory",
        existing_directory_hint: "Existing directory",
        save_window_png: "Save window PNG",
        save_all_logs: "Save all logs",
        export_har: "Export HAR…",
        save_bodies: "Save bodies…",
    }
    en_tooltips = {
        save_window_png: "Save the visible app content, without the OS window border.",
        save_all_logs: "Save all captured records and full bodies, regardless of the filter.",
        export_har: "Save the listed requests as a HAR 1.2 file, without bodies or sensitive headers unless you include them.",
        save_bodies: "Save the request and response bodies of many requests into a new folder with a manifest.json.",
    }
    ko = {
        missing_directory: "저장 메뉴에서 존재하는 저장 폴더를 선택하세요",
        worker_stopped: "저장 작업자가 예기치 않게 중지되었습니다",
        save_directory: "저장 폴더",
        existing_directory_hint: "존재하는 폴더 경로",
        save_window_png: "창 화면 PNG 저장",
        save_all_logs: "전체 로그 저장",
        export_har: "HAR 내보내기…",
        save_bodies: "본문 일괄 저장…",
    }
    ko_tooltips = {
        save_window_png: "OS 창 테두리를 제외한 현재 앱 화면을 PNG로 저장합니다.",
        save_all_logs: "필터와 관계없이 캡처된 모든 기록과 전체 본문을 저장합니다.",
        export_har: "목록의 요청을 HAR 1.2 파일로 저장하며, 본문과 민감 헤더는 직접 선택하지 않으면 제외됩니다.",
        save_bodies: "여러 요청의 요청·응답 본문을 manifest.json과 함께 새 폴더에 저장합니다.",
    }
}

/// Status bar label after a file save succeeds.
#[must_use]
pub(crate) fn saved_path(lang: Language, path: &Path) -> String {
    match lang {
        Language::English => format!("Saved: {}", path.display()),
        Language::Korean => format!("저장됨: {}", path.display()),
    }
}

/// Status bar label after a file save fails.
#[must_use]
pub(crate) fn save_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Save failed: {detail}"),
        Language::Korean => format!("저장 실패: {detail}"),
    }
}
