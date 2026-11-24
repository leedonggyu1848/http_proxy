//! Turns one finished [`Exchange`] into an [`EndpointObservation`].
//!
//! This is the boundary where captured request data enters schema
//! aggregation: header values, query values, credentials and body values are
//! read here only to name their type and are never copied out.

use super::body::{BodyAnalysis, BodyParse, ExclusionReason};
use super::endpoint::{BodyObservation, EndpointObservation, ResponseObservation};
use super::value::ValueShape;
use crate::url::{classify_id_segment, parse_exchange_target, visit_query_values};
use http_inspector_content::detect as content_detect;
use http_inspector_exchange::{
    BodyLifecycle, BodyPreview, BodySide, BodySource, Exchange, HeaderPreview, MediaType, Outcome,
    ReadBodyOutcome,
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

/// Describes the structure of a finished exchange, reading structured
/// bodies from `bodies`.
#[must_use]
pub fn observe_exchange(exchange: &Exchange, bodies: &dyn BodySource) -> EndpointObservation {
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
    bodies: &dyn BodySource,
    exchange: &Exchange,
    side: BodySide,
    preview: Option<&BodyPreview>,
    headers: &HeaderPreview,
    media: Option<&MediaType>,
) -> BodyObservation {
    let analysis = analyze(
        media,
        headers.get("content-type").next(),
        || bodies.read(exchange, side, MAX_ANALYZED_BODY_BYTES),
        || unread_body(preview),
    );
    BodyObservation {
        content_type: media.map(|media| media.essence().to_owned()),
        analysis,
    }
}

/// Describes a body in the format [`content_detect`] decides, reading it
/// only when that format can carry structure or has to be sniffed.
fn analyze(
    media: Option<&MediaType>,
    raw_content_type: Option<&str>,
    read: impl FnOnce() -> ReadBodyOutcome,
    unread: impl FnOnce() -> BodyAnalysis,
) -> BodyAnalysis {
    let Some(detection) = content_detect::from_content_type(media) else {
        return BodyAnalysis::describe_sniffed(media, read());
    };
    let parse = BodyParse::new(detection, raw_content_type);
    if parse.needs_bytes() {
        BodyAnalysis::describe(&parse, read())
    } else {
        unread()
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
    use std::error::Error;

    use http::header::CONTENT_TYPE;
    use http::{HeaderValue, Method, StatusCode};
    use serde_json::json;

    use super::*;
    use crate::endpoint_schema::FieldShape;
    use http_inspector_content::format::BodyFormat;
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};
    use http_inspector_exchange::{MemoryBodySource, ReadBodyOutcome};

    type TestResult = Result<(), Box<dyn Error>>;

    /// A finished `POST` whose request and response bodies have `content_type`.
    fn exchange(content_type: &'static str) -> Result<Exchange, Box<dyn Error>> {
        let media = HeaderValue::from_static(content_type);
        Ok(
            ExchangeBuilder::http(Method::POST, "http://api.example.test/users/42")?
                .header(CONTENT_TYPE, media.clone())
                .request_body("preview", BodyLifecycle::Complete)
                .respond(
                    ResponseFixture::new(StatusCode::OK)
                        .header(CONTENT_TYPE, media)
                        .body("preview", BodyLifecycle::Complete),
                )?,
        )
    }

    fn response_analysis(observation: &EndpointObservation) -> Option<&BodyAnalysis> {
        match &observation.response {
            ResponseObservation::Received { body, .. } => Some(&body.analysis),
            ResponseObservation::Missing => None,
        }
    }

    #[test]
    fn structured_bodies_are_described_from_the_body_source() -> TestResult {
        let exchange = exchange("application/json")?;
        let bodies = MemoryBodySource::new()
            .with_body(exchange.id(), BodySide::Request, r#"{"name":"SECRET"}"#)
            .with_body(exchange.id(), BodySide::Response, r#"{"id":7}"#);

        let observation = observe_exchange(&exchange, &bodies);

        assert_eq!(
            observation.request_body.analysis,
            BodyAnalysis::Fields(FieldShape::from_json(&json!({ "name": "value" })))
        );
        assert_eq!(
            response_analysis(&observation),
            Some(&BodyAnalysis::Fields(FieldShape::from_json(
                &json!({ "id": 0 })
            )))
        );
        assert_eq!(
            observation.request_body.content_type.as_deref(),
            Some("application/json")
        );
        Ok(())
    }

    #[test]
    fn unreadable_bodies_are_excluded_with_the_source_reason() -> TestResult {
        let exchange = exchange("application/json")?;
        let bodies = MemoryBodySource::new()
            .with_outcome(
                exchange.id(),
                BodySide::Request,
                ReadBodyOutcome::DecodeFailed,
            )
            .with_body(
                exchange.id(),
                BodySide::Response,
                vec![b' '; MAX_ANALYZED_BODY_BYTES + 1],
            );

        let observation = observe_exchange(&exchange, &bodies);
        assert_eq!(
            observation.request_body.analysis,
            BodyAnalysis::Excluded(ExclusionReason::DecodeFailed)
        );
        assert_eq!(
            response_analysis(&observation),
            Some(&BodyAnalysis::Excluded(ExclusionReason::TooLarge))
        );

        let missing = observe_exchange(&exchange, &MemoryBodySource::new());
        assert_eq!(
            missing.request_body.analysis,
            BodyAnalysis::Excluded(ExclusionReason::Unavailable)
        );
        Ok(())
    }

    #[test]
    fn bodies_without_field_structure_are_not_read() -> TestResult {
        let exchange = exchange("text/html")?;
        let observation = observe_exchange(&exchange, &MemoryBodySource::new());
        assert_eq!(
            observation.request_body.analysis,
            BodyAnalysis::Unstructured
        );
        assert_eq!(
            response_analysis(&observation),
            Some(&BodyAnalysis::Unstructured)
        );
        Ok(())
    }

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

    /// What the Map detail panel makes of a body of a Body tab format.
    #[derive(Debug, Eq, PartialEq)]
    enum MapReading {
        Fields,
        Parts,
        Unstructured,
    }

    impl MapReading {
        fn expected_for(format: BodyFormat) -> Self {
            match format {
                BodyFormat::Json | BodyFormat::Form => Self::Fields,
                BodyFormat::Multipart => Self::Parts,
                BodyFormat::Css
                | BodyFormat::Markup
                | BodyFormat::Xml
                | BodyFormat::JavaScript
                | BodyFormat::Text => Self::Unstructured,
            }
        }

        fn of(analysis: &BodyAnalysis) -> Option<Self> {
            match analysis {
                BodyAnalysis::Fields(_) => Some(Self::Fields),
                BodyAnalysis::Parts(_) => Some(Self::Parts),
                BodyAnalysis::Unstructured => Some(Self::Unstructured),
                BodyAnalysis::Empty | BodyAnalysis::Excluded(_) | BodyAnalysis::Invalid => None,
            }
        }
    }

    #[test]
    fn body_tab_and_map_classify_bodies_alike() {
        let late_json = [
            vec![b' '; content_detect::SNIFF_BYTES],
            br#"{"a":1}"#.to_vec(),
        ]
        .concat();
        let multipart = b"--B\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nx\r\n--B--";
        for (content_type, body, expected) in [
            (
                Some("application/json"),
                &br#"{"a":1}"#[..],
                BodyFormat::Json,
            ),
            (
                Some("application/problem+json; charset=utf-8"),
                br#"{"title":"x"}"#,
                BodyFormat::Json,
            ),
            (None, br#"{"a":1}"#, BodyFormat::Json),
            (None, b" [1,2]", BodyFormat::Json),
            (Some("text/plain"), br#"{"a":1}"#, BodyFormat::Json),
            (None, b"{oops", BodyFormat::Text),
            (None, b"42", BodyFormat::Text),
            (Some("text/plain"), b"hello", BodyFormat::Text),
            (
                Some("application/x-www-form-urlencoded"),
                b"a=1&b=2",
                BodyFormat::Form,
            ),
            (
                Some("multipart/form-data; boundary=B"),
                multipart,
                BodyFormat::Multipart,
            ),
            (Some("application/xml"), b"<a>1</a>", BodyFormat::Xml),
            (Some("application/atom+xml"), b"<feed/>", BodyFormat::Xml),
            (None, b"<a>1</a>", BodyFormat::Text),
            (Some("jsonish"), br#"{"a":1}"#, BodyFormat::Json),
            (None, &late_json, BodyFormat::Text),
        ] {
            let media = content_type.and_then(MediaType::parse);
            let body_tab = content_detect::settle(media.as_ref(), body);
            assert_eq!(body_tab, expected, "{content_type:?} {body:?}");
            let map = analyze(
                media.as_ref(),
                content_type,
                || ReadBodyOutcome::Complete(body.to_vec()),
                || BodyAnalysis::Unstructured,
            );
            assert_eq!(
                MapReading::of(&map),
                Some(MapReading::expected_for(body_tab)),
                "{content_type:?} {body:?}"
            );
        }
    }
}
