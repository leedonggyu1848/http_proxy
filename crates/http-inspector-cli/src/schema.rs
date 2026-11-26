//! `http-proxy schema`: every command, argument, output document, error
//! code, and exit code in one JSON document, so an agent can learn the
//! interface from a single call. Commands and arguments are read from the
//! parser itself and cannot drift from it.

use clap::CommandFactory as _;
use serde_json::{Value, json};

use crate::args::Cli;
use crate::error::ErrorCode;
use crate::protocol::SCHEMA_VERSION;

/// Exit codes and their meaning.
pub(crate) const EXIT_CODES: [(u8, &str); 7] = [
    (0, "success"),
    (1, "the command failed; see error.code"),
    (2, "usage error or invalid filter"),
    (3, "the daemon is not running"),
    (4, "the record, body, or tree path does not exist"),
    (6, "records wait timed out"),
    (75, "temporarily busy; retry shortly"),
];

/// Output documents per command. Every document also has `"schema": 1`.
fn record_fields() -> Value {
    json!({
        "id": "integer, stable record ID",
        "method": "string",
        "url": "string, request target as received",
        "host": "string",
        "path": "string",
        "outcome": "pending | response | tunnel | https_inspection | failed | cancelled | incomplete",
        "status": "integer or null",
        "summary": "string, the app's status text such as `404 Not Found`",
        "type": "string or null, short response media type",
        "received_bytes": "integer or null",
        "duration_ms": "number",
        "settled": "boolean, nothing about the record changes any more",
        "source": "http_proxy | connect | inspected_https",
        "replay_of": "integer or null",
        "started_at_ms": "integer, Unix time in milliseconds",
    })
}

fn show_fields() -> Value {
    json!({
        "...": "every records list field",
        "request": {"method": "string", "target": "string",
            "headers": [{"name": "string", "value": "string, [REDACTED] for credentials", "redacted": "boolean"}],
            "headers_omitted": "boolean", "auth_schemes": ["string"],
            "query": [{"name": "string", "value": "string"}], "body": "body summary or null"},
        "response": {"status": "integer", "reason": "string", "version": "string",
            "line": "string", "status_class": "string", "headers": "as request",
            "headers_omitted": "boolean", "body": "body summary"},
        "failure": {"stage": "string", "message": "string"},
        "tunnel": {"state": "open | closed | failed | incomplete", "failure": "object or null",
            "client_to_upstream_bytes": "integer", "upstream_to_client_bytes": "integer",
            "duration_ms": "number"},
        "timing": {"connect_ms": "number or null", "send_ms": "number or null",
            "wait_ms": "number or null", "receive_ms": "number or null",
            "settled_label": "string", "settled_ms": "number or null", "total_ms": "number or null"},
        "body summary": {"observed_bytes": "integer", "retained_bytes": "integer or null",
            "lifecycle": "streaming | complete | interrupted | cancelled",
            "capture": "whole | truncated | streaming | ended",
            "media_type": "string or null", "content_encoding": ["string"]},
    })
}

fn outputs() -> Value {
    let record = record_fields();
    json!({
        "proxy start": {
            "same as": "proxy status",
            "already_running": "boolean, true when a daemon already served the proxy",
        },
        "proxy status": {
            "running": "boolean", "address": "string or null, such as 127.0.0.1:8080",
            "port": "integer or null", "https_inspection": "boolean",
            "proxy_error": "string or null", "pid": "integer",
            "records": "integer", "record_limit": "integer",
            "records_dropped": "integer", "records_evicted": "integer",
            "session_log": {"available": "boolean", "error": "string or null",
                "body_limit_bytes": "integer", "body_limit_reached": "boolean"},
            "control_socket": "string",
        },
        "proxy set": { "same as": "proxy status" },
        "proxy stop": { "stopped": "boolean", "records": "integer, discarded records" },
        "records list": {
            "records": [record], "matched": "integer", "retained": "integer",
        },
        "records show": show_fields(),
        "records body": {
            "id": "integer", "side": "request | response",
            "view": "pretty | raw | tree | hex | image", "format": "string such as JSON",
            "media_type": "string or null", "content_encoding": ["string"],
            "observed_bytes": "integer", "retained_bytes": "integer",
            "capture": "whole | truncated | streaming | ended",
            "notices": [{"level": "info | warning", "text": "string"}],
            "text": "string (pretty, raw, hex, or a selected tree path)",
            "text_bytes": "integer, length before --max-bytes",
            "truncated": "boolean",
            "path": "string, with --json-path or --xpath",
            "nodes": [{"depth": "integer", "label": "string", "path": "string or null"}],
            "image": {"format": "string", "width": "integer", "height": "integer",
                "file_bytes": "integer", "notes": ["string"], "summary": "string"},
        },
        "records watch": {
            "NDJSON lines": "one document per line",
            "event": "started | added | updated | cleared | daemon_stopped",
            "since": "integer, on started",
            "record": "records list record, on added and updated",
        },
        "records wait": { "record": "records list record" },
        "records clear": { "removed": "integer" },
        "log save": { "path": "string" },
        "error": {"error": {"code": "string, see error_codes", "message": "string", "hint": "string or null"}},
    })
}

fn command_json(command: &clap::Command, prefix: &str) -> Vec<Value> {
    command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .flat_map(|sub| {
            let name = if prefix.is_empty() {
                sub.get_name().to_owned()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if sub.has_subcommands() {
                return command_json(sub, &name);
            }
            let arguments: Vec<Value> = sub
                .get_arguments()
                .filter(|arg| {
                    !arg.is_hide_set() && arg.get_id() != "help" && arg.get_id() != "json"
                })
                .map(|arg| {
                    json!({
                        "name": arg.get_long().map_or_else(
                            || arg.get_id().to_string(),
                            |long| format!("--{long}"),
                        ),
                        "help": arg.get_help().map(ToString::to_string),
                        "required": arg.is_required_set(),
                        "values": arg
                            .get_possible_values()
                            .iter()
                            .map(|value| value.get_name().to_owned())
                            .collect::<Vec<_>>(),
                        "default": arg
                            .get_default_values()
                            .iter()
                            .map(|value| value.to_string_lossy().into_owned())
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            vec![json!({
                "command": name,
                "about": sub.get_about().map(ToString::to_string),
                "arguments": arguments,
            })]
        })
        .collect()
}

/// The `http-proxy schema` document.
pub(crate) fn document() -> Value {
    let cli = Cli::command();
    json!({
        "schema": SCHEMA_VERSION,
        "program": "http-proxy",
        "global_arguments": [{"name": "--json", "help": "print JSON documents (NDJSON for records watch)"}],
        "commands": command_json(&cli, ""),
        "outputs": outputs(),
        "error_codes": ErrorCode::ALL
            .iter()
            .map(|code| json!({ "code": code.name(), "exit_code": code.exit_code() }))
            .collect::<Vec<_>>(),
        "exit_codes": EXIT_CODES
            .iter()
            .map(|(code, meaning)| json!({ "code": code, "meaning": meaning }))
            .collect::<Vec<_>>(),
    })
}

/// Command paths, such as `records body`, for documentation checks.
#[cfg(test)]
fn command_names() -> Vec<String> {
    command_json(&Cli::command(), "")
        .iter()
        .filter_map(|command| command["command"].as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE: &str = include_str!("../../../docs/cli.md");

    #[test]
    fn lists_every_command_and_code() {
        let document = document();
        let names = command_names();
        for expected in [
            "proxy start",
            "proxy stop",
            "proxy status",
            "proxy set",
            "records list",
            "records show",
            "records body",
            "records watch",
            "records wait",
            "records clear",
            "log save",
            "schema",
        ] {
            assert!(names.iter().any(|name| name == expected), "{expected}");
            assert!(
                document
                    .get("outputs")
                    .and_then(|outputs| outputs.get(expected))
                    .is_some()
                    || expected == "schema",
                "{expected} has no output description"
            );
        }
        assert!(!names.iter().any(|name| name.contains("__daemon")));
        for code in ErrorCode::ALL {
            assert!(
                EXIT_CODES.iter().any(|(exit, _)| *exit == code.exit_code()),
                "{code:?}"
            );
        }
    }

    /// docs/cli.md documents every command, error code, and exit code.
    #[test]
    fn the_guide_covers_the_interface() {
        for name in command_names() {
            assert!(
                GUIDE.contains(&format!("http-proxy {name}")),
                "docs/cli.md lacks {name}"
            );
        }
        for code in ErrorCode::ALL {
            assert!(
                GUIDE.contains(&format!("`{}`", code.name())),
                "docs/cli.md lacks {code:?}"
            );
        }
        for (exit, _) in EXIT_CODES {
            assert!(
                GUIDE.contains(&format!("| `{exit}` |")),
                "docs/cli.md lacks exit {exit}"
            );
        }
    }
}
