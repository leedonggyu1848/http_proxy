//! Shared outcome presentation for the request list, details, and search.

use eframe::egui::Color32;
use http_proxy::text_search::Matcher;
use http_proxy::{Outcome, TunnelPreview, TunnelState};

use crate::ui::theme::Palette;

/// Orders responses by status code, then every other outcome after them.
pub(in crate::ui) const fn status_rank(outcome: &Outcome) -> (u8, u16) {
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
pub(in crate::ui) fn outcome_text(outcome: &Outcome, palette: &Palette) -> (String, Color32) {
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
    match outcome {
        Outcome::InFlight => palette.muted,
        Outcome::Response(response) if response.status().as_u16() < 400 => palette.success,
        Outcome::Response(_) | Outcome::Failed { .. } => palette.error,
        Outcome::Tunnel(tunnel) => match tunnel.snapshot().state() {
            TunnelState::Open | TunnelState::Closed => palette.info,
            TunnelState::Failed { .. } => palette.error,
            TunnelState::Incomplete => palette.warning,
        },
        Outcome::InspectionEstablished => palette.info,
        Outcome::Cancelled | Outcome::Incomplete => palette.warning,
    }
}

/// Plain-language class of an HTTP status code, so that success and error do
/// not depend on the status color alone.
pub(in crate::ui) const fn status_class(code: u16) -> &'static str {
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
pub(in crate::ui) fn outcome_summary(outcome: &Outcome) -> String {
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

/// Matches the status code, failure stage and message, or the keyword of
/// an outcome without text, such as `pending` or `tunnel`. Response
/// headers are matched with the other headers.
pub(in crate::ui) fn outcome_matches(outcome: &Outcome, matcher: &Matcher) -> bool {
    match outcome {
        Outcome::Response(response) => matcher.is_match(response.status().as_str().as_bytes()),
        Outcome::Failed { stage, message } => {
            matcher.is_match(stage.to_string().as_bytes()) || matcher.is_match(message.as_bytes())
        }
        Outcome::InFlight
        | Outcome::Tunnel(_)
        | Outcome::InspectionEstablished
        | Outcome::Cancelled
        | Outcome::Incomplete => matcher.is_match(outcome_label(outcome).as_bytes()),
    }
}

/// Lowercase search keyword for outcomes that carry no searchable text.
const fn outcome_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "pending",
        Outcome::Response(_) => "response",
        Outcome::Tunnel(_) => "tunnel",
        Outcome::InspectionEstablished => "https inspection",
        Outcome::Failed { .. } => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Incomplete => "incomplete",
    }
}

pub(in crate::ui) fn detail_text(outcome: &Outcome, palette: &Palette) -> (String, Color32) {
    let text = match outcome {
        Outcome::InFlight => "Waiting for the upstream server.",
        Outcome::InspectionEstablished => "HTTPS inspection session established.",
        Outcome::Cancelled => "The client closed the connection before a response arrived.",
        Outcome::Incomplete => "The proxy stopped before a response arrived.",
        _ => return outcome_text(outcome, palette),
    };
    (text.to_owned(), outcome_text(outcome, palette).1)
}

pub(in crate::ui) const fn timing_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "Waiting",
        Outcome::Response(_) => "Response headers",
        Outcome::Tunnel(_) | Outcome::InspectionEstablished => "Established",
        Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => "Ended",
    }
}

pub(in crate::ui) fn tunnel_detail(state: &TunnelState) -> String {
    match state {
        TunnelState::Open => "Open".to_owned(),
        TunnelState::Closed => "Closed".to_owned(),
        TunnelState::Failed { stage, message } => format!("Failed at {stage}: {message}"),
        TunnelState::Incomplete => "Incomplete: the proxy stopped while it was open".to_owned(),
    }
}

/// Converts a duration to milliseconds for UI timing displays.
pub(in crate::ui) fn elapsed_millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_proxy::FailureStage;

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
            crate::ui::theme::Theme::Light,
            crate::ui::theme::Theme::Dark,
        ] {
            let palette = Palette::for_theme(theme);
            assert_eq!(
                outcome_text(&outcome, &palette),
                detail_text(&outcome, &palette)
            );
            assert_eq!(outcome_text(&outcome, &palette).1, palette.error);
        }
        let matches = |query: &str| {
            Matcher::new(query, http_proxy::text_search::MatchOptions::default())
                .is_ok_and(|matcher| outcome_matches(&outcome, &matcher))
        };
        assert!(matches("upstream tls handshake"));
        assert!(matches("certificate rejected"));
        assert!(!matches("response timeout"));
    }
}
