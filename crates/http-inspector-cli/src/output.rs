//! The only module that writes to stdout and stderr. Commands build JSON
//! documents; this module prints them as JSON with `"schema": 1` or as text
//! for people.
//!
//! In JSON mode every document, including errors, goes to stdout so a
//! script reads one stream. In text mode results go to stdout and errors
//! and body notices go to stderr.

use std::fmt::Write as _;
use std::io::{self, Write};

use serde_json::{Map, Value, json};

use crate::error::CliError;
use crate::protocol::SCHEMA_VERSION;

/// Which document a command printed, for text rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Status,
    Stopped,
    List,
    Show,
    Body,
    Wait,
    Cleared,
    LogSaved,
    Schema,
    Env,
    CaInfo,
    Replay,
    Curl,
    Exported,
    Endpoints,
    Hls,
}

/// How results are printed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Output {
    json: bool,
}

/// Adds `"schema": 1` to an object's fields.
pub(crate) fn with_schema(data: &Value) -> Value {
    let mut document = Map::new();
    document.insert("schema".to_owned(), json!(SCHEMA_VERSION));
    if let Value::Object(fields) = data {
        document.extend(fields.clone());
    }
    Value::Object(document)
}

impl Output {
    pub(crate) const fn new(json: bool) -> Self {
        Self { json }
    }

    /// Prints the result of a command.
    pub(crate) fn document(self, kind: Kind, data: &Value) {
        if self.json || kind == Kind::Schema {
            let text = if kind == Kind::Schema {
                serde_json::to_string_pretty(data).unwrap_or_default()
            } else {
                with_schema(data).to_string()
            };
            write_out(&format!("{text}\n"));
            return;
        }
        let (stdout, stderr) = render(kind, data);
        if !stderr.is_empty() {
            write_err(&stderr);
        }
        write_out(&stdout);
    }

    /// Prints one event of a stream. Returns `false` once stdout is closed.
    pub(crate) fn event(self, data: &Value) -> bool {
        let line = if self.json {
            format!("{}\n", with_schema(data))
        } else {
            match data.get("event").and_then(Value::as_str) {
                Some("added") => format!("+ {}\n", row(&data["record"])),
                Some("updated") => format!("~ {}\n", row(&data["record"])),
                Some("started") => format!(
                    "Watching records after ID {}\n",
                    data["since"].as_u64().unwrap_or_default()
                ),
                Some("cleared") => "Records cleared\n".to_owned(),
                Some("daemon_stopped") => "The daemon stopped\n".to_owned(),
                Some("progress") => format!(
                    "{} {}/{} segments ({} reused, {} bytes downloaded)\n",
                    text(&data["phase"]),
                    text(&data["completed_segments"]),
                    text(&data["total_segments"]),
                    text(&data["reused_segments"]),
                    text(&data["downloaded_bytes"]),
                ),
                Some("completed") => format!(
                    "Saved {} ({} bytes, {} segments)\n",
                    text(&data["path"]),
                    text(&data["total_bytes"]),
                    text(&data["total_segments"]),
                ),
                _ => format!("{data}\n"),
            }
        };
        try_write_out(&line).is_ok()
    }

    /// Prints a failure.
    pub(crate) fn error(self, error: &CliError) {
        if self.json {
            let document = with_schema(&json!({ "error": error.to_json() }));
            write_out(&format!("{document}\n"));
            return;
        }
        let mut text = format!("error: {}\n", error.message);
        if let Some(hint) = &error.hint {
            let _ = writeln!(text, "hint: {hint}");
        }
        write_err(&text);
    }
}

/// Prints a document in its own format, such as a HAR or `OpenAPI` file,
/// without the `schema` wrapper.
pub(crate) fn raw(document: &Value) {
    let text = serde_json::to_string_pretty(document).unwrap_or_default();
    write_out(&format!("{text}\n"));
}

/// Prints a notice for people to stderr, such as the `shell` banner.
pub(crate) fn notice(text: &str) {
    write_err(text);
}

/// Prints the daemon's single readiness line for `proxy start`.
pub(crate) fn ready_line(line: &Value) {
    write_out(&format!("{line}\n"));
}

fn try_write_out(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

fn write_out(text: &str) {
    // A closed pipe, as with `| head`, is not an error of the command.
    let _closed = try_write_out(text).is_err();
}

fn write_err(text: &str) {
    let mut stderr = io::stderr().lock();
    let _closed = stderr.write_all(text.as_bytes()).is_err();
}

/// The value at `path` below `value`, or `null`.
fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .unwrap_or(&Value::Null)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

fn millis(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| "-".to_owned(), |millis| format!("{millis:.1} ms"))
}

/// One record as a table row, from a list row or a full summary.
fn row(record: &Value) -> String {
    let status = record
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            record
                .get("status")
                .and_then(Value::as_u64)
                .map(|code| code.to_string())
        })
        .unwrap_or_else(|| text(&record["outcome"]));
    let size = record
        .get("response_bytes")
        .or_else(|| record.get("received_bytes"))
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |bytes| format!("{bytes} B"));
    format!(
        "{:>5}  {:<7} {:<24} {:>9} {:>10}  {}",
        text(&record["id"]),
        text(&record["method"]),
        status,
        size,
        millis(&record["duration_ms"]),
        text(&record["url"]),
    )
}

/// Text for stdout and stderr.
fn render(kind: Kind, data: &Value) -> (String, String) {
    match kind {
        Kind::Status => (status(data), String::new()),
        Kind::Stopped => (
            "Stopped the proxy and the daemon\n".to_owned(),
            String::new(),
        ),
        Kind::List => (list(data), String::new()),
        Kind::Show => (show(data), String::new()),
        Kind::Body => body(data),
        Kind::Wait => (format!("{}\n", row(&data["record"])), String::new()),
        Kind::Cleared => (
            format!("Cleared {} records\n", text(&data["removed"])),
            String::new(),
        ),
        Kind::LogSaved => (
            format!("Saved the session log to {}\n", text(&data["path"])),
            String::new(),
        ),
        Kind::Schema => (format!("{data}\n"), String::new()),
        Kind::Env => (env(data), String::new()),
        Kind::CaInfo => (ca_info(data), String::new()),
        Kind::Replay => replay(data),
        Kind::Curl => curl(data),
        Kind::Exported => (
            format!(
                "Wrote {} entries to {}\n",
                text(&data["entries"]),
                text(&data["path"])
            ),
            String::new(),
        ),
        Kind::Endpoints => (endpoints(data), String::new()),
        Kind::Hls => (hls(data), String::new()),
    }
}

/// One line per endpoint: method, path pattern, and observed responses.
fn endpoints(data: &Value) -> String {
    let mut out = String::new();
    for endpoint in data["endpoints"].as_array().into_iter().flatten() {
        let statuses: Vec<String> = endpoint["responses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|response| {
                format!(
                    "{} {}",
                    text(&response["status"]),
                    text(&response["content_type"])
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "{:<7} {}  ({} requests; {})",
            text(&endpoint["method"]),
            text(&endpoint["url"]),
            text(&endpoint["requests"]),
            statuses.join(", ")
        );
    }
    let _ = writeln!(
        out,
        "{} records analyzed, {} still in flight",
        text(&data["records_analyzed"]),
        text(&data["records_in_flight"])
    );
    out
}

fn hls(data: &Value) -> String {
    let mut out = format!("{}\n", text(&data["url"]));
    for variant in data["variants"].as_array().into_iter().flatten() {
        let default = if variant["index"] == data["default_variant"] {
            "*"
        } else {
            " "
        };
        let state = variant["blocked"]
            .as_str()
            .map_or_else(|| text(&variant["state"]), str::to_owned);
        let _ = writeln!(
            out,
            "{default}{:>2}  {:<10} {:<12} {}",
            text(&variant["index"]),
            text(&variant["quality"]),
            text(&variant["bandwidth"]),
            state
        );
    }
    out
}

/// `export` lines to `eval`, with notes as shell comments.
fn env(data: &Value) -> String {
    let mut out = String::new();
    for note in data["notes"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "# {}", text(note));
    }
    for variable in data["variables"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "export {}={}",
            text(&variable["name"]),
            crate::local::shell_quote(&text(&variable["value"]))
        );
    }
    out
}

fn ca_info(data: &Value) -> String {
    format!(
        "certificate  {}\nsha256       {}\nvalid        {} to {}\n",
        text(&data["certificate_path"]),
        text(&data["fingerprint_sha256"]),
        text(&data["valid_from"]),
        text(&data["valid_until"]),
    )
}

fn replay(data: &Value) -> (String, String) {
    if data["dry_run"].as_bool() == Some(true) {
        let request = &data["request"];
        let note = if data["needs_confirmation"].as_bool() == Some(true) {
            "note: sending this request needs --yes\n".to_owned()
        } else {
            String::new()
        };
        let mut out = format!("{} {}\n", text(&request["method"]), text(&request["url"]));
        for header in request["headers"].as_array().into_iter().flatten() {
            let _ = writeln!(out, "{}: {}", text(&header["name"]), text(&header["value"]));
        }
        match request["body"].as_str() {
            Some("") | None if request["body_bytes"].as_u64() == Some(0) => {}
            Some(body) => {
                let _ = write!(out, "\n{body}\n");
            }
            None => {
                let _ = writeln!(
                    out,
                    "\n({} bytes of binary body)",
                    text(&request["body_bytes"])
                );
            }
        }
        return (out, note);
    }
    let out = format!(
        "Replayed record {} as record {}\n{}\n",
        text(&data["replay_of"]),
        text(at(data, &["record", "id"])),
        row(&data["record"])
    );
    (out, String::new())
}

fn curl(data: &Value) -> (String, String) {
    let redacted: Vec<String> = data["redacted_headers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    let note = if redacted.is_empty() {
        String::new()
    } else if data["placeholders"].as_bool() == Some(true) {
        format!(
            "note: replace the <REDACTED> values of {} before running\n",
            redacted.join(", ")
        )
    } else {
        format!(
            "note: redacted headers were left out: {}\n",
            redacted.join(", ")
        )
    };
    (format!("{}\n", text(&data["command"])), note)
}

fn status(data: &Value) -> String {
    let mut out = String::new();
    if data["running"].as_bool() == Some(true) {
        let _ = writeln!(out, "proxy    running on {}", text(&data["address"]));
    } else {
        let _ = writeln!(out, "proxy    stopped");
        if let Some(error) = data["proxy_error"].as_str() {
            let _ = writeln!(out, "error    {error}");
        }
    }
    if data["already_running"].as_bool() == Some(true) {
        let _ = writeln!(out, "note     the daemon was already running");
    }
    let https = if data["https_inspection"].as_bool() == Some(true) {
        "on"
    } else {
        "off"
    };
    let _ = writeln!(out, "https    {https}");
    let _ = writeln!(
        out,
        "records  {} of {} retained · {} dropped · {} evicted",
        text(&data["records"]),
        text(&data["record_limit"]),
        text(&data["records_dropped"]),
        text(&data["records_evicted"]),
    );
    let log = &data["session_log"];
    let log_state = match (log["available"].as_bool(), log["error"].as_str()) {
        (Some(true), None) if log["body_limit_reached"].as_bool() == Some(true) => {
            "body limit reached".to_owned()
        }
        (Some(true), None) => "recording".to_owned(),
        (_, Some(error)) => error.to_owned(),
        _ => "unavailable".to_owned(),
    };
    let _ = writeln!(out, "log      {log_state}");
    let _ = writeln!(out, "socket   {}", text(&data["control_socket"]));
    out
}

fn list(data: &Value) -> String {
    let mut out = format!(
        "{:>5}  {:<7} {:<24} {:>9} {:>10}  {}\n",
        "ID", "METHOD", "STATUS", "SIZE", "TIME", "URL"
    );
    for record in data["records"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "{}", row(record));
    }
    let _ = writeln!(
        out,
        "{} matched of {} retained",
        text(&data["matched"]),
        text(&data["retained"])
    );
    out
}

fn headers(out: &mut String, title: &str, headers: &Value) {
    let Some(headers) = headers.as_array().filter(|headers| !headers.is_empty()) else {
        return;
    };
    let _ = writeln!(out, "\n{title}");
    for header in headers {
        let _ = writeln!(
            out,
            "  {}: {}",
            text(&header["name"]),
            text(&header["value"])
        );
    }
}

fn body_line(out: &mut String, side: &str, body: &Value) {
    if body.is_null() {
        return;
    }
    let _ = writeln!(
        out,
        "  {side}: {} bytes · {} · {}",
        text(&body["observed_bytes"]),
        text(&body["lifecycle"]),
        text(&body["media_type"]),
    );
}

fn show(data: &Value) -> String {
    let mut out = format!("{} {}\n", text(&data["method"]), text(&data["url"]));
    let response = &data["response"];
    let result = response
        .get("line")
        .map_or_else(|| text(&data["summary"]), text);
    let _ = writeln!(out, "{result}");
    if let Some(query) = at(data, &["request", "query"])
        .as_array()
        .filter(|query| !query.is_empty())
    {
        let _ = writeln!(out, "\nQuery");
        for pair in query {
            let _ = writeln!(out, "  {}: {}", text(&pair["name"]), text(&pair["value"]));
        }
    }
    headers(
        &mut out,
        "Request headers",
        at(data, &["request", "headers"]),
    );
    headers(&mut out, "Response headers", &response["headers"]);
    if let Some(tunnel) = data.get("tunnel") {
        let _ = writeln!(
            out,
            "\nTunnel\n  {} · {} bytes up · {} bytes down · {}",
            text(&tunnel["state"]),
            text(&tunnel["client_to_upstream_bytes"]),
            text(&tunnel["upstream_to_client_bytes"]),
            millis(&tunnel["duration_ms"]),
        );
    }
    let timing = &data["timing"];
    let _ = writeln!(out, "\nTiming");
    for (label, key) in [
        ("Connect", "connect_ms"),
        ("Send", "send_ms"),
        ("Wait", "wait_ms"),
        ("Receive", "receive_ms"),
    ] {
        let _ = writeln!(out, "  {label:<17} {}", millis(&timing[key]));
    }
    let _ = writeln!(
        out,
        "  {:<17} {}",
        text(&timing["settled_label"]),
        millis(&timing["settled_ms"])
    );
    let _ = writeln!(out, "  {:<17} {}", "Total", millis(&timing["total_ms"]));
    let _ = writeln!(out, "\nBodies");
    body_line(&mut out, "request", at(data, &["request", "body"]));
    body_line(&mut out, "response", &response["body"]);
    out
}

fn body(data: &Value) -> (String, String) {
    let mut notes = String::new();
    for notice in data["notices"].as_array().into_iter().flatten() {
        let _ = writeln!(notes, "note: {}", text(&notice["text"]));
    }
    if data["truncated"].as_bool() == Some(true) {
        let _ = writeln!(
            notes,
            "note: cut to --max-bytes; the full text is {} bytes",
            text(&data["text_bytes"])
        );
    }
    if let Some(nodes) = data["nodes"].as_array() {
        let mut out = String::new();
        for node in nodes {
            let depth = usize::try_from(node["depth"].as_u64().unwrap_or_default()).unwrap_or(0);
            let _ = writeln!(
                out,
                "{}{}    {}",
                "  ".repeat(depth),
                text(&node["label"]),
                text(&node["path"])
            );
        }
        return (out, notes);
    }
    if data["view"] == "image" {
        let summary = at(data, &["image", "summary"])
            .as_str()
            .unwrap_or("no image");
        return (format!("{summary}\n"), notes);
    }
    let mut out = data["text"].as_str().unwrap_or_default().to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_start_with_the_schema_version() {
        let document = with_schema(&json!({ "records": [] }));
        assert_eq!(document, json!({ "schema": 1, "records": [] }));
    }

    #[test]
    fn text_body_sends_notices_to_stderr() {
        let (out, notes) = body(&json!({
            "view": "pretty",
            "text": "{}",
            "notices": [{ "level": "warning", "text": "Still receiving; showing what arrived" }],
            "truncated": false,
        }));
        assert_eq!(out, "{}\n");
        assert_eq!(notes, "note: Still receiving; showing what arrived\n");
    }
}
