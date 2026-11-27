//! Request list, filter bar, outcome labels, and waterfall timing UI text catalog.

use std::net::SocketAddr;

use http_inspector_analysis::outcome;
use http_inspector_exchange::{Outcome, TunnelPreview, TunnelState};

use super::{Language, domain};

define_catalog! {
    pub(crate) struct RequestsText {
        col_method,
        col_path,
        col_type,
        col_size,
        col_duration,
        col_waterfall,
        follow_newest,
        arrival_order,
        save_response_body,
        save_request_body,
        request_outcome,
        map_filter,
        clear_map_filter,
        no_requests_match,
        no_requests_stopped,
        filter_by_value,
        exclude_value,
        start_time,
        connect,
        send,
        wait,
        receive,
        total,
    }
    tooltips: pub(crate) struct RequestsTooltips {
        clear_list,
        estimate_hint,
        field_filter_needs_text,
        follow_newest,
        shared_host,
        save_body,
        save_body_disabled,
    }
    en = {
        col_method: "Method",
        col_path: "Path",
        col_type: "Type",
        col_size: "Size",
        col_duration: "Duration",
        col_waterfall: "Waterfall",
        follow_newest: "Follow newest",
        arrival_order: "Arrival order",
        save_response_body: "Save response body",
        save_request_body: "Save request body",
        request_outcome: "Request outcome",
        map_filter: "Map filter",
        clear_map_filter: "Clear map filter",
        no_requests_match: "No requests match the filter",
        no_requests_stopped: "No requests · proxy stopped",
        filter_by_value: "Filter by value",
        exclude_value: "Exclude value",
        start_time: "Started",
        connect: "Connect",
        send: "Send",
        wait: "Wait (TTFB)",
        receive: "Receive",
        total: "Total",
    }
    en_tooltips = {
        clear_list: "Clear the listed requests and the URL map; the full-session log is kept.",
        estimate_hint: "Estimate based on requests included in endpoint analysis.",
        field_filter_needs_text: "Turn off regex mode to use field filters",
        follow_newest: "Keep the newest request in view while sorted by arrival.",
        shared_host: "Every request in the list goes to this host",
        save_body: "Save the decoded body to the Save directory.",
        save_body_disabled: "Unavailable while another body is being saved, or when the body was not observed.",
    }
    ko = {
        col_method: "메서드",
        col_path: "경로",
        col_type: "형식",
        col_size: "크기",
        col_duration: "시간",
        col_waterfall: "워터폴",
        follow_newest: "최신 항목 따라가기",
        arrival_order: "도착순",
        save_response_body: "응답 본문 저장",
        save_request_body: "요청 본문 저장",
        request_outcome: "요청 결과",
        map_filter: "맵 필터",
        clear_map_filter: "맵 필터 해제",
        no_requests_match: "필터와 일치하는 요청이 없습니다",
        no_requests_stopped: "요청 없음 · 프록시 중지됨",
        filter_by_value: "이 값으로 필터링",
        exclude_value: "이 값 제외",
        start_time: "시작 시각",
        connect: "연결",
        send: "전송",
        wait: "대기 (TTFB)",
        receive: "수신",
        total: "전체",
    }
    ko_tooltips = {
        clear_list: "전체 세션 로그는 유지한 채 표시된 요청 목록과 URL 맵을 비웁니다.",
        estimate_hint: "엔드포인트 분석에 포함된 요청을 기준으로 추정한 값입니다.",
        field_filter_needs_text: "필드 필터를 쓰려면 정규식 모드를 끄세요",
        follow_newest: "도착순으로 정렬된 동안 가장 최근 요청이 보이도록 유지합니다.",
        shared_host: "목록의 모든 요청이 이 호스트로 전송되었습니다",
        save_body: "디코딩된 본문을 저장 폴더에 저장합니다.",
        save_body_disabled: "다른 본문을 저장 중이거나 관찰된 본문이 없을 때는 사용할 수 없습니다.",
    }
}

/// Retention count next to the active URL map filter chip.
#[must_use]
pub(crate) fn map_retention_label(
    lang: Language,
    retained: usize,
    evicted: u64,
    approximate: bool,
) -> String {
    match lang {
        Language::English => {
            if approximate {
                format!("({retained} retained · ≈{evicted} older removed)")
            } else if evicted > 0 {
                format!("({retained} retained · {evicted} older removed)")
            } else {
                format!("({retained} matching)")
            }
        }
        Language::Korean => {
            if approximate {
                format!("({retained}건 유지 · 오래된 기록 ≈{evicted}건 정리됨)")
            } else if evicted > 0 {
                format!("({retained}건 유지 · 오래된 기록 {evicted}건 정리됨)")
            } else {
                format!("({retained}건 일치)")
            }
        }
    }
}

/// Empty-list prompt while the proxy is listening on `address`.
#[must_use]
pub(crate) fn empty_list_running(lang: Language, address: SocketAddr) -> String {
    match lang {
        Language::English => format!("No requests yet · proxy http://{address}"),
        Language::Korean => format!("요청 없음 · 프록시 http://{address}"),
    }
}

/// Compact replay marker shown ahead of the target path in the request list.
#[must_use]
pub(crate) fn replay_prefix(lang: Language, original: u64) -> String {
    match lang {
        Language::English => format!("Replay of #{original} ·"),
        Language::Korean => format!("재전송 #{original} ·"),
    }
}

/// Short status text for one outcome, independent of the theme.
#[must_use]
pub(crate) fn outcome_short_text(lang: Language, outcome: &Outcome) -> String {
    match lang {
        Language::English => outcome::short_text(outcome),
        Language::Korean => match outcome {
            Outcome::InFlight => "대기 중".to_owned(),
            Outcome::Response(response) => {
                let status = response.status();
                format!(
                    "{} {}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("알 수 없는 상태")
                )
            }
            Outcome::Tunnel(tunnel) => tunnel_summary_ko(tunnel),
            Outcome::InspectionEstablished => "HTTPS".to_owned(),
            Outcome::Failed { stage, message } => {
                format!("{}: {message}", domain::failure_stage(lang, *stage))
            }
            Outcome::Cancelled => "클라이언트 취소".to_owned(),
            Outcome::Incomplete => "불완전 (프록시 중지)".to_owned(),
        },
    }
}

fn tunnel_summary_ko(tunnel: &TunnelPreview) -> String {
    match tunnel.snapshot().state() {
        TunnelState::Open => "터널 · 열림".to_owned(),
        TunnelState::Closed => "터널 · 닫힘".to_owned(),
        TunnelState::Failed { stage, .. } => {
            let stage_label = domain::failure_stage(Language::Korean, *stage);
            format!("터널 · 실패 ({stage_label})")
        }
        TunnelState::Incomplete => "터널 · 불완전".to_owned(),
    }
}

/// Plain-language class of an HTTP status code.
#[must_use]
pub(crate) const fn outcome_status_class(lang: Language, code: u16) -> &'static str {
    match lang {
        Language::English => outcome::status_class(code),
        Language::Korean => match code {
            100..=199 => "정보 응답",
            200..=299 => "성공",
            300..=399 => "리다이렉트",
            400..=499 => "클라이언트 오류",
            500..=599 => "서버 오류",
            _ => "비표준 상태",
        },
    }
}

/// Detailed status text shown in the request inspector header.
#[must_use]
pub(crate) fn outcome_detail_text(lang: Language, outcome: &Outcome) -> String {
    match lang {
        Language::English => outcome::detail_text(outcome),
        Language::Korean => match outcome {
            Outcome::Response(response) => outcome::response_line(
                response.status().as_u16(),
                response.reason(),
                &format!("{:?}", response.version()),
            ),
            _ => outcome_short_text(lang, outcome),
        },
    }
}

/// Label for the elapsed time in the detail panel and timing views.
#[must_use]
pub(crate) const fn outcome_timing_label(lang: Language, outcome: &Outcome) -> &'static str {
    match lang {
        Language::English => outcome::timing_label(outcome),
        Language::Korean => match outcome {
            Outcome::InFlight => "대기",
            Outcome::Response(_) => "응답 헤더",
            Outcome::Tunnel(_) | Outcome::InspectionEstablished => "연결됨",
            Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => "종료됨",
        },
    }
}
