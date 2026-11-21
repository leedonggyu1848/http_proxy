//! Shared outcome presentation for the request list, details, and search.

use eframe::egui::Color32;
use http_proxy::{Outcome, TunnelPreview, TunnelState};

use crate::theme::Palette;

/// Orders responses by status code, then every other outcome after them.
pub(crate) const fn status_rank(outcome: &Outcome) -> (u8, u16) {
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
    match outcome {
        Outcome::InFlight => ("Pending".to_owned(), palette.muted),
        Outcome::Response(response) => {
            let status = response.status();
            let color = if status.as_u16() < 400 {
                palette.success
            } else {
                palette.error
            };
            (
                format!(
                    "{} {}",
                    status.as_str(),
                    status.canonical_reason().unwrap_or("Unknown status")
                ),
                color,
            )
        }
        Outcome::Tunnel(tunnel) => tunnel_summary(tunnel, palette),
        Outcome::InspectionEstablished => ("HTTPS".to_owned(), palette.info),
        Outcome::Failed { stage, message } => (format!("{stage}: {message}"), palette.error),
        Outcome::Cancelled => ("Cancelled by client".to_owned(), palette.warning),
        Outcome::Incomplete => ("Incomplete (proxy stopped)".to_owned(), palette.warning),
    }
}

fn tunnel_summary(tunnel: &TunnelPreview, palette: &Palette) -> (String, Color32) {
    match tunnel.snapshot().state() {
        TunnelState::Open => ("Tunnel · open".to_owned(), palette.info),
        TunnelState::Closed => ("Tunnel · closed".to_owned(), palette.info),
        TunnelState::Failed { stage, .. } => (format!("Tunnel · failed ({stage})"), palette.error),
        TunnelState::Incomplete => ("Tunnel · incomplete".to_owned(), palette.warning),
    }
}

pub(crate) fn outcome_matches_query(outcome: &Outcome, query: &str) -> bool {
    match outcome {
        Outcome::Response(response) => {
            response.status().as_str().contains(query)
                || super::headers_match(response.headers(), query)
        }
        Outcome::Failed { stage, message } => {
            super::text_matches(&format!("{stage}"), query) || super::text_matches(message, query)
        }
        Outcome::InFlight
        | Outcome::Tunnel(_)
        | Outcome::InspectionEstablished
        | Outcome::Cancelled
        | Outcome::Incomplete => outcome_label(outcome).contains(query),
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

pub(crate) fn tunnel_detail(state: &TunnelState) -> String {
    match state {
        TunnelState::Open => "Open".to_owned(),
        TunnelState::Closed => "Closed".to_owned(),
        TunnelState::Failed { stage, message } => format!("Failed at {stage}: {message}"),
        TunnelState::Incomplete => "Incomplete: the proxy stopped while it was open".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_proxy::FailureStage;

    #[test]
    fn failure_stage_is_shared_by_list_details_and_search() {
        let outcome = Outcome::Failed {
            stage: FailureStage::UpstreamTls,
            message: "certificate rejected".to_owned(),
        };
        for theme in [crate::theme::Theme::Light, crate::theme::Theme::Dark] {
            let palette = Palette::for_theme(theme);
            assert_eq!(
                outcome_text(&outcome, &palette),
                detail_text(&outcome, &palette)
            );
            assert_eq!(outcome_text(&outcome, &palette).1, palette.error);
        }
        assert!(outcome_matches_query(&outcome, "upstream tls handshake"));
        assert!(outcome_matches_query(&outcome, "certificate rejected"));
        assert!(!outcome_matches_query(&outcome, "response timeout"));
    }
}
