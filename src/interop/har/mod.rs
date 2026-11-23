//! HAR 1.2 export of captured exchanges.
//!
//! The export describes what the proxy observed and keeps in memory. It never
//! holds sensitive header values: the engine replaces them with `[REDACTED]`
//! when it captures them. By default the export also leaves out those header
//! fields and every body; [`HarOptions`] turns each back on.
//!
//! Records that are not plain responses are mapped as follows:
//!
//! - In flight, failed, cancelled, or incomplete: `response.status` is `0`,
//!   with the reason in the entry `comment` and in `response._error`, where
//!   Chrome writes the error of a failed request.
//! - `CONNECT`: `request.url` is `https://` plus the requested authority and
//!   `response.status` is the `200` the proxy answered. The tunnel's byte
//!   counts are the request and response `bodySize`, and its open time is
//!   `timings.receive`.
//!
//! The proxy does not keep header sizes, so `headersSize` is `-1` and
//! `response._transferSize`, which Chrome reads, counts body bytes only.

mod content;
mod timings;

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::exchange::{CaptureSource, Exchange, HeaderPreview, Outcome, TunnelState};
use serde_json::{Map, Value, json};

use self::timings::{Phases, iso8601, millis};

/// Header fields whose values the engine never retains.
const SENSITIVE_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

/// What an export adds on top of the default, which leaves out bodies and
/// sensitive header fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HarOptions {
    /// Writes body previews, which may be truncated, as `postData` and
    /// `content.text`.
    pub include_bodies: bool,
    /// Writes sensitive header fields with their `[REDACTED]` marker.
    pub include_sensitive_headers: bool,
}

/// Builds the HAR document for `records` in the given order, as observed at `now`.
#[must_use]
pub fn document(records: &[Exchange], options: HarOptions, now: SystemTime) -> Value {
    let entries: Vec<Value> = records
        .iter()
        .map(|record| entry(record, options, now))
        .collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": {
                "name": crate::APP_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            },
            "pages": [],
            "entries": entries,
            "comment": log_comment(options),
        }
    })
}

/// Writes `document` to a new file at `path`, never replacing an existing one.
///
/// The file is written to a temporary file in the same directory first, so a
/// failed export leaves no partial file, and is readable only by its owner.
pub fn write(path: &Path, document: &Value) -> io::Result<PathBuf> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if !directory.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not an existing directory", directory.display()),
        ));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, document)?;
        writer.flush()?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(path.to_path_buf())
}

fn log_comment(options: HarOptions) -> String {
    let bodies = if options.include_bodies {
        "Bodies are in-memory previews and may be truncated; see each comment."
    } else {
        "Bodies were left out."
    };
    let headers = if options.include_sensitive_headers {
        "Sensitive headers are listed with the value [REDACTED]."
    } else {
        "Authorization, Proxy-Authorization, Cookie, and Set-Cookie headers were left out."
    };
    format!(
        "Exported by {}. {bodies} {headers} Cookie values are never exported.",
        crate::APP_NAME
    )
}

fn entry(record: &Exchange, options: HarOptions, now: SystemTime) -> Value {
    let phases = Phases::of(record, now);
    let mut entry = Map::new();
    let _ = entry.insert(
        "startedDateTime".into(),
        json!(iso8601(record.started_at())),
    );
    let _ = entry.insert("time".into(), json!(millis(phases.total())));
    let _ = entry.insert("request".into(), request(record, options));
    let mut response = response(record.outcome(), options);
    if let (Some(error), Value::Object(fields)) = (error_text(record.outcome()), &mut response) {
        let _ = fields.insert("_error".into(), json!(error));
    }
    let _ = entry.insert("response".into(), response);
    let _ = entry.insert("cache".into(), json!({}));
    let _ = entry.insert("timings".into(), phases.to_json());
    let notes = entry_notes(record);
    if !notes.is_empty() {
        let _ = entry.insert("comment".into(), json!(notes.join(" ")));
    }
    Value::Object(entry)
}

fn request(record: &Exchange, options: HarOptions) -> Value {
    let request = record.request();
    let url = request_url(record);
    let body = request.body();
    let body_size = match record.outcome() {
        Outcome::Tunnel(tunnel) => json!(tunnel.snapshot().client_to_upstream()),
        _ => content::body_size(body),
    };
    let mut value = Map::new();
    let _ = value.insert("method".into(), json!(request.method().as_str()));
    let _ = value.insert("queryString".into(), query_string(&url));
    let _ = value.insert("url".into(), json!(url));
    let _ = value.insert("httpVersion".into(), json!("HTTP/1.1"));
    let _ = value.insert("cookies".into(), json!([]));
    let _ = value.insert("headers".into(), headers(request.headers(), options));
    let _ = value.insert("headersSize".into(), json!(-1));
    let _ = value.insert("bodySize".into(), body_size);
    if let Some(post_data) = content::post_data(body, request.headers(), options.include_bodies) {
        let _ = value.insert("postData".into(), post_data);
    }
    Value::Object(value)
}

fn response(outcome: &Outcome, options: HarOptions) -> Value {
    match outcome {
        Outcome::Response(response) => json!({
            "status": response.status().as_u16(),
            "statusText": response.reason(),
            "httpVersion": format!("{:?}", response.version()),
            "cookies": [],
            "headers": headers(response.headers(), options),
            "content": content::response_content(
                response.body(),
                response.headers(),
                options.include_bodies,
            ),
            "redirectURL": response.headers().get("location").next().unwrap_or_default(),
            "headersSize": -1,
            "bodySize": response.body().observed_bytes(),
            "_transferSize": response.body().observed_bytes(),
        }),
        Outcome::Tunnel(tunnel) => {
            let received = tunnel.snapshot().upstream_to_client();
            connect_response(received, "Encrypted tunnel bytes were not inspected.")
        }
        Outcome::InspectionEstablished => connect_response(
            0,
            "Decrypted requests of this connection are separate entries.",
        ),
        Outcome::InFlight | Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => {
            json!({
                "status": 0,
                "statusText": "",
                "httpVersion": "",
                "cookies": [],
                "headers": [],
                "content": { "size": 0, "mimeType": "x-unknown" },
                "redirectURL": "",
                "headersSize": -1,
                "bodySize": -1,
            })
        }
    }
}

/// The `200` the proxy answers to a `CONNECT` it accepted.
fn connect_response(received: u64, comment: &str) -> Value {
    json!({
        "status": 200,
        "statusText": "OK",
        "httpVersion": "HTTP/1.1",
        "cookies": [],
        "headers": [],
        "content": { "size": received, "mimeType": "x-unknown", "comment": comment },
        "redirectURL": "",
        "headersSize": -1,
        "bodySize": received,
        "_transferSize": received,
    })
}

/// Returns an absolute URL. A `CONNECT` target is an authority, so it gets
/// the `https://` scheme browsers use for the tunnels they open.
fn request_url(record: &Exchange) -> String {
    let target = record.request().target();
    if record.source() == CaptureSource::Connect && !target.contains("://") {
        format!("https://{target}")
    } else {
        target.to_owned()
    }
}

/// Splits the query of `url` into raw, still percent-encoded pairs.
fn query_string(url: &str) -> Value {
    let without_fragment = url.split_once('#').map_or(url, |(before, _)| before);
    let Some((_, query)) = without_fragment.split_once('?') else {
        return json!([]);
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            json!({ "name": name, "value": value })
        })
        .collect()
}

fn headers(headers: &HeaderPreview, options: HarOptions) -> Value {
    headers
        .entries()
        .iter()
        .filter(|header| options.include_sensitive_headers || !is_sensitive(header.name()))
        .map(|header| json!({ "name": header.name(), "value": header.value() }))
        .collect()
}

fn is_sensitive(name: &str) -> bool {
    SENSITIVE_HEADERS
        .iter()
        .any(|sensitive| sensitive.eq_ignore_ascii_case(name))
}

fn entry_notes(record: &Exchange) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(note) = outcome_note(record) {
        notes.push(note);
    }
    if let Some(original) = record.replay_of() {
        notes.push(format!("Manual replay of record #{}.", original.value()));
    }
    if record.request().headers().omitted() {
        notes.push("Some request headers were not kept because of the preview limit.".into());
    }
    if let Outcome::Response(response) = record.outcome()
        && response.headers().omitted()
    {
        notes.push("Some response headers were not kept because of the preview limit.".into());
    }
    notes
}

fn outcome_note(record: &Exchange) -> Option<String> {
    Some(match record.outcome() {
        Outcome::Response(_) => return None,
        Outcome::InFlight => "Still waiting for a response when exported.".to_owned(),
        Outcome::Tunnel(tunnel) => {
            let snapshot = tunnel.snapshot();
            format!(
                "CONNECT tunnel to {}; encrypted contents were not inspected. {} bytes client to server, {} bytes server to client. Tunnel: {}.",
                record.request().target(),
                snapshot.client_to_upstream(),
                snapshot.upstream_to_client(),
                snapshot.state(),
            )
        }
        Outcome::InspectionEstablished => format!(
            "CONNECT to {} for HTTPS inspection.",
            record.request().target()
        ),
        Outcome::Failed { stage, message } => {
            format!("Failed at {stage}: {message}. No upstream response was received.")
        }
        Outcome::Cancelled => "The client closed the connection before a response arrived.".into(),
        Outcome::Incomplete => "The proxy stopped before a response arrived.".into(),
    })
}

/// Returns the failure for `response._error`, where Chrome writes the
/// error of a failed request.
fn error_text(outcome: &Outcome) -> Option<String> {
    match outcome {
        Outcome::Failed { stage, message } => Some(format!("{stage}: {message}")),
        Outcome::Cancelled => Some("Cancelled by the client".into()),
        Outcome::Incomplete => Some("Incomplete because the proxy stopped".into()),
        Outcome::Tunnel(tunnel) => match tunnel.snapshot().state() {
            state @ (TunnelState::Failed { .. } | TunnelState::Incomplete) => {
                Some(state.to_string())
            }
            TunnelState::Open | TunnelState::Closed => None,
        },
        Outcome::InFlight | Outcome::Response(_) | Outcome::InspectionEstablished => None,
    }
}

#[cfg(test)]
mod tests;
