//! Shared outcome presentation for the request list, details, and search.

use http_inspector_exchange::{Outcome, TunnelPreview, TunnelState};

use crate::ui::i18n::{Language, domain};
use crate::ui::style::tokens::Tone;

/// Orders responses by status code, then every other outcome after them.
pub(super) const fn status_rank(outcome: &Outcome) -> (u8, u16) {
    match outcome {
        Outcome::Response(response) => (0, response.status().as_u16()),
        Outcome::InFlight => (1, 0),
        Outcome::Tunnel(_) => (2, 0),
        Outcome::InspectionEstablished => (3, 0),
        Outcome::Failed { .. } => (4, 0),
        Outcome::Cancelled => (5, 0),
        Outcome::Incomplete => (6, 0),
    }
}

/// Short status text and the tone of an exceptional outcome.
pub(crate) fn outcome_text(outcome: &Outcome, lang: Language) -> (String, Option<Tone>) {
    (short_text(outcome, lang), outcome_tone(outcome))
}

/// Short status text for one outcome, independent of the theme.
fn short_text(outcome: &Outcome, lang: Language) -> String {
    match outcome {
        Outcome::InFlight => lang.pick("Pending", "대기 중").to_owned(),
        Outcome::Response(response) => {
            let status = response.status();
            format!(
                "{} {}",
                status.as_str(),
                status
                    .canonical_reason()
                    .unwrap_or_else(|| lang.pick("Unknown status", "알 수 없는 상태"))
            )
        }
        Outcome::Tunnel(tunnel) => tunnel_summary(tunnel, lang),
        Outcome::InspectionEstablished => "HTTPS".to_owned(),
        Outcome::Failed { stage, message } => {
            format!("{}: {message}", domain::failure_stage(lang, *stage))
        }
        Outcome::Cancelled => lang
            .pick("Cancelled by client", "클라이언트 취소")
            .to_owned(),
        Outcome::Incomplete => lang
            .pick("Incomplete (proxy stopped)", "불완전 (프록시 중지)")
            .to_owned(),
    }
}

/// Tone of an exceptional outcome: 4xx, cancelled, and incomplete
/// exchanges need attention; 5xx and failed connections or tunnels failed.
/// Normal outcomes (1xx–3xx, pending, open or closed tunnels) have no tone
/// and use the default text color.
pub(crate) fn outcome_tone(outcome: &Outcome) -> Option<Tone> {
    match outcome {
        Outcome::InFlight | Outcome::InspectionEstablished => None,
        Outcome::Response(response) => match response.status().as_u16() {
            400..=499 => Some(Tone::Warning),
            500..=599 => Some(Tone::Danger),
            _ => None,
        },
        Outcome::Tunnel(tunnel) => match tunnel.snapshot().state() {
            TunnelState::Open | TunnelState::Closed => None,
            TunnelState::Failed { .. } => Some(Tone::Danger),
            TunnelState::Incomplete => Some(Tone::Warning),
        },
        Outcome::Failed { .. } => Some(Tone::Danger),
        Outcome::Cancelled | Outcome::Incomplete => Some(Tone::Warning),
    }
}

/// Plain-language class of an HTTP status code, so that success and error do
/// not depend on the status color alone.
pub(crate) const fn status_class(code: u16, lang: Language) -> &'static str {
    match code {
        100..=199 => lang.pick("informational", "정보 응답"),
        200..=299 => lang.pick("success", "성공"),
        300..=399 => lang.pick("redirect", "리다이렉트"),
        400..=499 => lang.pick("client error", "클라이언트 오류"),
        500..=599 => lang.pick("server error", "서버 오류"),
        _ => lang.pick("nonstandard status", "비표준 상태"),
    }
}

/// Status text with the response class appended, for tooltips and screen
/// readers. Other outcomes already name their state in the text.
pub(super) fn outcome_summary(outcome: &Outcome, lang: Language) -> String {
    let text = short_text(outcome, lang);
    match outcome {
        Outcome::Response(response) => {
            format!(
                "{text} ({})",
                status_class(response.status().as_u16(), lang)
            )
        }
        _ => text,
    }
}

fn tunnel_summary(tunnel: &TunnelPreview, lang: Language) -> String {
    match tunnel.snapshot().state() {
        TunnelState::Open => lang.pick("Tunnel · open", "터널 · 열림").to_owned(),
        TunnelState::Closed => lang.pick("Tunnel · closed", "터널 · 닫힘").to_owned(),
        TunnelState::Failed { stage, .. } => {
            let stage_label = domain::failure_stage(lang, *stage);
            match lang {
                Language::English => format!("Tunnel · failed ({stage_label})"),
                Language::Korean => format!("터널 · 실패 ({stage_label})"),
            }
        }
        TunnelState::Incomplete => lang.pick("Tunnel · incomplete", "터널 · 불완전").to_owned(),
    }
}

/// Status sentence for the details panel and the tone of an exceptional
/// outcome.
pub(crate) fn detail_text(outcome: &Outcome, lang: Language) -> (String, Option<Tone>) {
    let text = match outcome {
        Outcome::InFlight => lang.pick(
            "Waiting for the upstream server.",
            "업스트림 서버의 응답을 기다리는 중입니다.",
        ),
        Outcome::InspectionEstablished => lang.pick(
            "HTTPS inspection session established.",
            "HTTPS 복호화 세션이 연결되었습니다.",
        ),
        Outcome::Cancelled => lang.pick(
            "The client closed the connection before a response arrived.",
            "응답이 도착하기 전에 클라이언트가 연결을 닫았습니다.",
        ),
        Outcome::Incomplete => lang.pick(
            "The proxy stopped before a response arrived.",
            "응답이 도착하기 전에 프록시가 중지되었습니다.",
        ),
        _ => return outcome_text(outcome, lang),
    };
    (text.to_owned(), outcome_tone(outcome))
}

pub(crate) const fn timing_label(outcome: &Outcome, lang: Language) -> &'static str {
    match outcome {
        Outcome::InFlight => lang.pick("Waiting", "대기"),
        Outcome::Response(_) => lang.pick("Response headers", "응답 헤더"),
        Outcome::Tunnel(_) | Outcome::InspectionEstablished => lang.pick("Established", "연결됨"),
        Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => {
            lang.pick("Ended", "종료됨")
        }
    }
}

/// Converts a duration to milliseconds for UI timing displays.
pub(crate) fn elapsed_millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_inspector_exchange::FailureStage;

    #[test]
    fn names_every_status_class() {
        assert_eq!(status_class(101, Language::English), "informational");
        assert_eq!(status_class(200, Language::English), "success");
        assert_eq!(status_class(204, Language::English), "success");
        assert_eq!(status_class(304, Language::English), "redirect");
        assert_eq!(status_class(404, Language::English), "client error");
        assert_eq!(status_class(503, Language::English), "server error");
        assert_eq!(status_class(999, Language::English), "nonstandard status");
        assert_eq!(status_class(200, Language::Korean), "성공");
        assert_eq!(status_class(404, Language::Korean), "클라이언트 오류");
    }

    #[test]
    fn only_exceptions_have_a_tone() -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode};
        use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

        for (status, tone) in [
            (200, None),
            (301, None),
            (404, Some(Tone::Warning)),
            (503, Some(Tone::Danger)),
            // Nonstandard codes are not errors.
            (600, None),
        ] {
            let record = ExchangeBuilder::http(Method::GET, "http://example.test/")?
                .respond(ResponseFixture::new(StatusCode::from_u16(status)?))?;
            assert_eq!(outcome_tone(record.outcome()), tone, "{status}");
        }
        let failed = Outcome::Failed {
            stage: FailureStage::Connect,
            message: "refused".to_owned(),
        };
        assert_eq!(outcome_tone(&failed), Some(Tone::Danger));
        assert_eq!(outcome_tone(&Outcome::InFlight), None);
        assert_eq!(outcome_tone(&Outcome::Cancelled), Some(Tone::Warning));
        Ok(())
    }

    #[test]
    fn summary_keeps_non_response_text() {
        assert_eq!(
            outcome_summary(&Outcome::InFlight, Language::English),
            "Pending"
        );
        assert_eq!(
            outcome_summary(&Outcome::Cancelled, Language::English),
            "Cancelled by client"
        );
        assert_eq!(
            outcome_summary(&Outcome::InFlight, Language::Korean),
            "대기 중"
        );
    }

    #[test]
    fn failure_stage_is_shared_by_list_details_and_search() {
        let outcome = Outcome::Failed {
            stage: FailureStage::UpstreamTls,
            message: "certificate rejected".to_owned(),
        };
        assert_eq!(
            outcome_text(&outcome, Language::English),
            detail_text(&outcome, Language::English)
        );
        assert_eq!(
            outcome_text(&outcome, Language::Korean),
            detail_text(&outcome, Language::Korean)
        );
        assert_eq!(
            outcome_text(&outcome, Language::English).1,
            Some(Tone::Danger)
        );
        let matches = |query: &str| {
            http_inspector_analysis::search::Matcher::new(
                query,
                http_inspector_analysis::search::MatchOptions::default(),
            )
            .is_ok_and(|matcher| {
                http_inspector_analysis::search::outcome_matches(&outcome, &matcher)
            })
        };
        assert!(matches("upstream tls handshake"));
        assert!(matches("certificate rejected"));
        assert!(!matches("response timeout"));
    }
}
