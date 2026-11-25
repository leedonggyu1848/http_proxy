//! Shared outcome presentation for the request list, details, and search.

use eframe::egui::Color32;
use http_inspector_exchange::{Outcome, TunnelPreview, TunnelState};

use crate::ui::style::theme::Palette;
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

/// Short status text and color for one outcome.
pub(crate) fn outcome_text(outcome: &Outcome, palette: &Palette) -> (String, Color32) {
    (short_text(outcome), outcome_color(outcome, palette))
}

/// Short status text for one outcome, independent of the theme.
fn short_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::InFlight => "Pending".to_owned(),
        Outcome::Response(response) => {
            let status = response.status();
            format!(
                "{} {}",
                status.as_str(),
                status.canonical_reason().unwrap_or("Unknown status")
            )
        }
        Outcome::Tunnel(tunnel) => tunnel_summary(tunnel),
        Outcome::InspectionEstablished => "HTTPS".to_owned(),
        Outcome::Failed { stage, message } => format!("{stage}: {message}"),
        Outcome::Cancelled => "Cancelled by client".to_owned(),
        Outcome::Incomplete => "Incomplete (proxy stopped)".to_owned(),
    }
}

/// Status color for one outcome in the current theme's palette.
fn outcome_color(outcome: &Outcome, palette: &Palette) -> Color32 {
    match outcome_tone(outcome) {
        Tone::Neutral => palette.muted,
        Tone::Info => palette.info,
        Tone::Success => palette.success,
        Tone::Warning => palette.warning,
        Tone::Danger => palette.error,
    }
}

/// Meaning of one outcome: 2xx success, 1xx and 3xx informational, 4xx a
/// warning, 5xx and failures danger, and pending or open tunnels neutral.
pub(crate) fn outcome_tone(outcome: &Outcome) -> Tone {
    match outcome {
        Outcome::InFlight | Outcome::InspectionEstablished => Tone::Neutral,
        Outcome::Response(response) => match response.status().as_u16() {
            200..=299 => Tone::Success,
            400..=499 => Tone::Warning,
            500..=599 => Tone::Danger,
            _ => Tone::Info,
        },
        Outcome::Tunnel(tunnel) => match tunnel.snapshot().state() {
            TunnelState::Open | TunnelState::Closed => Tone::Neutral,
            TunnelState::Failed { .. } => Tone::Danger,
            TunnelState::Incomplete => Tone::Warning,
        },
        Outcome::Failed { .. } => Tone::Danger,
        Outcome::Cancelled | Outcome::Incomplete => Tone::Warning,
    }
}

/// Tone of the edge mark that makes a request stand out in the list: client
/// errors, server errors, and failed connections or tunnels.
pub(super) fn alert_tone(outcome: &Outcome) -> Option<Tone> {
    match outcome {
        Outcome::Response(response) if response.status().as_u16() >= 400 => {
            Some(outcome_tone(outcome))
        }
        Outcome::Failed { .. } => Some(Tone::Danger),
        Outcome::Tunnel(tunnel)
            if matches!(tunnel.snapshot().state(), TunnelState::Failed { .. }) =>
        {
            Some(Tone::Danger)
        }
        _ => None,
    }
}

/// Plain-language class of an HTTP status code, so that success and error do
/// not depend on the status color alone.
pub(crate) const fn status_class(code: u16) -> &'static str {
    match code {
        100..=199 => "informational",
        200..=299 => "success",
        300..=399 => "redirect",
        400..=499 => "client error",
        500..=599 => "server error",
        _ => "nonstandard status",
    }
}

/// Status text with the response class appended, for tooltips and screen
/// readers. Other outcomes already name their state in the text.
pub(super) fn outcome_summary(outcome: &Outcome) -> String {
    let text = short_text(outcome);
    match outcome {
        Outcome::Response(response) => {
            format!("{text} ({})", status_class(response.status().as_u16()))
        }
        _ => text,
    }
}

fn tunnel_summary(tunnel: &TunnelPreview) -> String {
    match tunnel.snapshot().state() {
        TunnelState::Open => "Tunnel · open".to_owned(),
        TunnelState::Closed => "Tunnel · closed".to_owned(),
        TunnelState::Failed { stage, .. } => format!("Tunnel · failed ({stage})"),
        TunnelState::Incomplete => "Tunnel · incomplete".to_owned(),
    }
}

pub(crate) fn detail_text(outcome: &Outcome, palette: &Palette) -> (String, Color32) {
    let text = match outcome {
        Outcome::InFlight => "Waiting for the upstream server.",
        Outcome::InspectionEstablished => "HTTPS inspection session established.",
        Outcome::Cancelled => "The client closed the connection before a response arrived.",
        Outcome::Incomplete => "The proxy stopped before a response arrived.",
        _ => return outcome_text(outcome, palette),
    };
    (text.to_owned(), outcome_text(outcome, palette).1)
}

pub(crate) const fn timing_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "Waiting",
        Outcome::Response(_) => "Response headers",
        Outcome::Tunnel(_) | Outcome::InspectionEstablished => "Established",
        Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => "Ended",
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
        assert_eq!(status_class(101), "informational");
        assert_eq!(status_class(200), "success");
        assert_eq!(status_class(204), "success");
        assert_eq!(status_class(304), "redirect");
        assert_eq!(status_class(404), "client error");
        assert_eq!(status_class(503), "server error");
        assert_eq!(status_class(999), "nonstandard status");
    }

    #[test]
    fn tones_follow_status_class_and_marks_only_errors() -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode};
        use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

        for (status, tone, alert) in [
            (200, Tone::Success, None),
            (301, Tone::Info, None),
            (404, Tone::Warning, Some(Tone::Warning)),
            (503, Tone::Danger, Some(Tone::Danger)),
        ] {
            let record = ExchangeBuilder::http(Method::GET, "http://example.test/")?
                .respond(ResponseFixture::new(StatusCode::from_u16(status)?))?;
            assert_eq!(outcome_tone(record.outcome()), tone, "{status}");
            assert_eq!(alert_tone(record.outcome()), alert, "{status}");
        }
        let failed = Outcome::Failed {
            stage: FailureStage::Connect,
            message: "refused".to_owned(),
        };
        assert_eq!(alert_tone(&failed), Some(Tone::Danger));
        assert_eq!(outcome_tone(&Outcome::InFlight), Tone::Neutral);
        assert_eq!(alert_tone(&Outcome::InFlight), None);
        assert_eq!(alert_tone(&Outcome::Cancelled), None);
        Ok(())
    }

    #[test]
    fn summary_keeps_non_response_text() {
        assert_eq!(outcome_summary(&Outcome::InFlight), "Pending");
        assert_eq!(outcome_summary(&Outcome::Cancelled), "Cancelled by client");
    }

    #[test]
    fn failure_stage_is_shared_by_list_details_and_search() {
        let outcome = Outcome::Failed {
            stage: FailureStage::UpstreamTls,
            message: "certificate rejected".to_owned(),
        };
        for theme in [
            crate::ui::style::theme::Theme::Light,
            crate::ui::style::theme::Theme::Dark,
        ] {
            let palette = Palette::for_theme(theme);
            assert_eq!(
                outcome_text(&outcome, &palette),
                detail_text(&outcome, &palette)
            );
            assert_eq!(outcome_text(&outcome, &palette).1, palette.error);
        }
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
