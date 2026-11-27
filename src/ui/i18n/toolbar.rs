//! Toolbar and status bar UI text catalog.

use super::Language;

define_catalog! {
    pub(crate) struct ToolbarText {
        view_switch,
        map,
        stopped,
        port,
        start,
        stop,
        https_on,
        https_off,
        decrypt_https,
        settings,
        copy_proxy_url,
        copy_curl_example,
        copy_http_proxy_env,
        missing_ca_directory,
        authorization_warning,
        range_merge_saving_unavailable,
        range_merge_worker_stopped,
        searching,
        search_on_ui_thread,
        url_map_limit,
        full_log_unavailable,
        log_body_limit,
    }
    tooltips: pub(crate) struct ToolbarTooltips {
        port_hint,
        https_stop_to_change,
        https_decrypt_new,
        dropped_hint,
        domain_limit_hint,
    }
    en = {
        view_switch: "View",
        map: "Map",
        stopped: "Stopped",
        port: "Port",
        start: "Start",
        stop: "Stop",
        https_on: "HTTPS on",
        https_off: "HTTPS off",
        decrypt_https: "Decrypt HTTPS",
        settings: "Settings",
        copy_proxy_url: "Proxy URL",
        copy_curl_example: "curl example",
        copy_http_proxy_env: "HTTP_PROXY variables",
        missing_ca_directory: "HTTPS inspection requires an application data directory; check HOME and platform storage settings",
        authorization_warning: "Only enable inspection for traffic that you are authorized to inspect.",
        range_merge_saving_unavailable: "Saving bodies is unavailable",
        range_merge_worker_stopped: "The range planning worker stopped unexpectedly",
        searching: "Searching…",
        search_on_ui_thread: "Search on UI thread",
        url_map_limit: "URL map limit",
        full_log_unavailable: "Full log unavailable",
        log_body_limit: "Log body limit",
    }
    en_tooltips = {
        port_hint: "Loopback port to listen on; 0 picks a free port.",
        https_stop_to_change: "Stop the proxy to change this setting.",
        https_decrypt_new: "Decrypt HTTPS in new connections with the app CA.",
        dropped_hint: "The capture queue was full, so these exchanges were forwarded but not shown.",
        domain_limit_hint: "Raise the domain limit in the graph header to show more.",
    }
    ko = {
        view_switch: "보기",
        map: "맵",
        stopped: "중지됨",
        port: "포트",
        start: "시작",
        stop: "중지",
        https_on: "HTTPS 켬",
        https_off: "HTTPS 끔",
        decrypt_https: "HTTPS 복호화",
        settings: "설정",
        copy_proxy_url: "프록시 URL",
        copy_curl_example: "curl 예시",
        copy_http_proxy_env: "HTTP_PROXY 환경 변수",
        missing_ca_directory: "HTTPS 복호화를 사용하려면 앱 데이터 폴더가 필요합니다. HOME 및 플랫폼 저장소 설정을 확인하세요",
        authorization_warning: "HTTPS 복호화는 검사 권한이 있는 트래픽에만 켜세요.",
        range_merge_saving_unavailable: "본문 저장을 사용할 수 없습니다",
        range_merge_worker_stopped: "범위 병합 계획 작업자가 예기치 않게 중지되었습니다",
        searching: "검색 중…",
        search_on_ui_thread: "UI 스레드 검색",
        url_map_limit: "URL 맵 한도",
        full_log_unavailable: "전체 로그 사용 불가",
        log_body_limit: "로그 본문 한도",
    }
    ko_tooltips = {
        port_hint: "프록시가 수신할 루프백 포트이며 0을 입력하면 빈 포트를 자동으로 선택합니다.",
        https_stop_to_change: "이 설정을 바꾸려면 프록시를 중지하세요.",
        https_decrypt_new: "앱 CA로 새 연결의 HTTPS를 복호화합니다.",
        dropped_hint: "캡처 대기열이 가득 차 해당 요청은 전달되었지만 화면에는 표시되지 않았습니다.",
        domain_limit_hint: "더 많은 도메인을 보려면 그래프 머리줄에서 도메인 한도를 높이세요.",
    }
}

/// Proxy stopped unexpectedly message.
#[must_use]
pub(crate) fn proxy_stopped(lang: Language, message: &str) -> String {
    match lang {
        Language::English => format!("The proxy stopped: {message}"),
        Language::Korean => format!("프록시가 중지되었습니다: {message}"),
    }
}

/// Session error banner for a failed range merge.
#[must_use]
pub(crate) fn range_merge_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("Merging ranges failed: {detail}"),
        Language::Korean => format!("범위 응답 병합 실패: {detail}"),
    }
}

/// Session error banner for a failed HLS download.
#[must_use]
pub(crate) fn hls_download_failed(lang: Language, detail: &str) -> String {
    match lang {
        Language::English => format!("HLS download failed: {detail}"),
        Language::Korean => format!("HLS 다운로드 실패: {detail}"),
    }
}

/// Request count in the status bar, or how many of them the filters show.
#[must_use]
pub(crate) fn request_count(lang: Language, shown: usize, total: usize) -> String {
    match lang {
        Language::English => {
            if shown == total {
                format!("{total} requests")
            } else {
                format!("{shown} of {total} requests")
            }
        }
        Language::Korean => {
            if shown == total {
                format!("요청 {total}건")
            } else {
                format!("요청 {shown}/{total}건")
            }
        }
    }
}

/// Failed request count in the status bar.
#[must_use]
pub(crate) fn failure_count(lang: Language, failed: usize) -> String {
    match lang {
        Language::English => format!("{failed} failed"),
        Language::Korean => format!("실패 {failed}건"),
    }
}

/// Host and node counts of the URL map in the status bar.
#[must_use]
pub(crate) fn map_count(lang: Language, hosts: usize, nodes: usize) -> String {
    match lang {
        Language::English => format!("{hosts} hosts · {nodes} nodes"),
        Language::Korean => format!("호스트 {hosts} · 노드 {nodes}"),
    }
}

/// Search progress in the status bar.
#[must_use]
pub(crate) fn search_progress(lang: Language, checked: usize, total: usize) -> String {
    format!("{} {checked}/{total}", lang.toolbar().searching)
}

/// Tooltip of the removed-records entry of the status bar.
#[must_use]
pub(crate) fn older_removed_hint(lang: Language, kept: usize) -> String {
    match lang {
        Language::English => format!(
            "Only the newest {kept} requests stay on screen; the full-session log keeps every record."
        ),
        Language::Korean => {
            format!("화면에는 최근 {kept}건만 유지되며 전체 세션 로그에는 모든 기록이 남습니다.")
        }
    }
}

/// Tooltip of the URL map limit entry of the status bar.
#[must_use]
pub(crate) fn url_map_limit_hint(lang: Language, nodes: usize) -> String {
    match lang {
        Language::English => {
            format!("The URL map reached {nodes} nodes; deeper paths are folded into parent nodes.")
        }
        Language::Korean => {
            format!("URL 맵이 {nodes}개 노드 한도에 도달해 하위 경로는 상위 노드에 병합됩니다.")
        }
    }
}

/// Tooltip of the log body limit entry of the status bar.
#[must_use]
pub(crate) fn log_body_limit_hint(lang: Language, limit: &str) -> String {
    match lang {
        Language::English => format!(
            "Body bytes after {limit} are left out of the full-session log; headers and results are still logged."
        ),
        Language::Korean => format!(
            "{limit}를 넘는 본문 바이트는 전체 세션 로그에서 생략되며 헤더와 결과는 계속 기록됩니다."
        ),
    }
}

/// Bodies the search could read only in part.
#[must_use]
pub(crate) fn partly_searched(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("{count} bodies partly searched"),
        Language::Korean => format!("본문 {count}건 일부만 검색됨"),
    }
}

/// Exchanges the capture queue could not keep.
#[must_use]
pub(crate) fn dropped(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} dropped"),
        Language::Korean => format!("누락 {count}건"),
    }
}

/// Requests removed from the list but still in the counts.
#[must_use]
pub(crate) fn older_removed(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} older removed"),
        Language::Korean => format!("오래된 기록 {count}건 정리됨"),
    }
}

/// Domains and edges the graph shows, in the status bar.
#[must_use]
pub(crate) fn graph_count(
    lang: Language,
    shown: usize,
    total: usize,
    visible_edges: usize,
    total_edges: usize,
) -> String {
    match lang {
        Language::English => {
            let domains = if total > shown {
                format!("top {shown} of {total} domains")
            } else {
                format!("{shown} domains")
            };
            let edges = if visible_edges == total_edges {
                format!("{total_edges} edges")
            } else {
                format!("{visible_edges}/{total_edges} edges")
            };
            format!("{domains} · {edges}")
        }
        Language::Korean => {
            let domains = if total > shown {
                format!("상위 도메인 {shown}/{total}")
            } else {
                format!("도메인 {shown}")
            };
            let edges = if visible_edges == total_edges {
                format!("연결선 {total_edges}")
            } else {
                format!("연결선 {visible_edges}/{total_edges}")
            };
            format!("{domains} · {edges}")
        }
    }
}
