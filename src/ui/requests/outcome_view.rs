//! Shared outcome presentation for the request list, details, and search.

use http_inspector_exchange::{Outcome, TunnelState};

use crate::ui::i18n::{Language, requests};
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
    (
        requests::outcome_short_text(lang, outcome),
        outcome_tone(outcome),
    )
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
    requests::outcome_status_class(lang, code)
}

/// Status text with the response class appended, for tooltips and screen
/// readers. Other outcomes already name their state in the text.
pub(super) fn outcome_summary(outcome: &Outcome, lang: Language) -> String {
    let text = requests::outcome_short_text(lang, outcome);
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

/// Status text of the inspector's result line and the tone of an
/// exceptional outcome. A response shows the reason phrase as the server
/// sent it and the HTTP version it answered with; other outcomes show the
/// same text as the request list.
pub(crate) fn detail_status(outcome: &Outcome, lang: Language) -> (String, Option<Tone>) {
    (
        requests::outcome_detail_text(lang, outcome),
        outcome_tone(outcome),
    )
}

pub(crate) const fn timing_label(outcome: &Outcome, lang: Language) -> &'static str {
    requests::outcome_timing_label(lang, outcome)
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
            (
                "Upstream TLS handshake: certificate rejected".to_owned(),
                Some(Tone::Danger)
            )
        );
        assert_eq!(
            outcome_text(&outcome, Language::English),
            detail_status(&outcome, Language::English)
        );
        assert_eq!(
            outcome_text(&outcome, Language::Korean),
            detail_status(&outcome, Language::Korean)
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

    #[test]
    fn inspector_shows_the_reason_and_version_the_server_sent()
    -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode, Version};
        use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

        let answered = |fixture: ResponseFixture| {
            ExchangeBuilder::http(Method::GET, "http://localhost:8080/api")?.respond(fixture)
        };
        let gateway = answered(
            ResponseFixture::new(StatusCode::BAD_GATEWAY).reason("Upstream Took Too Long"),
        )?;
        assert_eq!(
            detail_status(gateway.outcome(), Language::English),
            (
                "502 Upstream Took Too Long · HTTP/1.1".to_owned(),
                Some(Tone::Danger)
            ),
            "a nonstandard reason phrase is shown as sent"
        );
        let custom =
            answered(ResponseFixture::new(StatusCode::from_u16(599)?).reason("Net Timeout"))?;
        assert_eq!(
            detail_status(custom.outcome(), Language::English).0,
            "599 Net Timeout · HTTP/1.1",
            "a nonstandard code keeps its phrase instead of `Unknown status`"
        );
        let old = answered(ResponseFixture::new(StatusCode::OK).version(Version::HTTP_10))?;
        assert_eq!(
            detail_status(old.outcome(), Language::English),
            ("200 OK · HTTP/1.0".to_owned(), None)
        );
        // Other outcomes keep the request list's text.
        assert_eq!(
            detail_status(&Outcome::Cancelled, Language::English),
            outcome_text(&Outcome::Cancelled, Language::English)
        );
        assert_eq!(
            detail_status(&Outcome::Cancelled, Language::Korean),
            outcome_text(&Outcome::Cancelled, Language::Korean)
        );
        Ok(())
    }
}
