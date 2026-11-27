//! HTTPS CA registration panel UI text catalog.

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct CaText {
        restart_browser_notice,
        worker_stopped,
        checking,
        registering,
        removing,
        not_checked,
        error,
        not_registered,
        trusted,
        registered_untrusted,
        storage_unavailable,
        register_ca,
        ca_details,
        public_ca_cert,
        sha256_fingerprint,
        check_registration,
        remove_ca,
    }
    tooltips: pub(crate) struct CaTooltips {
        register_help,
        storage_unavailable_hint,
        badge_status_hint,
        remove_ca_hint,
    }
    en = {
        restart_browser_notice: "Restart Chrome/Chromium after changing CA registration.",
        worker_stopped: "CA registration worker stopped",
        checking: "Checking…",
        registering: "Registering…",
        removing: "Removing…",
        not_checked: "Not checked",
        error: "Error",
        not_registered: "Not registered",
        trusted: "Trusted",
        registered_untrusted: "Registered, not trusted",
        storage_unavailable: "CA storage unavailable",
        register_ca: "Register CA",
        ca_details: "CA details",
        public_ca_cert: "Public CA certificate",
        sha256_fingerprint: "SHA-256 fingerprint",
        check_registration: "Check registration",
        remove_ca: "Remove CA",
    }
    en_tooltips = {
        register_help: "Trust the public CA in Chrome and Chromium for the current user.",
        storage_unavailable_hint: "Check HOME and platform storage settings.",
        badge_status_hint: "CA registration status; click to check again.",
        remove_ca_hint: "Remove the CA that HTTP Inspector registered.",
    }
    ko = {
        restart_browser_notice: "CA 등록을 변경한 뒤에는 Chrome/Chromium을 다시 시작하세요.",
        worker_stopped: "CA 등록 작업자가 예기치 않게 중지되었습니다",
        checking: "확인 중…",
        registering: "등록 중…",
        removing: "제거 중…",
        not_checked: "미확인",
        error: "오류",
        not_registered: "미등록",
        trusted: "신뢰됨",
        registered_untrusted: "등록됨(신뢰 안 됨)",
        storage_unavailable: "CA 저장소를 사용할 수 없음",
        register_ca: "CA 등록",
        ca_details: "CA 정보",
        public_ca_cert: "공개 CA 인증서",
        sha256_fingerprint: "SHA-256 지문",
        check_registration: "등록 상태 확인",
        remove_ca: "CA 제거",
    }
    ko_tooltips = {
        register_help: "현재 사용자의 Chrome 및 Chromium에서 공개 CA를 신뢰하도록 등록합니다.",
        storage_unavailable_hint: "HOME 및 플랫폼 저장소 설정을 확인하세요.",
        badge_status_hint: "CA 등록 상태이며, 클릭하면 다시 확인합니다.",
        remove_ca_hint: "HTTP Inspector가 등록한 CA를 제거합니다.",
    }
}

/// Guidance shown on macOS while the system keychain approval dialog is open.
#[cfg(any(target_os = "macos", test))]
#[must_use]
pub(crate) const fn keychain_help(lang: Language) -> &'static str {
    lang.pick(
        "Approve the macOS keychain dialog to finish; cancelling leaves registration incomplete.",
        "등록을 완료하려면 macOS 키체인 대화상자를 승인해야 하며, 취소하면 등록이 완료되지 않습니다.",
    )
}

/// Error when spawning the CA registration worker fails.
#[must_use]
pub(crate) fn start_failed(lang: Language, error: &std::io::Error) -> String {
    match lang {
        Language::English => format!("Could not start CA registration: {error}"),
        Language::Korean => format!("CA 등록 작업을 시작하지 못했습니다: {error}"),
    }
}

/// Label for the trust store location line inside CA details.
#[must_use]
pub(crate) fn trust_store_label(lang: Language, target: &str) -> String {
    match lang {
        Language::English => format!("Trust store: {target}"),
        Language::Korean => format!("신뢰 저장소: {target}"),
    }
}
