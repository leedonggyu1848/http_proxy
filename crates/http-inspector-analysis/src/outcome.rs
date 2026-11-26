//! Words for an exchange's outcome, shared by the request list, the
//! inspector, and the command line, so every view names a result the same way.

use http_inspector_exchange::{Outcome, TunnelPreview, TunnelState};

/// Short status text for one outcome, such as `404 Not Found`, `Pending`,
/// or `Connect: connection refused`.
#[must_use]
pub fn short_text(outcome: &Outcome) -> String {
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

fn tunnel_summary(tunnel: &TunnelPreview) -> String {
    match tunnel.snapshot().state() {
        TunnelState::Open => "Tunnel · open".to_owned(),
        TunnelState::Closed => "Tunnel · closed".to_owned(),
        TunnelState::Failed { stage, .. } => format!("Tunnel · failed ({stage})"),
        TunnelState::Incomplete => "Tunnel · incomplete".to_owned(),
    }
}

/// Plain-language class of an HTTP status code, so that success and error do
/// not depend on the status color alone.
#[must_use]
pub const fn status_class(code: u16) -> &'static str {
    match code {
        100..=199 => "informational",
        200..=299 => "success",
        300..=399 => "redirect",
        400..=499 => "client error",
        500..=599 => "server error",
        _ => "nonstandard status",
    }
}

/// A response as the server sent it: status code, reason phrase as sent,
/// and HTTP version, such as `502 Bad Gateway · HTTP/1.1`. A response
/// without a reason phrase shows the code and version only.
#[must_use]
pub fn response_line(code: u16, reason: &str, version: &str) -> String {
    if reason.is_empty() {
        format!("{code} · {version}")
    } else {
        format!("{code} {reason} · {version}")
    }
}

/// Status text of the inspector's result line. A response shows the reason
/// phrase as the server sent it and the HTTP version it answered with;
/// other outcomes show [`short_text`].
#[must_use]
pub fn detail_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Response(response) => response_line(
            response.status().as_u16(),
            response.reason(),
            &format!("{:?}", response.version()),
        ),
        _ => short_text(outcome),
    }
}

/// Name of the phase that ends when the outcome is known.
#[must_use]
pub const fn timing_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "Waiting",
        Outcome::Response(_) => "Response headers",
        Outcome::Tunnel(_) | Outcome::InspectionEstablished => "Established",
        Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => "Ended",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn names_outcomes_without_a_response() {
        assert_eq!(short_text(&Outcome::InFlight), "Pending");
        assert_eq!(short_text(&Outcome::Cancelled), "Cancelled by client");
        assert_eq!(
            response_line(502, "Bad Gateway", "HTTP/1.1"),
            "502 Bad Gateway · HTTP/1.1"
        );
        assert_eq!(response_line(200, "", "HTTP/2.0"), "200 · HTTP/2.0");
    }
}
