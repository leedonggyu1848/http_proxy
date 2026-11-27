//! Capture scope help popover UI text catalog.

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct ScopeHelpText {
        capture_scope,
        recorded,
        not_recorded,
        recorded_http,
        recorded_https_off,
        recorded_https_on,
        not_recorded_ignore_proxy,
        not_recorded_no_proxy,
        not_recorded_http3,
        not_recorded_vpn,
        quic_command,
    }
    tooltips: pub(crate) struct ScopeHelpTooltips {
        what_is_captured,
    }
    en = {
        capture_scope: "Capture scope",
        recorded: "Recorded",
        not_recorded: "Not recorded",
        recorded_http: "HTTP: method, URL, headers, bodies",
        recorded_https_off: "HTTPS, Decrypt off: one Tunnel row per CONNECT",
        recorded_https_on: "HTTPS, Decrypt on: each HTTP/1.1 request",
        not_recorded_ignore_proxy: "Apps that ignore proxy settings",
        not_recorded_no_proxy: "NO_PROXY and bypass-list hosts",
        not_recorded_http3: "HTTP/3 (QUIC) over UDP",
        not_recorded_vpn: "Traffic through a VPN or another proxy",
        quic_command: "Chrome without QUIC",
    }
    en_tooltips = {
        what_is_captured: "What is captured?",
    }
    ko = {
        capture_scope: "캡처 범위",
        recorded: "기록되는 항목",
        not_recorded: "기록되지 않는 항목",
        recorded_http: "HTTP: 메서드, URL, 헤더, 본문",
        recorded_https_off: "HTTPS(복호화 끔): CONNECT당 터널 행 1개",
        recorded_https_on: "HTTPS(복호화 켬): 각 HTTP/1.1 요청",
        not_recorded_ignore_proxy: "프록시 설정을 무시하는 앱",
        not_recorded_no_proxy: "NO_PROXY 및 우회 목록 호스트",
        not_recorded_http3: "UDP 기반 HTTP/3 (QUIC)",
        not_recorded_vpn: "VPN 또는 다른 프록시 경유 트래픽",
        quic_command: "QUIC 비활성화 Chrome 실행 명령",
    }
    ko_tooltips = {
        what_is_captured: "무엇이 캡처되나요?",
    }
}

/// Bullet items listed under "Recorded".
#[must_use]
pub(crate) const fn recorded_lines(lang: Language) -> [&'static str; 3] {
    let text = lang.scope_help();
    [
        text.recorded_http,
        text.recorded_https_off,
        text.recorded_https_on,
    ]
}

/// Bullet items listed under "Not recorded".
#[must_use]
pub(crate) const fn not_recorded_lines(lang: Language) -> [&'static str; 4] {
    let text = lang.scope_help();
    [
        text.not_recorded_ignore_proxy,
        text.not_recorded_no_proxy,
        text.not_recorded_http3,
        text.not_recorded_vpn,
    ]
}
