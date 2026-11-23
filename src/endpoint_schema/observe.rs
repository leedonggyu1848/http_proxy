//! Turns one finished [`Exchange`] into an [`EndpointObservation`].
//!
//! This is the boundary where captured request data enters schema
//! aggregation: header values, query values, credentials and body values are
//! read here only to name their type and are never copied out.

use super::body::{BodyAnalysis, BodyFormat, ExclusionReason};
use super::endpoint::{BodyObservation, EndpointObservation, ResponseObservation};
use super::value::ValueShape;
use crate::url_map::{classify_id_segment, parse_exchange_target, visit_query_values};
use crate::{
    BodyLifecycle, BodyPreview, BodySide, BodySource, BodyStore, Exchange, HeaderPreview,
    MediaType, Outcome, PreviewBodies, ReadBodyOutcome,
};

/// Largest decoded body described; larger bodies are excluded as too large.
pub const MAX_ANALYZED_BODY_BYTES: usize = 1024 * 1024;
/// Longest header name kept, in bytes.
const MAX_HEADER_NAME_BYTES: usize = 64;

/// Request headers every client sends or that describe the transport or
/// the body rather than the API. They are left out of the custom header list.
const STANDARD_HEADERS: &[&str] = &[
    "accept",
    "accept-charset",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "connection",
    "content-encoding",
    "content-length",
    "content-type",
    "date",
    "dnt",
    "expect",
    "forwarded",
    "from",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "keep-alive",
    "max-forwards",
    "origin",
    "pragma",
    "priority",
    "proxy-connection",
    "range",
    "referer",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "upgrade-insecure-requests",
    "user-agent",
    "via",
];

/// Headers that carry credentials; they are reported as authentication
/// schemes instead of header names.
const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "x-apikey",
    "x-auth-token",
    "x-access-token",
];

/// Reads complete bodies of finished exchanges from their in-memory
/// preview, or from the body store when the preview kept only a prefix.
///
/// Reading from the store waits for its writer and reads a file, so this is
/// only used on the schema analysis thread.
#[derive(Clone, Debug, Default)]
pub struct CapturedBodies {
    store: Option<BodyStore>,
}

impl CapturedBodies {
    /// Reads bodies from previews and, when given, from `store`.
    #[must_use]
    pub const fn new(store: Option<BodyStore>) -> Self {
        Self { store }
    }

    fn read(&self, exchange: &Exchange, side: BodySide) -> ReadBodyOutcome {
        let source: &dyn BodySource = self.store.as_ref().map_or(&PreviewBodies, |store| store);
        source.read(exchange, side, MAX_ANALYZED_BODY_BYTES)
    }
}

/// Describes the structure of a finished exchange.
#[must_use]
pub fn observe_exchange(exchange: &Exchange, bodies: &CapturedBodies) -> EndpointObservation {
    let request = exchange.request();
    let path_ids =
        parse_exchange_target(request.target(), exchange.source()).map_or_default(|url| {
            url.segments()
                .iter()
                .enumerate()
                .filter_map(|(position, segment)| {
                    classify_id_segment(segment).map(|kind| (position, kind))
                })
                .collect()
        });
    let mut query = Vec::new();
    visit_query_values(request.target(), |name, value| {
        query.push((name, ValueShape::of_text(value)));
    });
    let request_body = describe_body(
        bodies,
        exchange,
        BodySide::Request,
        request.body(),
        request.headers(),
        request.media_type(),
    );
    let response = match exchange.outcome() {
        Outcome::Response(response) => ResponseObservation::Received {
            status: response.status().as_u16(),
            body: describe_body(
                bodies,
                exchange,
                BodySide::Response,
                Some(response.body()),
                response.headers(),
                response.media_type(),
            ),
        },
        Outcome::InFlight
        | Outcome::Tunnel(_)
        | Outcome::InspectionEstablished
        | Outcome::Failed { .. }
        | Outcome::Cancelled
        | Outcome::Incomplete => ResponseObservation::Missing,
    };
    EndpointObservation {
        path_ids,
        query,
        headers: custom_header_names(request.headers()),
        auth: request.auth_schemes().to_vec(),
        request_body,
        response,
    }
}

fn describe_body(
    bodies: &CapturedBodies,
    exchange: &Exchange,
    side: BodySide,
    preview: Option<&BodyPreview>,
    headers: &HeaderPreview,
    media: Option<&MediaType>,
) -> BodyObservation {
    let format = BodyFormat::detect(media, headers.get("content-type").next());
    let analysis = if format.needs_bytes() {
        BodyAnalysis::describe(&format, bodies.read(exchange, side))
    } else {
        unread_body(preview)
    };
    BodyObservation {
        content_type: media.map(|media| media.essence().to_owned()),
        analysis,
    }
}

/// Describes a body whose format has no field structure without reading it.
fn unread_body(preview: Option<&BodyPreview>) -> BodyAnalysis {
    let Some(preview) = preview else {
        return BodyAnalysis::Empty;
    };
    match preview.lifecycle() {
        BodyLifecycle::Complete if preview.observed_bytes() == 0 => BodyAnalysis::Empty,
        BodyLifecycle::Complete => BodyAnalysis::Unstructured,
        BodyLifecycle::Streaming | BodyLifecycle::Interrupted | BodyLifecycle::Cancelled => {
            BodyAnalysis::Excluded(ExclusionReason::Truncated)
        }
    }
}

/// Lowercase names of request headers that are neither standard nor credential carriers.
fn custom_header_names(headers: &HeaderPreview) -> Vec<String> {
    let mut names: Vec<String> = headers
        .entries()
        .iter()
        .map(|header| header.name().to_ascii_lowercase())
        .filter(|name| is_custom_header(name))
        .map(|name| bounded(&name))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn is_custom_header(name: &str) -> bool {
    !STANDARD_HEADERS.contains(&name)
        && !CREDENTIAL_HEADERS.contains(&name)
        && !name.starts_with("sec-")
}

fn bounded(name: &str) -> String {
    if name.len() <= MAX_HEADER_NAME_BYTES {
        return name.to_owned();
    }
    let cut = name.floor_char_boundary(MAX_HEADER_NAME_BYTES);
    name.get(..cut).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_headers_skip_standard_credential_and_fetch_metadata_names() {
        assert!(is_custom_header("x-request-id"));
        assert!(is_custom_header("x-client-version"));
        assert!(!is_custom_header("user-agent"));
        assert!(!is_custom_header("cookie"));
        assert!(!is_custom_header("x-api-key"));
        assert!(!is_custom_header("sec-fetch-mode"));
        assert_eq!(bounded(&"x".repeat(100)).len(), MAX_HEADER_NAME_BYTES);
    }
}
