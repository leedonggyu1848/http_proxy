//! What one exchange offers to search, and whether it matches a query.
//!
//! Bodies are searched in their decoded preview: the first 8 KiB that the
//! detail panel shows, after removing `Content-Encoding` under the same
//! limits as the preview. Binary bodies are skipped.

use std::collections::HashMap;
use std::sync::Arc;

use super::SearchScope;
use super::text_search::Matcher;
use crate::content::decode as body_view;
use crate::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySide, Exchange, HeaderPreview, MediaType,
    Outcome, RecordId,
};

/// Bytes at the start of a decoded body that are checked for NUL when the
/// media type does not say whether the body is text.
const SNIFF_BYTES: usize = 1024;

/// Why a body that did not match may still contain the query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Coverage {
    /// Only the first part of a body was searched.
    pub partial: bool,
    /// A body was binary and was not searched.
    pub binary: bool,
    /// A body could not be decoded and was not searched.
    pub undecodable: bool,
}

impl Coverage {
    const fn add(&mut self, body: &BodyText) {
        match body {
            BodyText::Searchable { partial, .. } => self.partial |= *partial,
            BodyText::Binary => self.binary = true,
            BodyText::Undecodable => self.undecodable = true,
            BodyText::Empty => {}
        }
    }
}

/// The result of checking one exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Checked {
    /// Identifier of the checked exchange.
    pub id: RecordId,
    /// Whether the exchange matched the query.
    pub matched: bool,
    /// Nothing searched can change any more, so the result is final.
    pub settled: bool,
    /// Set only for exchanges that did not match.
    pub coverage: Coverage,
}

/// The searchable form of one body.
#[derive(Debug)]
enum BodyText {
    Searchable { bytes: Vec<u8>, partial: bool },
    Binary,
    Undecodable,
    Empty,
}

/// Decoded bodies of settled exchanges, so a new query does not decompress
/// them again.
#[derive(Default)]
pub struct BodyCache {
    bodies: HashMap<(RecordId, BodySide), Arc<BodyText>>,
}

impl BodyCache {
    /// Forgets bodies of exchanges older than `oldest`. Record ids grow, and
    /// the list drops its oldest records first, so these are no longer shown.
    pub fn forget_before(&mut self, oldest: RecordId) {
        self.bodies
            .retain(|(id, _), _| id.value() >= oldest.value());
    }

    /// Number of cached decoded body previews.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Reports whether the cache holds no decoded body previews.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }
}

/// Reports whether every part of `record` that search reads is final.
#[must_use]
pub fn is_settled(record: &Exchange) -> bool {
    let ended = |body: &BodyPreview| body.lifecycle() != BodyLifecycle::Streaming;
    let request = record.request().body().is_none_or(ended);
    request
        && match record.outcome() {
            Outcome::InFlight => false,
            Outcome::Response(response) => ended(response.body()),
            _ => true,
        }
}

/// Checks `record` against `matcher` in `scope`.
#[must_use]
pub fn check(
    record: &Exchange,
    matcher: &Matcher,
    scope: SearchScope,
    cache: &mut BodyCache,
) -> Checked {
    let settled = is_settled(record);
    let mut coverage = Coverage::default();
    let hit = metadata_matches(record, matcher, scope)
        || [BodySide::Request, BodySide::Response]
            .into_iter()
            .filter(|side| scope.covers_body(*side))
            .any(|side| {
                let Some(body) = body_text(record, side, settled, cache) else {
                    return false;
                };
                coverage.add(&body);
                matches!(&*body, BodyText::Searchable { bytes, .. } if matcher.is_match(bytes))
            });
    Checked {
        id: record.id(),
        matched: hit,
        settled,
        coverage: if hit { Coverage::default() } else { coverage },
    }
}

fn metadata_matches(record: &Exchange, matcher: &Matcher, scope: SearchScope) -> bool {
    let request = record.request();
    let response_headers = match record.outcome() {
        Outcome::Response(response) => Some(response.headers()),
        _ => None,
    };
    let text = |value: &str| matcher.is_match(value.as_bytes());
    (scope.covers_url() && text(request.target()))
        || (scope.covers_headers()
            && (headers_match(request.headers(), matcher)
                || response_headers.is_some_and(|headers| headers_match(headers, matcher))))
        || (scope == SearchScope::All
            && (text(request.method().as_str()) || outcome_matches(record.outcome(), matcher)))
}

/// Matches the status code, failure stage and message, or the keyword of
/// an outcome without text, such as `pending` or `tunnel`. Response
/// headers are matched with the other headers.
#[must_use]
pub fn outcome_matches(outcome: &Outcome, matcher: &Matcher) -> bool {
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

/// Matches header names and the values kept at capture. Values such as
/// `Authorization` were replaced before they reached analysis, so their
/// original text cannot be found.
#[must_use]
pub fn headers_match(headers: &HeaderPreview, matcher: &Matcher) -> bool {
    headers.entries().iter().any(|header| {
        matcher.is_match(header.name().as_bytes()) || matcher.is_match(header.value().as_bytes())
    })
}

/// The searchable text of one body, decoded once and cached when settled.
/// `None` when the body is not available yet.
fn body_text(
    record: &Exchange,
    side: BodySide,
    settled: bool,
    cache: &mut BodyCache,
) -> Option<Arc<BodyText>> {
    let key = (record.id(), side);
    if let Some(body) = cache.bodies.get(&key) {
        return Some(Arc::clone(body));
    }
    let (body, headers, media_type) = match (side, record.outcome()) {
        (BodySide::Request, _) => {
            let request = record.request();
            (request.body()?, request.headers(), request.media_type())
        }
        (BodySide::Response, Outcome::Response(response)) => {
            (response.body(), response.headers(), response.media_type())
        }
        (BodySide::Response, _) => return None,
    };
    let snapshot = body.snapshot()?;
    let encodings: Vec<&str> = headers.get("content-encoding").collect();
    let text = Arc::new(searchable(
        snapshot.bytes(),
        &encodings,
        media_type,
        snapshot.retention() != BodyRetention::Prefix
            || snapshot.lifecycle() != BodyLifecycle::Complete,
    ));
    if settled {
        cache.bodies.insert(key, Arc::clone(&text));
    }
    Some(text)
}

/// Decodes retained bytes into searchable text. `incomplete` means the
/// retained bytes are not the whole body.
fn searchable(
    bytes: &[u8],
    encodings: &[&str],
    media_type: Option<&MediaType>,
    incomplete: bool,
) -> BodyText {
    if bytes.is_empty() {
        return if incomplete {
            BodyText::Searchable {
                bytes: Vec::new(),
                partial: true,
            }
        } else {
            BodyText::Empty
        };
    }
    if media_type.is_some_and(is_binary_media_type) {
        return BodyText::Binary;
    }
    let Ok(decoded) = body_view::decode_prefix(bytes, encodings) else {
        return BodyText::Undecodable;
    };
    let sniffed = decoded.bytes.get(..SNIFF_BYTES).unwrap_or(&decoded.bytes);
    if memchr::memchr(0, sniffed).is_some() {
        return BodyText::Binary;
    }
    BodyText::Searchable {
        partial: decoded.cut || incomplete,
        bytes: decoded.bytes,
    }
}

/// Media types whose bodies are not text. SVG is an image written as XML,
/// so it is searched.
fn is_binary_media_type(media_type: &MediaType) -> bool {
    match media_type.kind() {
        "image" => media_type.subtype() != "svg+xml",
        "audio" | "video" | "font" => true,
        "application" => {
            let subtype = media_type.subtype();
            matches!(
                subtype,
                "octet-stream"
                    | "pdf"
                    | "zip"
                    | "gzip"
                    | "x-gzip"
                    | "zstd"
                    | "x-tar"
                    | "x-7z-compressed"
                    | "x-rar-compressed"
                    | "wasm"
                    | "protobuf"
                    | "x-protobuf"
                    | "vnd.google.protobuf"
                    | "msgpack"
                    | "x-msgpack"
                    | "cbor"
                    | "vnd.ms-fontobject"
            ) || subtype.starts_with("grpc")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn media(value: &str) -> Option<MediaType> {
        MediaType::parse(value)
    }

    fn found(body: &BodyText, query: &str) -> bool {
        let Ok(matcher) = Matcher::new(query, crate::analysis::search::MatchOptions::default())
        else {
            return false;
        };
        matches!(body, BodyText::Searchable { bytes, .. } if matcher.is_match(bytes))
    }

    #[test]
    fn compressed_bodies_are_searched_after_decoding() -> Result<(), Box<dyn std::error::Error>> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(br#"{"error":"Quota exceeded"}"#)?;
        let gzip = encoder.finish()?;
        let body = searchable(&gzip, &["gzip"], media("application/json").as_ref(), false);
        assert!(found(&body, "quota EXCEEDED"));
        assert!(matches!(body, BodyText::Searchable { partial: false, .. }));
        let zstd = zstd::encode_all(&b"zstd needle"[..], 0)?;
        assert!(found(&searchable(&zstd, &["zstd"], None, false), "needle"));
        Ok(())
    }

    #[test]
    fn long_or_incomplete_bodies_are_marked_partial() {
        let long = vec![b'a'; crate::BODY_PREVIEW_BYTES + 10];
        assert!(matches!(
            searchable(&long, &[], None, false),
            BodyText::Searchable { partial: true, .. }
        ));
        assert!(matches!(
            searchable(b"abc", &[], None, true),
            BodyText::Searchable { partial: true, .. }
        ));
        assert!(matches!(searchable(b"", &[], None, false), BodyText::Empty));
    }

    #[test]
    fn binary_bodies_are_skipped_by_type_or_content() {
        let png = searchable(b"\x89PNG needle", &[], media("image/png").as_ref(), false);
        assert!(matches!(png, BodyText::Binary));
        let unknown = searchable(b"needle\0\x01", &[], None, false);
        assert!(matches!(unknown, BodyText::Binary));
        let svg = searchable(
            b"<svg>needle</svg>",
            &[],
            media("image/svg+xml").as_ref(),
            false,
        );
        assert!(found(&svg, "needle"));
        assert!(matches!(
            searchable(b"x", &["compress"], None, false),
            BodyText::Undecodable
        ));
    }
}
