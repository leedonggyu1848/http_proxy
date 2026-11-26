//! `records replay` and `records curl`: reproduce a captured request with
//! the same rules as the app's **Edit and Resend** and **Copy as cURL**.

use std::time::{Duration, Instant};

use http_inspector_core::replay::{
    ReplayBodyFileError, ReplayDraft, ReplayError, ReplayField, ReplayHeaderError,
};
use http_inspector_exchange::http_semantics::{
    REDACTED_VALUE, is_sensitive_header, is_sensitive_request_header,
};
use http_inspector_exchange::{Exchange, RecordId};
use http_inspector_interop::curl::{RedactedHeaders, curl_command};
use serde_json::{Value, json};

use super::session::Session;
use super::{Shared, TICK};
use crate::error::{CliError, ErrorCode};
use crate::protocol::{RecordRef, ReplayRequest};
use crate::view;

/// Body text shown by `--dry-run`, so a large upload does not flood output.
const PREVIEW_BODY_BYTES: usize = 64 * 1024;

/// Validates, optionally sends, and reports one replay.
pub(super) fn replay(shared: &Shared, request: &ReplayRequest) -> Result<Value, CliError> {
    let (original, proxy) = {
        let session = shared.current();
        (session.find(request.id)?.clone(), session.proxy_address())
    };
    let draft = edited_draft(original, request)?;
    let built = draft
        .request(proxy)
        .map_err(|error| replay_error(&error, &draft))?;
    let preview = preview(&built, &draft.body);
    let original_id = draft.original.id();
    let needs_confirmation = draft.needs_confirmation();
    let mut document = json!({
        "replay_of": original_id.value(),
        "dry_run": request.dry_run,
        "needs_confirmation": needs_confirmation,
        "request": preview,
    });
    if request.dry_run {
        return Ok(document);
    }
    if needs_confirmation && !request.yes {
        return Err(CliError::new(
            ErrorCode::ConfirmationRequired,
            format!(
                "{} may change server state and needs confirmation",
                draft.method
            ),
        )
        .with_hint("check the request with `--dry-run`, then repeat with `--yes`"));
    }
    let baseline = {
        let session = shared.lock();
        let baseline = session.newest_id();
        session
            .replay(&draft)
            .map_err(|error| replay_error(&error, &draft))?;
        baseline
    };
    let record = wait_for_replay(shared, original_id, baseline, request.timeout)?;
    if let Some(document) = document.as_object_mut() {
        document.insert("record".to_owned(), record);
    }
    Ok(document)
}

/// Applies the command line changes to a draft of the captured request.
fn edited_draft(original: Exchange, request: &ReplayRequest) -> Result<ReplayDraft, CliError> {
    let mut draft = ReplayDraft::new(original).map_err(|error| not_reproducible(&error))?;
    if let Some(method) = &request.method {
        draft.method.clone_from(method);
    }
    if let Some(url) = &request.url {
        draft.url.clone_from(url);
    }
    let disable = |draft: &mut ReplayDraft, name: &str| {
        for field in &mut draft.headers {
            if field.name.eq_ignore_ascii_case(name) {
                field.enabled = false;
            }
        }
    };
    for name in &request.remove_headers {
        disable(&mut draft, name);
    }
    for (name, value) in &request.headers {
        disable(&mut draft, name);
        if name.eq_ignore_ascii_case("host") {
            draft.automatic_host = false;
        }
        draft.headers.push(ReplayField {
            enabled: true,
            name: name.clone(),
            value: value.clone(),
        });
    }
    if let Some(path) = &request.body_file {
        draft.load_body_file(path).map_err(|error| match error {
            ReplayBodyFileError::Io(error) => {
                CliError::io(&format!("Could not read {}", path.display()), &error)
            }
            ReplayBodyFileError::TooLarge => not_reproducible(&error),
        })?;
    }
    Ok(draft)
}

/// The request as it will be sent. Credentials are shown as `[REDACTED]`,
/// as everywhere else in the output.
fn preview<B>(built: &http::Request<B>, body: &[u8]) -> Value {
    let headers: Vec<Value> = built
        .headers()
        .iter()
        .map(|(name, value)| {
            let redacted = is_sensitive_request_header(name.as_str());
            json!({
                "name": name.as_str(),
                "value": if redacted {
                    REDACTED_VALUE.to_owned()
                } else {
                    String::from_utf8_lossy(value.as_bytes()).into_owned()
                },
                "redacted": redacted,
            })
        })
        .collect();
    let (shown, _) = body.split_at(body.len().min(PREVIEW_BODY_BYTES));
    json!({
        "method": built.method().as_str(),
        "url": built.uri().to_string(),
        "headers": headers,
        "body_bytes": body.len(),
        "body": std::str::from_utf8(shown).ok(),
        "body_truncated": shown.len() < body.len(),
    })
}

/// Waits for the replayed exchange to appear and finish.
fn wait_for_replay(
    shared: &Shared,
    original: RecordId,
    baseline: u64,
    timeout: Duration,
) -> Result<Value, CliError> {
    let deadline = Instant::now() + timeout;
    let mut session = shared.lock();
    loop {
        let found = session
            .records()
            .iter()
            .find(|record| record.id().value() > baseline && record.replay_of() == Some(original));
        let now = Instant::now();
        match found {
            Some(record)
                if http_inspector_analysis::search::is_settled(record) || now >= deadline =>
            {
                return Ok(view::summary(record));
            }
            None if now >= deadline => {
                return Err(CliError::new(
                    ErrorCode::Timeout,
                    "The replayed request was sent, but its record did not appear in time",
                )
                .with_hint("look for it with `http-proxy records list --full`"));
            }
            _ => {}
        }
        session = shared.wait(session, deadline.saturating_duration_since(now).min(TICK));
    }
}

/// `records curl`: the request as a POSIX shell `curl` command.
pub(super) fn curl(
    session: &Session,
    id: RecordRef,
    placeholders: bool,
) -> Result<Value, CliError> {
    let record = session.find(id)?;
    let mode = if placeholders {
        RedactedHeaders::Placeholder
    } else {
        RedactedHeaders::Omit
    };
    let command = curl_command(record, mode).map_err(|error| not_reproducible(&error))?;
    Ok(json!({
        "id": record.id().value(),
        "command": command.text(),
        "redacted_headers": command.redacted_headers(),
        "placeholders": command.has_placeholders(),
    }))
}

fn not_reproducible(error: &dyn std::fmt::Display) -> CliError {
    CliError::new(ErrorCode::NotReproducible, error.to_string())
}

/// Maps a replay failure to a code and a next step on the command line.
fn replay_error(error: &ReplayError, draft: &ReplayDraft) -> CliError {
    match error {
        ReplayError::ProxyNotRunning => {
            CliError::new(ErrorCode::ProxyNotRunning, error.to_string())
                .with_hint("run `http-proxy proxy start`")
        }
        ReplayError::HttpsInspectionDisabled => {
            CliError::new(ErrorCode::HttpsUnavailable, error.to_string())
                .with_hint("run `http-proxy proxy set --https-inspection on`")
        }
        ReplayError::IncompleteCapture => CliError::new(
            ErrorCode::NotReproducible,
            "The captured body or headers are incomplete",
        )
        .with_hint("pass the complete body with `--body-file PATH`"),
        ReplayError::Header(ReplayHeaderError::RedactedValue) => {
            let names: Vec<&str> = draft
                .headers
                .iter()
                .filter(|field| {
                    field.enabled
                        && (field.value == REDACTED_VALUE
                            || (field.value.is_empty() && is_sensitive_header(&field.name)))
                })
                .map(|field| field.name.as_str())
                .collect();
            CliError::new(
                ErrorCode::NotReproducible,
                format!(
                    "Redacted headers have no stored value: {}",
                    names.join(", ")
                ),
            )
            .with_hint("pass `--header 'Name: value'` for each, or `--remove-header Name`")
        }
        _ => not_reproducible(error),
    }
}
