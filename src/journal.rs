use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use http::{HeaderMap, Method};
use tempfile::NamedTempFile;

use crate::capture::full_headers;
use crate::{BodyLifecycle, FailureStage, Outcome, RecordId, TunnelSnapshot};

#[derive(Clone, Copy, Debug)]
#[allow(
    clippy::redundant_pub_crate,
    reason = "Body direction is an internal journal API, not a public crate API"
)]
pub(crate) enum BodySide {
    Request,
    Response,
}

impl fmt::Display for BodySide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Request => "Request",
            Self::Response => "Response",
        })
    }
}

const QUEUE_CAPACITY: usize = 1_024;
const EVENT_BYTES: usize = 8 * 1_024;
const BODY_CHUNK_BYTES: usize = 8 * 1_024;

/// Body bytes one app session may write to its journal.
///
/// The journal lives in the temporary directory, which is often a RAM-backed
/// tmpfs, so an unbounded download must not exhaust memory or disk. Headers,
/// results and failures are still written after the limit is reached.
pub const SESSION_BODY_LIMIT_BYTES: u64 = 512 * 1_024 * 1_024;

type ExportResult = io::Result<PathBuf>;
type Failure = Arc<Mutex<Option<String>>>;

#[derive(Debug)]
enum Event {
    Append(Vec<u8>),
    Export(PathBuf, mpsc::Sender<ExportResult>),
}

/// A disk-backed session journal whose size is independent of UI retention.
#[derive(Clone, Debug)]
pub struct FullLog {
    sender: SyncSender<Event>,
    failure: Failure,
    body_budget: Arc<AtomicU64>,
    body_limit: u64,
}

impl FullLog {
    /// Creates a private temporary journal and starts its writer.
    pub fn new() -> io::Result<Self> {
        Self::with_body_limit(SESSION_BODY_LIMIT_BYTES)
    }

    pub(crate) fn with_body_limit(body_limit: u64) -> io::Result<Self> {
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        thread::Builder::new()
            .name("session-journal".to_owned())
            .spawn(move || run(&journal, writer, &receiver, &worker_failure))?;
        Ok(Self {
            sender,
            failure,
            body_budget: Arc::new(AtomicU64::new(body_limit)),
            body_limit,
        })
    }

    /// Returns the configured session body limit in bytes.
    #[must_use]
    pub const fn body_limit(&self) -> u64 {
        self.body_limit
    }

    /// Formats the configured body limit, using bytes below one MiB.
    #[must_use]
    pub fn formatted_body_limit(&self) -> String {
        const MIB: u64 = 1_024 * 1_024;
        let bytes = self.body_limit();
        if bytes < MIB {
            format!("{bytes} bytes")
        } else if bytes.is_multiple_of(MIB) {
            format!("{} MiB", bytes / MIB)
        } else {
            format!("{} MiB + {} bytes", bytes / MIB, bytes % MIB)
        }
    }

    /// Returns whether the session body limit stopped body bytes from being
    /// written to the journal.
    #[must_use]
    pub fn body_limit_reached(&self) -> bool {
        self.body_budget.load(Ordering::Relaxed) == 0
    }

    /// Returns the first error that made this journal incomplete.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.failure.lock().map_or_else(
            |_| Some("The session journal error state is unavailable".to_owned()),
            |failure| failure.clone(),
        )
    }

    fn text(&self, mut text: String) {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let Ok(mut failure) = self.failure.lock() else {
            return;
        };
        for chunk in text.as_bytes().chunks(EVENT_BYTES) {
            if self
                .submit_locked(Event::Append(chunk.to_vec()), &mut failure)
                .is_err()
            {
                break;
            }
        }
    }

    pub(crate) fn request_head(&self, id: u64, method: &Method, target: &str, headers: &HeaderMap) {
        self.text(format!(
            "[{id}] Request {method} {target}\n{}",
            full_headers(headers)
        ));
    }

    pub(crate) fn replay_of(&self, id: u64, original: RecordId) {
        self.text(format!("[{id}] Replay of #{}\n", original.value()));
    }

    pub(crate) fn response_head(&self, id: u64, headers: &HeaderMap) {
        self.text(format!(
            "[{id}] Response headers\n{}",
            full_headers(headers)
        ));
    }

    pub(crate) fn failure(&self, id: u64, stage: FailureStage, message: &str) {
        self.text(format!(
            "[{id}] Failure {stage}: {}\n",
            message.escape_debug()
        ));
    }

    pub(crate) fn settled(&self, id: u64, outcome: &Outcome, elapsed: Duration) {
        let timing_label = if matches!(outcome, Outcome::Response(_)) {
            "Response headers in"
        } else {
            "Settled after"
        };
        self.text(format!(
            "[{id}] {outcome} · {timing_label} {:.1} ms\n",
            elapsed.as_secs_f64() * 1000.0
        ));
    }

    pub(crate) fn tunnel_ended(&self, id: u64, tunnel: &TunnelSnapshot) {
        self.text(format!("[{id}] Tunnel ended {:?} · {} bytes client→upstream · {} bytes upstream→client · {:.1} ms\n", tunnel.state(), tunnel.client_to_upstream(), tunnel.upstream_to_client(), tunnel.duration().as_secs_f64() * 1000.0));
    }

    pub(crate) fn trailers(&self, id: u64, side: BodySide, headers: &HeaderMap) {
        self.text(format!("[{id}] {side} trailers\n{}", full_headers(headers)));
    }

    /// Writes body bytes while the session body budget lasts.
    ///
    /// Returns `false` when some of `bytes` were not written.
    pub(crate) fn body(&self, id: u64, side: BodySide, bytes: &[u8]) -> bool {
        let allowed = self.reserve_body(bytes.len());
        let written = bytes.get(..allowed).unwrap_or_default();
        for chunk in written.chunks(BODY_CHUNK_BYTES) {
            self.text(format!(
                "[{id}] {side} body bytes: {}",
                chunk.escape_ascii()
            ));
        }
        allowed == bytes.len()
    }

    pub(crate) fn body_omitted(&self, id: u64, side: BodySide) {
        self.text(format!(
            "[{id}] {side} body: later bytes omitted because the session body limit of {} was reached",
            self.formatted_body_limit()
        ));
    }

    /// Takes up to `length` bytes from the session budget and returns how
    /// many may be written.
    fn reserve_body(&self, length: usize) -> usize {
        let wanted = u64::try_from(length).unwrap_or(u64::MAX);
        let remaining =
            self.body_budget
                .update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    remaining.saturating_sub(wanted)
                });
        usize::try_from(remaining.min(wanted)).unwrap_or(length)
    }

    pub(crate) fn body_end(&self, id: u64, side: BodySide, state: BodyLifecycle) {
        let state = match state {
            BodyLifecycle::Streaming => "streaming",
            BodyLifecycle::Complete => "complete",
            BodyLifecycle::Interrupted => "interrupted",
            BodyLifecycle::Cancelled => "cancelled",
        };
        self.text(format!("[{id}] {side} body: {state}"));
    }

    /// Saves all journal events accepted before this call to a new file.
    ///
    /// Existing files are never overwritten. The returned receiver reports
    /// completion without blocking the caller on disk I/O.
    pub fn export(&self, path: PathBuf) -> io::Result<Receiver<ExportResult>> {
        let (sender, receiver) = mpsc::channel();
        self.submit(Event::Export(path, sender))?;
        Ok(receiver)
    }

    fn submit(&self, event: Event) -> io::Result<()> {
        let mut failure = self
            .failure
            .lock()
            .map_err(|_| io::Error::other("The session journal error state is unavailable"))?;
        self.submit_locked(event, &mut failure)
    }

    fn submit_locked(&self, event: Event, failure: &mut Option<String>) -> io::Result<()> {
        if let Some(message) = failure.as_ref() {
            return Err(io::Error::other(message.clone()));
        }
        let is_export = matches!(&event, Event::Export(..));
        match self.sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) if is_export => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Session logging is busy; retry saving the log shortly",
            )),
            Err(error) => {
                let message = match error {
                    TrySendError::Full(_) => {
                        "Session logging stopped because its queue filled; the log is incomplete"
                    }
                    TrySendError::Disconnected(_) => {
                        "Session logging stopped because its writer disconnected; the log is incomplete"
                    }
                }
                .to_owned();
                *failure = Some(message.clone());
                Err(io::Error::other(message))
            }
        }
    }
}

fn run(
    journal: &NamedTempFile,
    mut writer: BufWriter<File>,
    receiver: &Receiver<Event>,
    failure: &Failure,
) {
    let mut write_failure = None;
    for event in receiver {
        match event {
            Event::Append(bytes) => {
                if write_failure.is_none()
                    && let Err(error) = writer.write_all(&bytes)
                {
                    record_write_failure(failure, &mut write_failure, &error);
                }
            }
            Event::Export(path, result) => {
                if let Some(message) = write_failure.as_ref() {
                    let _receiver_closed = result.send(Err(io::Error::other(message.clone())));
                    continue;
                }
                if let Err(error) = writer.flush() {
                    record_write_failure(failure, &mut write_failure, &error);
                    let _receiver_closed = result.send(Err(error));
                    continue;
                }
                let snapshot = journal.reopen().and_then(|reader| {
                    let length = reader.metadata()?.len();
                    Ok((reader, length))
                });
                match snapshot {
                    Ok((reader, length)) => {
                        let spawn_failure = result.clone();
                        if let Err(error) = thread::Builder::new()
                            .name("session-export".to_owned())
                            .spawn(move || {
                                let _receiver_closed =
                                    result.send(copy_snapshot(reader, length, &path));
                            })
                        {
                            let _receiver_closed = spawn_failure.send(Err(error));
                        }
                    }
                    Err(error) => {
                        record_write_failure(failure, &mut write_failure, &error);
                        let _receiver_closed = result.send(Err(error));
                    }
                }
            }
        }
    }
}

fn record_write_failure(failure: &Failure, local: &mut Option<String>, error: &io::Error) {
    let message = format!("Session logging failed; the log is incomplete: {error}");
    *local = Some(message.clone());
    if let Ok(mut failure) = failure.lock() {
        failure.get_or_insert(message);
    }
}

fn copy_snapshot(reader: File, length: u64, path: &Path) -> ExportResult {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut destination = NamedTempFile::new_in(parent)?;
    let copied = io::copy(&mut reader.take(length), destination.as_file_mut())?;
    if copied != length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The session journal snapshot ended early",
        ));
    }
    destination.as_file_mut().sync_all()?;
    destination
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn exported(log: &FullLog, path: PathBuf) -> io::Result<String> {
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        std::fs::read_to_string(path)
    }

    fn assert_output(write: impl FnOnce(&FullLog), expected: &str) -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::new()?;
        write(&log);
        assert_eq!(exported(&log, directory.path().join("log.txt"))?, expected);
        Ok(())
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", http::HeaderValue::from_static("secret"));
        headers.insert("x-test", http::HeaderValue::from_static("value"));
        headers
    }

    #[test]
    fn request_head_format() -> io::Result<()> {
        assert_output(
            |log| log.request_head(1, &Method::GET, "http://example.test/", &headers()),
            "[1] Request GET http://example.test/\ncookie: [REDACTED]\nx-test: value\n",
        )
    }

    #[test]
    fn response_head_format() -> io::Result<()> {
        assert_output(
            |log| log.response_head(2, &headers()),
            "[2] Response headers\ncookie: [REDACTED]\nx-test: value\n",
        )
    }

    #[test]
    fn replay_format() -> io::Result<()> {
        assert_output(
            |log| log.replay_of(2, RecordId::new(1)),
            "[2] Replay of #1\n",
        )
    }

    #[test]
    fn failure_format() -> io::Result<()> {
        assert_output(
            |log| log.failure(3, FailureStage::Request, "bad\ninput"),
            "[3] Failure Request: bad\\ninput\n",
        )
    }

    #[test]
    fn settled_format() -> io::Result<()> {
        assert_output(
            |log| log.settled(4, &Outcome::Cancelled, Duration::from_micros(1250)),
            "[4] Cancelled by the client · Settled after 1.2 ms\n",
        )
    }

    #[test]
    fn response_settled_format() -> io::Result<()> {
        let response = crate::CapturedResponse::new(
            http::StatusCode::OK,
            http::Version::HTTP_11,
            "OK".to_owned(),
            crate::capture_headers(&HeaderMap::new()),
            crate::BodyPreview::new(0),
        );
        assert_output(
            |log| log.settled(4, &Outcome::Response(response), Duration::from_millis(2)),
            "[4] Response 200 · Response headers in 2.0 ms\n",
        )
    }

    #[test]
    fn tunnel_ended_format() -> io::Result<()> {
        let tunnel = crate::TunnelPreview::new();
        tunnel.add_client_to_upstream(12);
        tunnel.add_upstream_to_client(34);
        tunnel.close();
        let snapshot = tunnel.snapshot();
        assert_output(
            |log| log.tunnel_ended(5, &snapshot),
            &format!(
                "[5] Tunnel ended Closed · 12 bytes client→upstream · 34 bytes upstream→client · {:.1} ms\n",
                snapshot.duration().as_secs_f64() * 1000.0
            ),
        )
    }

    #[test]
    fn trailers_format() -> io::Result<()> {
        for (side, label) in [
            (BodySide::Request, "Request"),
            (BodySide::Response, "Response"),
        ] {
            assert_output(
                |log| log.trailers(6, side, &headers()),
                &format!("[6] {label} trailers\ncookie: [REDACTED]\nx-test: value\n"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn body_lifecycle_format() -> io::Result<()> {
        for (state, label) in [
            (BodyLifecycle::Streaming, "streaming"),
            (BodyLifecycle::Complete, "complete"),
            (BodyLifecycle::Interrupted, "interrupted"),
            (BodyLifecycle::Cancelled, "cancelled"),
        ] {
            assert_output(
                |log| log.body_end(7, BodySide::Response, state),
                &format!("[7] Response body: {label}\n"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn omission_uses_the_configured_limit() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        for (limit, expected) in [(3, "3 bytes"), (SESSION_BODY_LIMIT_BYTES, "512 MiB")] {
            let log = FullLog::with_body_limit(limit)?;
            assert_eq!(log.body_limit(), limit);
            log.body_omitted(1, BodySide::Request);
            let text = exported(&log, directory.path().join(format!("{limit}.txt")))?;
            assert_eq!(
                text,
                format!(
                    "[1] Request body: later bytes omitted because the session body limit of {expected} was reached\n"
                )
            );
        }
        Ok(())
    }

    #[test]
    fn formats_body_limits_at_the_mib_boundary() -> io::Result<()> {
        for (limit, expected) in [
            (0, "0 bytes"),
            (1_048_575, "1048575 bytes"),
            (1_048_576, "1 MiB"),
            (1_048_577, "1 MiB + 1 bytes"),
        ] {
            assert_eq!(
                FullLog::with_body_limit(limit)?.formatted_body_limit(),
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn retains_complete_large_bodies() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::new()?;
        let body: Vec<u8> = (0..=255).cycle().take(20_000).collect();
        assert!(log.body(7, BodySide::Request, &body));
        log.body_end(7, BodySide::Request, BodyLifecycle::Complete);
        let text = exported(&log, directory.path().join("session.txt"))?;
        let mut expected = String::new();
        for chunk in body.chunks(BODY_CHUNK_BYTES) {
            writeln!(expected, "[7] Request body bytes: {}", chunk.escape_ascii())
                .map_err(io::Error::other)?;
        }
        assert_eq!(text, format!("{expected}[7] Request body: complete\n"));
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn body_limit_writes_a_prefix_and_reports_the_omission() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::with_body_limit(5)?;
        assert!(log.body(1, BodySide::Request, b"abc"));
        assert!(!log.body_limit_reached());
        assert!(!log.body(1, BodySide::Request, b"defg"));
        assert!(log.body_limit_reached());
        assert!(!log.body(2, BodySide::Response, b"h"));
        assert!(log.body(2, BodySide::Response, b""));
        let text = exported(&log, directory.path().join("limited.txt"))?;
        assert_eq!(
            text,
            "[1] Request body bytes: abc\n[1] Request body bytes: de\n"
        );
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn exports_have_independent_boundaries_and_do_not_overwrite() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::new()?;
        log.text("first".to_owned());
        let first_path = directory.path().join("first.txt");
        let first = log.export(first_path.clone())?;
        log.text("second".to_owned());
        let second_path = directory.path().join("second.txt");
        let second = log.export(second_path.clone())?;
        first.recv().map_err(io::Error::other)??;
        second.recv().map_err(io::Error::other)??;
        assert_eq!(std::fs::read_to_string(&first_path)?, "first\n");
        assert_eq!(std::fs::read_to_string(&second_path)?, "first\nsecond\n");
        assert!(
            log.export(first_path.clone())?
                .recv()
                .map_err(io::Error::other)?
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(first_path)?, "first\n");
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn queue_overflow_is_reported_and_blocks_export() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let log = FullLog {
            sender,
            failure: Arc::new(Mutex::new(None)),
            body_budget: Arc::new(AtomicU64::new(SESSION_BODY_LIMIT_BYTES)),
            body_limit: SESSION_BODY_LIMIT_BYTES,
        };
        log.text("first".to_owned());
        log.text("lost".to_owned());
        assert!(
            log.error()
                .is_some_and(|error| error.contains("queue filled"))
        );
        assert!(log.export(PathBuf::from("unused.txt")).is_err());
    }

    #[test]
    fn export_queue_overflow_is_retryable_without_invalidating_log() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = FullLog {
            sender,
            failure: Arc::new(Mutex::new(None)),
            body_budget: Arc::new(AtomicU64::new(SESSION_BODY_LIMIT_BYTES)),
            body_limit: SESSION_BODY_LIMIT_BYTES,
        };
        log.text("first".to_owned());
        let error = log.export(PathBuf::from("unused.txt"));
        assert!(matches!(error, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        assert!(log.error().is_none());
        assert!(receiver.try_recv().is_ok());
        assert!(log.export(PathBuf::from("unused.txt")).is_ok());
    }

    #[test]
    fn later_queue_overflow_preserves_an_accepted_export() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(1);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog {
            sender,
            failure: Arc::clone(&failure),
            body_budget: Arc::new(AtomicU64::new(SESSION_BODY_LIMIT_BYTES)),
            body_limit: SESSION_BODY_LIMIT_BYTES,
        };
        let destination = directory.path().join("snapshot.txt");
        let saved = log.export(destination.clone())?;
        log.text("later event exceeds queue capacity".to_owned());
        assert!(log.error().is_some());
        drop(log);
        run(&journal, writer, &receiver, &failure);
        saved.recv().map_err(io::Error::other)??;
        assert_eq!(std::fs::read(destination)?, b"");
        Ok(())
    }

    #[test]
    fn journal_write_failure_is_reported_to_export() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(File::open(journal.path())?);
        let (sender, receiver) = mpsc::sync_channel(4);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog {
            sender,
            failure: Arc::clone(&failure),
            body_budget: Arc::new(AtomicU64::new(SESSION_BODY_LIMIT_BYTES)),
            body_limit: SESSION_BODY_LIMIT_BYTES,
        };
        log.text("cannot write to read-only journal".to_owned());
        let destination = directory.path().join("snapshot.txt");
        let saved = log.export(destination.clone())?;
        drop(log);
        run(&journal, writer, &receiver, &failure);
        assert!(saved.recv().map_err(io::Error::other)?.is_err());
        assert!(!destination.exists());
        let error = failure
            .lock()
            .map_err(|_| io::Error::other("poisoned error state"))?;
        assert!(
            error
                .as_ref()
                .is_some_and(|message| message.contains("incomplete"))
        );
        drop(error);
        Ok(())
    }
}
