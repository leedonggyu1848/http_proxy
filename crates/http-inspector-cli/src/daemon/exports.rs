//! `records export` (HAR), `records endpoints`, and `records hls`: the
//! app's HAR export, Map endpoint structure, and HLS download.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use http_inspector_core::hls::{
    HlsBlockedReason, HlsDownloadConfig, HlsDownloadError, HlsDownloadOutcome, HlsDownloadPhase,
    HlsDownloadProgress, HlsDownloadSummary, HlsSessionPlan, HlsVariantEntry, VariantMediaState,
    inspect_hls_session, start_hls_download,
};
use http_inspector_exchange::Exchange;
use http_inspector_interop::har::{self, HarOptions};
use serde_json::{Value, json};

use super::select::Selection;
use super::{Shared, client_left, write_line};
use crate::endpoints;
use crate::error::{CliError, ErrorCode};
use crate::protocol::{ExportRequest, FilterSpec, HlsRequest, error_line, ok_line};

/// How often a running HLS download reports progress without other news.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Clones the retained records that `filter` selects, oldest first.
fn selected(shared: &Shared, filter: &FilterSpec) -> Result<Vec<Exchange>, CliError> {
    let mut selection = Selection::compile(filter)?;
    let session = shared.current();
    Ok(session
        .records()
        .iter()
        .filter(|record| selection.matches(record))
        .cloned()
        .collect())
}

/// Builds the HAR with the app's exporter and writes it to a new file, or
/// returns it.
pub(super) fn export(shared: &Shared, request: &ExportRequest) -> Result<Value, CliError> {
    let records = selected(shared, &request.filter)?;
    let options = HarOptions {
        include_bodies: request.include_bodies,
        include_sensitive_headers: request.include_redacted_headers,
    };
    let document = har::document(&records, options, SystemTime::now());
    let Some(path) = &request.output else {
        return Ok(json!({ "entries": records.len(), "har": document }));
    };
    if fs::symlink_metadata(path).is_ok() {
        return Err(file_exists(path));
    }
    let written = har::write(path, &document).map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => file_exists(path),
        _ => CliError::io(&format!("Could not write {}", path.display()), &error),
    })?;
    Ok(json!({ "entries": records.len(), "path": written.to_string_lossy() }))
}

/// Endpoint structure of the selected records, or its `OpenAPI` document.
pub(super) fn endpoints(
    shared: &Shared,
    filter: &FilterSpec,
    openapi: bool,
) -> Result<Value, CliError> {
    let records = selected(shared, filter)?;
    let collected = endpoints::collect(records.iter());
    if openapi {
        Ok(json!({ "openapi": collected.to_openapi()? }))
    } else {
        Ok(collected.to_json())
    }
}

fn file_exists(path: &Path) -> CliError {
    CliError::new(
        ErrorCode::FileExists,
        format!("{} already exists", path.display()),
    )
    .with_hint("choose a new file name; existing files are never overwritten")
}

fn hls_error(message: impl Into<String>) -> CliError {
    CliError::new(ErrorCode::HlsUnavailable, message)
}

/// Inspects a playlist record against every retained record, as the app's
/// HLS dialog does.
fn plan(shared: &Shared, request: &HlsRequest) -> Result<HlsSessionPlan, CliError> {
    let session = shared.current();
    let record = session.find(request.id)?.clone();
    let records: Vec<Exchange> = session.records().iter().cloned().collect();
    drop(session);
    if !http_inspector_core::hls::is_hls_playlist_record(&record) {
        return Err(
            hls_error("The record is not a captured 200 OK HLS playlist")
                .with_hint("pick a `.m3u8` record from `http-proxy records list --filter m3u8`"),
        );
    }
    inspect_hls_session(&record, &records, None).map_err(|error| hls_error(error.to_string()))
}

fn variant_json(index: usize, variant: &HlsVariantEntry) -> Value {
    let (state, segments) = match &variant.state {
        VariantMediaState::NotCaptured => ("not_captured", Value::Null),
        VariantMediaState::Failed(_) => ("failed", Value::Null),
        VariantMediaState::Ready(media) => (
            "ready",
            json!({
                "total": media.segments.len(),
                "cached": media.cached_count,
                "missing": media.missing_count,
                "suggested_file_name": media.suggested_file_name,
            }),
        ),
    };
    json!({
        "index": index,
        "quality": variant.quality_label,
        "bandwidth": variant.bandwidth_label,
        "resolution": variant.resolution,
        "codecs": variant.codecs,
        "playlist_url": variant.playlist_url.to_string(),
        "playlist_record": variant.playlist_record_id.map(http_inspector_exchange::RecordId::value),
        "state": state,
        "segments": segments,
        "can_download": variant.can_download(),
        "blocked": variant.blocked_reason().map(|reason| reason.to_string()),
    })
}

/// `records hls ID`: the variants and whether each can be downloaded.
pub(super) fn inspect(shared: &Shared, request: &HlsRequest) -> Result<Value, CliError> {
    let plan = plan(shared, request)?;
    Ok(json!({
        "playlist": plan.source_record_id.value(),
        "url": plan.source_url.to_string(),
        "master": plan.is_master,
        "default_variant": plan.default_variant,
        "variants": plan
            .variants
            .iter()
            .enumerate()
            .map(|(index, variant)| variant_json(index, variant))
            .collect::<Vec<_>>(),
    }))
}

/// `records hls ID --output PATH`: downloads one variant, streaming
/// progress, and ends with `completed` or an error.
pub(super) fn download(
    shared: &Shared,
    request: &HlsRequest,
    output: &Path,
    writer: &mut UnixStream,
) -> std::io::Result<()> {
    let config = match download_config(shared, request, output) {
        Ok(config) => config,
        Err(error) => return write_line(writer, &error_line(&error)),
    };
    let (notify, progressed) = mpsc::sync_channel(1);
    let handle = match start_hls_download(config, move || {
        let _already_pending = notify.try_send(());
    }) {
        Ok(handle) => handle,
        Err(error) => return write_line(writer, &error_line(&download_error(&error))),
    };
    let probe = writer.try_clone()?;
    let mut last = None;
    loop {
        if let Some(outcome) = handle.try_finish() {
            return write_line(writer, &finished(outcome, output));
        }
        let progress = handle.progress();
        let event = progress_json(&progress);
        if last.as_ref() != Some(&event) {
            write_line(writer, &ok_line(&event))?;
            last = Some(event);
        }
        if client_left(&probe) {
            // Dropping the handle cancels the download.
            return Ok(());
        }
        let _woken = progressed.recv_timeout(PROGRESS_INTERVAL);
    }
}

fn download_config(
    shared: &Shared,
    request: &HlsRequest,
    output: &Path,
) -> Result<HlsDownloadConfig, CliError> {
    // `symlink_metadata` does not follow links, so a dangling symlink at the
    // destination counts as an existing file.
    if fs::symlink_metadata(output).is_ok() {
        return Err(file_exists(output));
    }
    let (directory, name) = match (output.parent(), output.file_name()) {
        (Some(directory), Some(name)) if directory.is_dir() => (directory, name),
        _ => {
            return Err(CliError::new(
                ErrorCode::Usage,
                format!(
                    "{} is not a file in an existing directory",
                    output.display()
                ),
            ));
        }
    };
    let plan = plan(shared, request)?;
    let index = request.variant.unwrap_or(plan.default_variant);
    let variant = plan.variants.get(index).ok_or_else(|| {
        CliError::new(
            ErrorCode::Usage,
            format!(
                "The playlist has {} variants; {index} is not one",
                plan.variants.len()
            ),
        )
        .with_hint("list them with `http-proxy records hls ID`")
    })?;
    let mut media = variant
        .media()
        .cloned()
        .ok_or_else(|| blocked(variant.blocked_reason()))?;
    if let Some(reason) = &media.blocked {
        return Err(blocked(Some(reason.clone())));
    }
    // The app's downloader picks a free name in the directory. Give it a
    // hidden staging name; `publish_exact` then moves the file to `output`.
    media.suggested_file_name = format!(".{}.partial", name.to_string_lossy());
    let proxy = shared.current().running_address();
    Ok(HlsDownloadConfig::new(media, PathBuf::from(directory), None).with_proxy_addr(proxy))
}

fn blocked(reason: Option<HlsBlockedReason>) -> CliError {
    hls_error(reason.map_or_else(
        || "This variant cannot be downloaded".to_owned(),
        |reason| reason.to_string(),
    ))
}

fn download_error(error: &HlsDownloadError) -> CliError {
    match error {
        HlsDownloadError::NotReady(reason) => blocked(Some(reason.clone())),
        HlsDownloadError::Io(message) => CliError::new(ErrorCode::Io, message.clone()),
        other => hls_error(other.to_string()),
    }
}

fn progress_json(progress: &HlsDownloadProgress) -> Value {
    json!({
        "event": "progress",
        "phase": match progress.phase {
            HlsDownloadPhase::Downloading => "downloading",
            HlsDownloadPhase::Merging => "merging",
            HlsDownloadPhase::Finished => "finished",
        },
        "total_segments": progress.total_segments,
        "completed_segments": progress.completed_segments,
        "reused_segments": progress.reused_segments,
        "downloaded_segments": progress.downloaded_segments,
        "downloaded_bytes": progress.downloaded_bytes,
    })
}

/// Publishes the staged file at exactly `output` and removes the staging
/// file whatever happens. `hard_link` fails if anything exists at `output`,
/// including a dangling symlink or a file created during the download, so
/// the file is never saved under another name and nothing is overwritten.
fn publish_exact(staged: &Path, output: &Path) -> Result<(), CliError> {
    let linked = fs::hard_link(staged, output);
    let _staging_gone = fs::remove_file(staged).is_err();
    linked.map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => file_exists(output),
        _ => CliError::io(&format!("Could not save {}", output.display()), &error),
    })
}

fn finished(outcome: HlsDownloadOutcome, output: &Path) -> Value {
    match outcome {
        HlsDownloadOutcome::Completed(summary) => {
            if let Err(error) = publish_exact(&summary.path, output) {
                return error_line(&error);
            }
            completed(&summary, output)
        }
        HlsDownloadOutcome::Cancelled => error_line(&hls_error("The download was cancelled")),
        HlsDownloadOutcome::Failed(error) => error_line(&download_error(&error)),
    }
}

fn completed(summary: &HlsDownloadSummary, output: &Path) -> Value {
    ok_line(&json!({
            "event": "completed",
            "path": output.to_string_lossy(),
            "total_bytes": summary.total_bytes,
            "total_segments": summary.total_segments,
            "reused_segments": summary.reused_segments,
            "downloaded_segments": summary.downloaded_segments,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that appears at the destination during the download wins:
    /// the result is `file_exists`, and no staging file is left behind.
    #[test]
    fn publishing_never_renames_or_overwrites() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("video.ts");
        let staged = directory.path().join(".video.ts.partial");
        fs::write(&staged, b"media")?;
        fs::write(&output, b"someone else's file")?;
        let error = publish_exact(&staged, &output)
            .err()
            .map(|error| error.code);
        assert_eq!(error, Some(ErrorCode::FileExists));
        assert_eq!(fs::read(&output)?, b"someone else's file");
        assert!(!staged.exists());

        let dangling = directory.path().join("dangling.ts");
        std::os::unix::fs::symlink(directory.path().join("missing"), &dangling)?;
        fs::write(&staged, b"media")?;
        let error = publish_exact(&staged, &dangling)
            .err()
            .map(|error| error.code);
        assert_eq!(error, Some(ErrorCode::FileExists));
        assert!(!staged.exists());
        assert!(!directory.path().join("missing").exists());

        let fresh = directory.path().join("fresh.ts");
        fs::write(&staged, b"media")?;
        publish_exact(&staged, &fresh)?;
        assert_eq!(fs::read(&fresh)?, b"media");
        assert!(!staged.exists());
        let mut names: Vec<_> = fs::read_dir(directory.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["dangling.ts", "fresh.ts", "video.ts"]);
        Ok(())
    }
}
