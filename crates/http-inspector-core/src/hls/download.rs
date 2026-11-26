//! Concurrent HLS segment downloader, cache reuser, and playlist-order `.ts` merger.

use std::fs::File;
use std::io::{self, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

use http_inspector_encoding::{EmptyTokens, Encodings};
use http_inspector_exchange::{BodySide, Outcome};
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use super::fetch::{FetchError, SegmentFetcher, fetch_segment_to_file};
use super::headers::SafeRequestHeaders;
use super::plan::{CachedSegment, HlsBlockedReason, MediaDownloadPlan, SegmentPlan, SegmentSource};
use super::url::HlsUrl;
use crate::recording::{BodyStore, body_store};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const MAX_ALLOWED_CONCURRENCY: usize = 8;
const BROTLI_BUFFER: usize = 64 * 1024;
const MAX_DECODED_CACHED_BYTES: u64 = 512 * 1024 * 1024;

/// Parameters required to start an HLS media download.
#[derive(Clone, Debug)]
pub struct HlsDownloadConfig {
    pub plan: MediaDownloadPlan,
    pub output_directory: PathBuf,
    pub body_store: Option<BodyStore>,
    pub additional_upstream_root: Option<PathBuf>,
    pub proxy_addr: Option<SocketAddr>,
    pub max_concurrency: usize,
}

impl HlsDownloadConfig {
    #[must_use]
    pub const fn new(
        plan: MediaDownloadPlan,
        output_directory: PathBuf,
        body_store: Option<BodyStore>,
    ) -> Self {
        Self {
            plan,
            output_directory,
            body_store,
            additional_upstream_root: None,
            proxy_addr: None,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
        }
    }

    #[must_use]
    pub const fn with_proxy_addr(mut self, proxy_addr: Option<SocketAddr>) -> Self {
        self.proxy_addr = proxy_addr;
        self
    }

    #[must_use]
    pub fn with_additional_upstream_root(mut self, path: Option<PathBuf>) -> Self {
        self.additional_upstream_root = path;
        self
    }
}

/// High-level execution phase of an HLS download job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HlsDownloadPhase {
    Downloading,
    Merging,
    Finished,
}

/// Per-segment runtime status displayed in the UI table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SegmentStatus {
    Cached,
    Queued,
    Downloading,
    Done,
    Failed {
        status: Option<u16>,
        message: String,
    },
    Cancelled,
}

/// Non-blocking progress snapshot of a running HLS download.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlsDownloadProgress {
    pub phase: HlsDownloadPhase,
    pub total_segments: usize,
    pub completed_segments: usize,
    pub reused_segments: usize,
    pub downloaded_segments: usize,
    pub downloaded_bytes: u64,
    pub cancelled: bool,
    pub segment_statuses: Vec<SegmentStatus>,
}

/// Summary of a successfully merged and published `.ts` file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlsDownloadSummary {
    pub path: PathBuf,
    pub total_bytes: u64,
    pub total_segments: usize,
    pub reused_segments: usize,
    pub downloaded_segments: usize,
}

/// Reason an HLS download failed before producing a complete `.ts` file.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HlsDownloadError {
    #[error("{0}")]
    NotReady(HlsBlockedReason),
    #[error("HTTP {status} {reason} on segment #{segment_number} ({segment_name})")]
    HttpStatus {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
        segment_url: String,
        status: u16,
        reason: String,
    },
    #[error("Too many redirects on segment #{segment_number} ({segment_name})")]
    TooManyRedirects {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
    },
    #[error("Invalid redirect on segment #{segment_number} ({segment_name}): {detail}")]
    InvalidRedirect {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
        detail: String,
    },
    #[error("Network error on segment #{segment_number} ({segment_name}): {detail}")]
    Network {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
        detail: String,
    },
    #[error("Decoding failed on segment #{segment_number} ({segment_name}): {detail}")]
    Decode {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
        detail: String,
    },
    #[error("Cached segment #{segment_number} ({segment_name}) could not be read: {detail}")]
    CacheRead {
        segment_index: usize,
        segment_number: usize,
        segment_name: String,
        detail: String,
    },
    #[error("TLS setup failed: {0}")]
    TlsSetup(String),
    #[error("File write failed: {0}")]
    Io(String),
}

/// Final outcome of an HLS download job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HlsDownloadOutcome {
    Completed(HlsDownloadSummary),
    Cancelled,
    Failed(HlsDownloadError),
}

struct SharedProgress {
    phase: Mutex<HlsDownloadPhase>,
    total_segments: usize,
    reused_segments: usize,
    completed_segments: AtomicUsize,
    downloaded_segments: AtomicUsize,
    downloaded_bytes: AtomicU64,
    cancelled: AtomicBool,
    segment_statuses: Mutex<Vec<SegmentStatus>>,
}

impl SharedProgress {
    fn from_plan(plan: &MediaDownloadPlan) -> Self {
        let segment_statuses: Vec<SegmentStatus> = plan
            .segments
            .iter()
            .map(|seg| {
                if seg.is_cached() {
                    SegmentStatus::Cached
                } else {
                    SegmentStatus::Queued
                }
            })
            .collect();
        Self {
            phase: Mutex::new(HlsDownloadPhase::Downloading),
            total_segments: plan.segments.len(),
            reused_segments: plan.cached_count,
            completed_segments: AtomicUsize::new(plan.cached_count),
            downloaded_segments: AtomicUsize::new(0),
            downloaded_bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            segment_statuses: Mutex::new(segment_statuses),
        }
    }

    fn set_phase(&self, phase: HlsDownloadPhase) {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = phase;
    }

    fn set_segment_status(&self, index: usize, status: SegmentStatus) {
        let mut guard = self
            .segment_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = guard.get_mut(index) {
            *slot = status;
        }
    }

    fn mark_remaining_cancelled(&self) {
        let mut guard = self
            .segment_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for status in guard.iter_mut() {
            if matches!(status, SegmentStatus::Queued | SegmentStatus::Downloading) {
                *status = SegmentStatus::Cancelled;
            }
        }
    }

    fn snapshot(&self) -> HlsDownloadProgress {
        let phase = *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let segment_statuses = self
            .segment_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        HlsDownloadProgress {
            phase,
            total_segments: self.total_segments,
            completed_segments: self.completed_segments.load(Ordering::Acquire),
            reused_segments: self.reused_segments,
            downloaded_segments: self.downloaded_segments.load(Ordering::Acquire),
            downloaded_bytes: self.downloaded_bytes.load(Ordering::Acquire),
            cancelled: self.cancelled.load(Ordering::Acquire),
            segment_statuses,
        }
    }
}

/// Handle to a running background HLS download job.
pub struct HlsDownloadHandle {
    shared: Arc<SharedProgress>,
    cancel_tx: watch::Sender<bool>,
    done: Receiver<HlsDownloadOutcome>,
}

impl HlsDownloadHandle {
    /// Returns a non-blocking snapshot of download progress and per-segment states.
    #[must_use]
    pub fn progress(&self) -> HlsDownloadProgress {
        self.shared.snapshot()
    }

    /// Requests cancellation of any in-flight segment fetches and merge work.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
        let _ = self.cancel_tx.send(true);
    }

    /// Returns the final outcome if the job has finished, or `None` while running.
    #[must_use]
    pub fn try_finish(&self) -> Option<HlsDownloadOutcome> {
        match self.done.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(HlsDownloadOutcome::Failed(
                HlsDownloadError::Io("HLS download worker stopped unexpectedly".to_owned()),
            )),
        }
    }

    /// Blocks until the download finishes and returns its final outcome.
    #[must_use]
    pub fn wait(self) -> HlsDownloadOutcome {
        self.done.recv().unwrap_or_else(|_| {
            HlsDownloadOutcome::Failed(HlsDownloadError::Io(
                "HLS download worker stopped unexpectedly".to_owned(),
            ))
        })
    }
}

impl Drop for HlsDownloadHandle {
    fn drop(&mut self) {
        // Dropping the UI handle cancels any still-running background fetch.
        self.cancel();
    }
}

/// Starts downloading missing segments and merging all segments in playlist order into one `.ts` file.
pub fn start_hls_download(
    config: HlsDownloadConfig,
    notify: impl Fn() + Send + Sync + 'static,
) -> Result<HlsDownloadHandle, HlsDownloadError> {
    if let Some(reason) = config.plan.blocked.clone() {
        return Err(HlsDownloadError::NotReady(reason));
    }
    if config.plan.segments.is_empty() {
        return Err(HlsDownloadError::NotReady(HlsBlockedReason::NoSegments));
    }

    let shared = Arc::new(SharedProgress::from_plan(&config.plan));
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (done_tx, done_rx) = mpsc::channel();
    let worker_shared = Arc::clone(&shared);
    let worker_cancel_tx = cancel_tx.clone();
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);

    thread::Builder::new()
        .name("hls-download".to_owned())
        .spawn(move || {
            let outcome = run_download_job(
                &config,
                &worker_shared,
                &worker_cancel_tx,
                cancel_rx,
                &notify,
            );
            worker_shared.set_phase(HlsDownloadPhase::Finished);
            let _ = done_tx.send(outcome);
            notify();
        })
        .map_err(|error| {
            HlsDownloadError::Io(format!("failed to start download thread: {error}"))
        })?;

    Ok(HlsDownloadHandle {
        shared,
        cancel_tx,
        done: done_rx,
    })
}

fn run_download_job(
    config: &HlsDownloadConfig,
    shared: &Arc<SharedProgress>,
    cancel_tx: &watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> HlsDownloadOutcome {
    if !config.output_directory.is_dir()
        && let Err(error) = std::fs::create_dir_all(&config.output_directory)
    {
        return HlsDownloadOutcome::Failed(HlsDownloadError::Io(format!(
            "could not create output directory: {error}"
        )));
    }
    let work_dir = match tempfile::tempdir_in(&config.output_directory) {
        Ok(dir) => dir,
        Err(error) => {
            return HlsDownloadOutcome::Failed(HlsDownloadError::Io(format!(
                "could not create temporary workspace: {error}"
            )));
        }
    };

    if let Some(early_outcome) =
        download_missing_segments(config, &work_dir, shared, cancel_tx, cancel_rx, notify)
    {
        return early_outcome;
    }

    if shared.cancelled.load(Ordering::Acquire) {
        shared.mark_remaining_cancelled();
        return HlsDownloadOutcome::Cancelled;
    }

    shared.set_phase(HlsDownloadPhase::Merging);
    notify();

    merge_segments_in_order(config, &work_dir, shared)
}

struct FetchJob {
    index: usize,
    name: String,
    url: HlsUrl,
    headers: SafeRequestHeaders,
    temp_path: PathBuf,
}

struct FetchRunnerContext {
    concurrency: usize,
    base_fetcher: SegmentFetcher,
    shared: Arc<SharedProgress>,
    cancel_tx: watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

fn download_missing_segments(
    config: &HlsDownloadConfig,
    work_dir: &TempDir,
    shared: &Arc<SharedProgress>,
    cancel_tx: &watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> Option<HlsDownloadOutcome> {
    let jobs: Vec<FetchJob> = config
        .plan
        .segments
        .iter()
        .filter_map(|seg| match &seg.source {
            SegmentSource::Cached(_) => None,
            SegmentSource::Fetch { headers } => Some(FetchJob {
                index: seg.segment.index,
                name: segment_display_name(seg),
                url: seg.segment.url.clone(),
                headers: headers.clone(),
                temp_path: segment_temp_path(work_dir.path(), seg.segment.index),
            }),
        })
        .collect();

    if jobs.is_empty() {
        return None;
    }

    let runtime = match RuntimeBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return Some(HlsDownloadOutcome::Failed(HlsDownloadError::Io(format!(
                "failed to create async runtime: {error}"
            ))));
        }
    };

    let ctx = FetchRunnerContext {
        concurrency: config.max_concurrency.clamp(1, MAX_ALLOWED_CONCURRENCY),
        base_fetcher: SegmentFetcher::new(
            config.additional_upstream_root.clone(),
            config.proxy_addr,
        ),
        shared: Arc::clone(shared),
        cancel_tx: cancel_tx.clone(),
        cancel_rx,
        notify: Arc::clone(notify),
    };

    runtime.block_on(run_concurrent_fetches(jobs, ctx))
}

async fn run_concurrent_fetches(
    jobs: Vec<FetchJob>,
    ctx: FetchRunnerContext,
) -> Option<HlsDownloadOutcome> {
    let semaphore = Arc::new(Semaphore::new(ctx.concurrency));
    let mut set = JoinSet::new();

    for job in jobs {
        spawn_segment_job(&mut set, job, &semaphore, &ctx);
    }

    let mut first_error: Option<HlsDownloadError> = None;
    let mut saw_cancel = false;

    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(TaskFailure::Error(error))) => {
                if first_error.is_none() {
                    first_error = Some(error);
                    let _ = ctx.cancel_tx.send(true);
                    set.abort_all();
                }
            }
            Err(join_err) if !join_err.is_cancelled() => {
                if first_error.is_none() {
                    first_error = Some(HlsDownloadError::Io(format!(
                        "segment task panicked: {join_err}"
                    )));
                    let _ = ctx.cancel_tx.send(true);
                    set.abort_all();
                }
            }
            Ok(Err(TaskFailure::Cancelled)) | Err(_) => {
                saw_cancel = true;
            }
        }
    }

    if let Some(error) = first_error {
        ctx.shared.mark_remaining_cancelled();
        return Some(HlsDownloadOutcome::Failed(error));
    }
    if ctx.shared.cancelled.load(Ordering::Acquire) || saw_cancel {
        ctx.shared.mark_remaining_cancelled();
        return Some(HlsDownloadOutcome::Cancelled);
    }
    None
}

fn spawn_segment_job(
    set: &mut JoinSet<Result<(), TaskFailure>>,
    job: FetchJob,
    semaphore: &Arc<Semaphore>,
    ctx: &FetchRunnerContext,
) {
    let permit_sem = Arc::clone(semaphore);
    let mut fetcher = ctx.base_fetcher.clone();
    let mut task_cancel = ctx.cancel_rx.clone();
    let task_shared = Arc::clone(&ctx.shared);
    let task_notify = Arc::clone(&ctx.notify);

    set.spawn(async move {
        let Ok(_permit) = permit_sem.acquire_owned().await else {
            return Err(TaskFailure::Cancelled);
        };
        if *task_cancel.borrow() {
            return Err(TaskFailure::Cancelled);
        }
        task_shared.set_segment_status(job.index, SegmentStatus::Downloading);
        task_notify();

        let bytes_shared = Arc::clone(&task_shared);
        let bytes_notify = Arc::clone(&task_notify);
        let result = fetch_segment_to_file(
            &mut fetcher,
            &job.url,
            &job.headers,
            &job.temp_path,
            &mut task_cancel,
            move |delta| {
                bytes_shared
                    .downloaded_bytes
                    .fetch_add(delta, Ordering::Relaxed);
                bytes_notify();
            },
        )
        .await;

        record_segment_fetch_result(&task_shared, &task_notify, &job, result)
    });
}

fn record_segment_fetch_result(
    shared: &SharedProgress,
    notify: &Arc<dyn Fn() + Send + Sync>,
    job: &FetchJob,
    result: Result<u64, FetchError>,
) -> Result<(), TaskFailure> {
    match result {
        Ok(_) => {
            shared.set_segment_status(job.index, SegmentStatus::Done);
            shared.downloaded_segments.fetch_add(1, Ordering::AcqRel);
            shared.completed_segments.fetch_add(1, Ordering::AcqRel);
            notify();
            Ok(())
        }
        Err(FetchError::Cancelled) => {
            shared.set_segment_status(job.index, SegmentStatus::Cancelled);
            notify();
            Err(TaskFailure::Cancelled)
        }
        Err(error) => {
            let mapped = map_fetch_error(job.index, &job.name, &job.url, error);
            let status_code = match &mapped {
                HlsDownloadError::HttpStatus { status, .. } => Some(*status),
                _ => None,
            };
            shared.set_segment_status(
                job.index,
                SegmentStatus::Failed {
                    status: status_code,
                    message: mapped.to_string(),
                },
            );
            notify();
            Err(TaskFailure::Error(mapped))
        }
    }
}

enum TaskFailure {
    Cancelled,
    Error(HlsDownloadError),
}

fn map_fetch_error(
    segment_index: usize,
    segment_name: &str,
    segment_url: &HlsUrl,
    error: FetchError,
) -> HlsDownloadError {
    let segment_number = segment_index + 1;
    let name = segment_name.to_owned();
    match error {
        FetchError::HttpStatus { status, reason } => HlsDownloadError::HttpStatus {
            segment_index,
            segment_number,
            segment_name: name,
            segment_url: segment_url.as_str().to_owned(),
            status,
            reason,
        },
        FetchError::TooManyRedirects => HlsDownloadError::TooManyRedirects {
            segment_index,
            segment_number,
            segment_name: name,
        },
        FetchError::InvalidRedirect(detail) => HlsDownloadError::InvalidRedirect {
            segment_index,
            segment_number,
            segment_name: name,
            detail,
        },
        FetchError::Network(detail) => HlsDownloadError::Network {
            segment_index,
            segment_number,
            segment_name: name,
            detail,
        },
        FetchError::Decode(detail) => HlsDownloadError::Decode {
            segment_index,
            segment_number,
            segment_name: name,
            detail,
        },
        FetchError::TlsSetup(detail) => HlsDownloadError::TlsSetup(detail),
        FetchError::Io(err) => HlsDownloadError::Io(err.to_string()),
        FetchError::Cancelled => HlsDownloadError::Network {
            segment_index,
            segment_number,
            segment_name: name,
            detail: "cancelled".to_owned(),
        },
    }
}

fn write_segment_bytes(
    seg: &SegmentPlan,
    config: &HlsDownloadConfig,
    work_dir: &TempDir,
    writer: &mut impl io::Write,
) -> Result<(u64, SegmentStatus), HlsDownloadError> {
    match &seg.source {
        SegmentSource::Cached(cached) => {
            let bytes = copy_cached_segment(cached, config.body_store.as_ref(), writer).map_err(
                |error| HlsDownloadError::CacheRead {
                    segment_index: seg.segment.index,
                    segment_number: seg.segment.index + 1,
                    segment_name: segment_display_name(seg),
                    detail: error.to_string(),
                },
            )?;
            Ok((bytes, SegmentStatus::Cached))
        }
        SegmentSource::Fetch { .. } => {
            let path = segment_temp_path(work_dir.path(), seg.segment.index);
            let bytes = copy_file_to(&path, writer).map_err(|error| {
                HlsDownloadError::Io(format!(
                    "missing downloaded segment #{}: {error}",
                    seg.segment.index + 1
                ))
            })?;
            Ok((bytes, SegmentStatus::Done))
        }
    }
}

fn merge_segments_in_order(
    config: &HlsDownloadConfig,
    work_dir: &TempDir,
    shared: &SharedProgress,
) -> HlsDownloadOutcome {
    let mut temp_output = match NamedTempFile::new_in(&config.output_directory) {
        Ok(file) => file,
        Err(error) => {
            return HlsDownloadOutcome::Failed(HlsDownloadError::Io(format!(
                "could not create output file: {error}"
            )));
        }
    };

    let mut total_bytes: u64 = 0;
    let mut merged_count: usize = 0;

    {
        let mut writer = io::BufWriter::new(temp_output.as_file_mut());
        for seg in &config.plan.segments {
            if shared.cancelled.load(Ordering::Acquire) {
                return HlsDownloadOutcome::Cancelled;
            }
            match write_segment_bytes(seg, config, work_dir, &mut writer) {
                Ok((bytes, final_status)) => {
                    total_bytes = total_bytes.saturating_add(bytes);
                    merged_count += 1;
                    shared.set_segment_status(seg.segment.index, final_status);
                }
                Err(error) => return HlsDownloadOutcome::Failed(error),
            }
        }
        if let Err(error) = writer.flush() {
            return HlsDownloadOutcome::Failed(HlsDownloadError::Io(error.to_string()));
        }
    }

    if merged_count != config.plan.segments.len() {
        return HlsDownloadOutcome::Failed(HlsDownloadError::Io(
            "segment count mismatch during merge".to_owned(),
        ));
    }
    if shared.cancelled.load(Ordering::Acquire) {
        return HlsDownloadOutcome::Cancelled;
    }
    if let Err(error) = temp_output.as_file_mut().sync_all() {
        return HlsDownloadOutcome::Failed(HlsDownloadError::Io(error.to_string()));
    }

    let path = match body_store::persist_unique_file(
        temp_output,
        &config.output_directory,
        &config.plan.suggested_file_name,
    ) {
        Ok(path) => path,
        Err(error) => {
            return HlsDownloadOutcome::Failed(HlsDownloadError::Io(format!(
                "could not publish output file: {error}"
            )));
        }
    };

    HlsDownloadOutcome::Completed(HlsDownloadSummary {
        path,
        total_bytes,
        total_segments: config.plan.segments.len(),
        reused_segments: config.plan.cached_count,
        downloaded_segments: config.plan.missing_count,
    })
}

fn copy_cached_segment(
    cached: &CachedSegment,
    body_store: Option<&BodyStore>,
    output: &mut impl io::Write,
) -> io::Result<u64> {
    if let Outcome::Response(response) = cached.exchange.outcome()
        && let Some(snapshot) = response.body().snapshot()
        && snapshot.is_complete()
    {
        return write_decoded_bytes(snapshot.bytes(), &cached.encodings, output);
    }
    let store = body_store.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "body store unavailable for cached segment",
        )
    })?;
    store.copy_complete_decoded_to(
        cached.record_id,
        BodySide::Response,
        &cached.encodings,
        output,
    )
}

fn write_decoded_bytes(
    bytes: &[u8],
    encodings: &[String],
    output: &mut impl io::Write,
) -> io::Result<u64> {
    use std::io::Read as _;
    let decode = Encodings::parse(encodings.iter().map(String::as_str), EmptyTokens::Skip)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if decode.is_empty() {
        output.write_all(bytes)?;
        return Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
    let probe = MAX_DECODED_CACHED_BYTES.saturating_add(1);
    let mut reader = decode.reader(bytes, probe, BROTLI_BUFFER)?.take(probe);
    let written = io::copy(&mut reader, output)?;
    if written > MAX_DECODED_CACHED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "decoded cached segment exceeded maximum size",
        ));
    }
    Ok(written)
}

fn copy_file_to(path: &Path, output: &mut impl io::Write) -> io::Result<u64> {
    let mut input = io::BufReader::new(File::open(path)?);
    io::copy(&mut input, output)
}

fn segment_temp_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("seg_{index:06}.ts"))
}

fn segment_display_name(seg: &SegmentPlan) -> String {
    seg.segment
        .url
        .last_path_segment()
        .map_or_else(|| seg.segment.raw_uri.clone(), str::to_owned)
}
