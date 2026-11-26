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

/// One record as a table row.
fn row(record: &Value) -> String {
    format!(
        "{:>5}  {:<7} {:<24} {:<6} {:>9} {:>10}  {}",
        text(&record["id"]),
        text(&record["method"]),
        text(&record["summary"]),
        text(&record["type"]),
        record["received_bytes"]
            .as_u64()
            .map_or_else(|| "-".to_owned(), |bytes| format!("{bytes} B")),
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
    }
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
        "{:>5}  {:<7} {:<24} {:<6} {:>9} {:>10}  {}\n",
        "ID", "METHOD", "STATUS", "TYPE", "SIZE", "TIME", "URL"
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
