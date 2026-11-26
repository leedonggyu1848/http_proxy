//! JSON documents for captured records. Header values are printed as they
//! were captured, so credentials masked at capture stay `[REDACTED]`.

use std::time::{Duration, UNIX_EPOCH};

use http_inspector_analysis::filter::{TargetParts, content_type};
use http_inspector_analysis::outcome;
use http_inspector_analysis::search::is_settled;
use http_inspector_content::format::form;
use http_inspector_content::view::BodyCapture;
use http_inspector_exchange::http_semantics::{REDACTED_VALUE, is_sensitive_header, urlencoded};
use http_inspector_exchange::{
    BodyLifecycle, BodyPreview, CaptureSource, Exchange, HeaderPreview, MediaType, Outcome,
    TunnelState,
};
use serde_json::{Value, json};

use crate::protocol::Part;

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn optional_millis(duration: Option<Duration>) -> Value {
    duration.map_or(Value::Null, |duration| json!(millis(duration)))
}

/// Stable lowercase name of an outcome kind.
pub(crate) const fn outcome_kind(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "pending",
        Outcome::Response(_) => "response",
        Outcome::Tunnel(_) => "tunnel",
        Outcome::InspectionEstablished => "https_inspection",
        Outcome::Failed { .. } => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Incomplete => "incomplete",
    }
}

const fn source_name(source: CaptureSource) -> &'static str {
    match source {
        CaptureSource::HttpProxy => "http_proxy",
        CaptureSource::Connect => "connect",
        CaptureSource::InspectedHttps => "inspected_https",
    }
}

const fn lifecycle_name(lifecycle: BodyLifecycle) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => "streaming",
        BodyLifecycle::Complete => "complete",
        BodyLifecycle::Interrupted => "interrupted",
        BodyLifecycle::Cancelled => "cancelled",
    }
}

/// One row of `records list`, `records watch`, and `records wait`.
pub(crate) fn summary(record: &Exchange) -> Value {
    let request = record.request();
    let parts = TargetParts::of(request.target());
    let outcome = record.outcome();
    let status = match outcome {
        Outcome::Response(response) => Some(response.status().as_u16()),
        _ => None,
    };
    let started_ms = record
        .started_at()
        .duration_since(UNIX_EPOCH)
        .map_or(Value::Null, |since| {
            json!(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        });
    json!({
        "id": record.id().value(),
        "method": request.method().as_str(),
        "url": request.target(),
        "host": parts.host,
        "path": parts.path,
        "outcome": outcome_kind(outcome),
        "status": status,
        "summary": outcome::short_text(outcome),
        "type": content_type(record),
        "received_bytes": record.received_bytes(),
        "duration_ms": millis(record.elapsed()),
        "settled": is_settled(record),
        "source": source_name(record.source()),
        "replay_of": record.replay_of().map(http_inspector_exchange::RecordId::value),
        "started_at_ms": started_ms,
    })
}

/// The default row of `records list` and `records watch`: enough to pick a
/// record and decide what to read next. `--full` prints [`summary`].
pub(crate) fn row(record: &Exchange) -> Value {
    let request = record.request();
    let status = match record.outcome() {
        Outcome::Response(response) => Some(response.status().as_u16()),
        _ => None,
    };
    json!({
        "id": record.id().value(),
        "method": request.method().as_str(),
        "url": request.target(),
        "outcome": outcome_kind(record.outcome()),
        "status": status,
        "duration_ms": millis(record.elapsed()),
        "request_bytes": request.body().map(BodyPreview::observed_bytes),
        "response_bytes": record.received_bytes(),
    })
}

/// One record, as a row or with every summary field.
pub(crate) fn listed(record: &Exchange, full: bool) -> Value {
    if full { summary(record) } else { row(record) }
}

/// Compact change marker for `records watch`: a row is printed again when
/// this changes.
pub(crate) fn state_key(record: &Exchange) -> (&'static str, Option<u16>, bool) {
    let status = match record.outcome() {
        Outcome::Response(response) => Some(response.status().as_u16()),
        _ => None,
    };
    (outcome_kind(record.outcome()), status, is_settled(record))
}

fn headers(preview: &HeaderPreview) -> Value {
    let entries: Vec<Value> = preview
        .entries()
        .iter()
        .map(|header| {
            json!({
                "name": header.name(),
                "value": header.value(),
                "redacted": header.value() == REDACTED_VALUE && is_sensitive_header(header.name()),
            })
        })
        .collect();
    Value::Array(entries)
}

fn query(target: &str) -> Value {
    urlencoded::split_target(target).query.map_or_else(
        || json!([]),
        |query| {
            Value::Array(
                form::pairs(query)
                    .into_iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect(),
            )
        },
    )
}

/// What is known about one body without printing it.
pub(crate) fn body_summary(
    body: &BodyPreview,
    headers: &HeaderPreview,
    media_type: Option<&MediaType>,
) -> Value {
    let snapshot = body.snapshot();
    json!({
        "observed_bytes": body.observed_bytes(),
        "retained_bytes": snapshot.as_ref().map(|snapshot| snapshot.bytes().len()),
        "lifecycle": lifecycle_name(body.lifecycle()),
        "capture": snapshot.as_ref().map(|snapshot| BodyCapture::of(snapshot).name()),
        "media_type": media_type.map(MediaType::essence),
        "content_encoding": headers.get("content-encoding").collect::<Vec<_>>(),
    })
}

fn request_detail(record: &Exchange) -> Value {
    let request = record.request();
    json!({
        "method": request.method().as_str(),
        "target": request.target(),
        "headers": headers(request.headers()),
        "headers_omitted": request.headers().omitted(),
        "auth_schemes": request
            .auth_schemes()
            .iter()
            .map(|scheme| scheme.label())
            .collect::<Vec<_>>(),
        "query": query(request.target()),
        "body": request.body().map(|body| {
            body_summary(body, request.headers(), request.media_type())
        }),
    })
}

fn response_detail(record: &Exchange) -> Value {
    let Outcome::Response(response) = record.outcome() else {
        return Value::Null;
    };
    json!({
        "status": response.status().as_u16(),
        "reason": response.reason(),
        "version": format!("{:?}", response.version()),
        "line": outcome::detail_text(record.outcome()),
        "status_class": outcome::status_class(response.status().as_u16()),
        "headers": headers(response.headers()),
        "headers_omitted": response.headers().omitted(),
        "body": body_summary(response.body(), response.headers(), response.media_type()),
    })
}

fn outcome_detail(record: &Exchange) -> Value {
    match record.outcome() {
        Outcome::Failed { stage, message } => json!({
            "failure": { "stage": stage.to_string(), "message": message },
        }),
        Outcome::Tunnel(tunnel) => {
            let snapshot = tunnel.snapshot();
            let (state, failure) = match snapshot.state() {
                TunnelState::Open => ("open", Value::Null),
                TunnelState::Closed => ("closed", Value::Null),
                TunnelState::Failed { stage, message } => (
                    "failed",
                    json!({ "stage": stage.to_string(), "message": message }),
                ),
                TunnelState::Incomplete => ("incomplete", Value::Null),
            };
            json!({
                "tunnel": {
                    "state": state,
                    "failure": failure,
                    "client_to_upstream_bytes": snapshot.client_to_upstream(),
                    "upstream_to_client_bytes": snapshot.upstream_to_client(),
                    "duration_ms": millis(snapshot.duration()),
                },
            })
        }
        _ => json!({}),
    }
}

fn timing(record: &Exchange) -> Value {
    let timing = record.timing();
    json!({
        "connect_ms": optional_millis(timing.connect()),
        "send_ms": optional_millis(timing.send()),
        "wait_ms": optional_millis(timing.wait()),
        "receive_ms": optional_millis(timing.receive()),
        "settled_label": outcome::timing_label(record.outcome()),
        "settled_ms": optional_millis(timing.settled()),
        "total_ms": optional_millis(timing.complete()),
    })
}

/// The `records show` document.
pub(crate) fn detail(record: &Exchange, part: Part) -> Value {
    let mut document = summary(record);
    let extra = outcome_detail(record);
    if let (Some(document), Some(extra)) = (document.as_object_mut(), extra.as_object()) {
        document.extend(extra.clone());
        if part != Part::Response {
            document.insert("request".to_owned(), request_detail(record));
        }
        if part != Part::Request {
            document.insert("response".to_owned(), response_detail(record));
        }
        document.insert("timing".to_owned(), timing(record));
    }
    document
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "indexing a serde_json value by key returns null instead of panicking"
)]
mod tests {
    use std::error::Error;

    use http::header::{AUTHORIZATION, CONTENT_TYPE, SET_COOKIE};
    use http::{HeaderValue, Method, StatusCode};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    use super::*;

    #[test]
    fn details_keep_redacted_headers_masked() -> Result<(), Box<dyn Error>> {
        let record =
            ExchangeBuilder::http(Method::GET, "http://api.example.test/users?page=2&q=a%20b")?
                .header(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer secret-token"),
                )
                .respond(
                    ResponseFixture::new(StatusCode::OK)
                        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                        .header(SET_COOKIE, HeaderValue::from_static("id=secret-cookie")),
                )?;
        let document = detail(&record, Part::All);
        let text = document.to_string();
        assert!(!text.contains("secret-token"), "{text}");
        assert!(!text.contains("secret-cookie"), "{text}");
        assert_eq!(document["request"]["headers"][0]["value"], REDACTED_VALUE);
        assert_eq!(document["request"]["headers"][0]["redacted"], true);
        assert_eq!(document["request"]["query"][1]["value"], "a b");
        assert_eq!(document["status"], 200);
        assert_eq!(document["type"], "json");
        assert_eq!(document["response"]["line"], "200 OK · HTTP/1.1");
        assert_eq!(document["response"]["body"]["capture"], "whole");
        assert!(detail(&record, Part::Request).get("response").is_none());
        Ok(())
    }
}
