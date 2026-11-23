use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::slice::Chunks;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread;
use std::time::Duration;

use http::{HeaderMap, Method};
use tempfile::NamedTempFile;

use crate::engine::capture::redacted_value;
use crate::exchange::{BodyLifecycle, FailureStage, Outcome, RecordId, TunnelSnapshot};

/// Which message of an exchange a body belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BodySide {
    /// The request body sent by the client.
    Request,
    /// The response body sent by the upstream server.
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

fn full_headers(headers: &HeaderMap) -> String {
    headers
        .iter()
        .fold(String::new(), |mut text, (name, value)| {
            let value = redacted_value(name, value).escape_ascii().to_string();
            text.push_str(name.as_str());
            text.push_str(": ");
            text.push_str(&value);
            text.push('\n');
            text
        })
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

const DISCONNECTED: &str =
    "Session logging stopped because its writer disconnected; the log is incomplete";
const QUEUE_UNAVAILABLE: &str = "The session journal queue is unavailable; the log is incomplete";

#[derive(Debug)]
enum Event {
    Append(Vec<u8>),
    Export(PathBuf, mpsc::Sender<ExportResult>),
}

/// A disk-backed session journal whose size is independent of UI retention.
///
/// The journal never drops an event to keep up with traffic. When its
/// bounded queue is full, recording waits for the writer thread, which
/// slows forwarding during a burst instead of losing log records.
#[derive(Clone, Debug)]
pub struct FullLog {
    /// Serialises submissions so the chunks of one entry stay contiguous and
    /// an export sees every entry accepted before it.
    sender: Arc<Mutex<SyncSender<Event>>>,
    failure: Failure,
    body_budget: Arc<AtomicU64>,
    body_limit: u64,
}

impl FullLog {
    /// Creates a private temporary journal and starts its writer.
    pub fn new() -> io::Result<Self> {
        Self::with_body_limit(SESSION_BODY_LIMIT_BYTES)
    }

    pub(in crate::engine) fn with_body_limit(body_limit: u64) -> io::Result<Self> {
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        thread::Builder::new()
            .name("session-journal".to_owned())
            .spawn(move || run(&journal, writer, &receiver, &worker_failure))?;
        Ok(Self::from_parts(sender, failure, body_limit))
    }

    fn from_parts(sender: SyncSender<Event>, failure: Failure, body_limit: u64) -> Self {
        Self {
            sender: Arc::new(Mutex::new(sender)),
            failure,
            body_budget: Arc::new(AtomicU64::new(body_limit)),
            body_limit,
        }
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
        if self.error().is_some() {
            return;
        }
        let mut chunks = text.as_bytes().chunks(EVENT_BYTES);
        // Fast path: an uncontended lock and free queue space never wait.
        // Every wait, for the lock or for queue space, goes through
        // `while_waiting` so a full queue cannot pin a runtime worker.
        let result = match self.sender.try_lock() {
            Ok(sender) => match send_without_waiting(&sender, &mut chunks) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(event)) => {
                    while_waiting(|| send_waiting(&sender, Some(event), chunks))
                }
                Err(TrySendError::Disconnected(_)) => Err(DISCONNECTED),
            },
            Err(TryLockError::WouldBlock) => while_waiting(|| {
                let sender = self.sender.lock().map_err(|_| QUEUE_UNAVAILABLE)?;
                send_waiting(&sender, None, chunks)
            }),
            Err(TryLockError::Poisoned(_)) => Err(QUEUE_UNAVAILABLE),
        };
        if let Err(message) = result {
            self.record_failure(message);
        }
    }

    fn record_failure(&self, message: &str) {
        if let Ok(mut failure) = self.failure.lock() {
            failure.get_or_insert_with(|| message.to_owned());
        }
    }

    pub(in crate::engine) fn request_head(
        &self,
        id: u64,
        method: &Method,
        target: &str,
        headers: &HeaderMap,
    ) {
        self.text(format!(
            "[{id}] Request {method} {target}\n{}",
            full_headers(headers)
        ));
    }

    pub(in crate::engine) fn replay_of(&self, id: u64, original: RecordId) {
        self.text(format!("[{id}] Replay of #{}\n", original.value()));
    }

    pub(in crate::engine) fn response_head(&self, id: u64, headers: &HeaderMap) {
        self.text(format!(
            "[{id}] Response headers\n{}",
            full_headers(headers)
        ));
    }

    pub(in crate::engine) fn failure(&self, id: u64, stage: FailureStage, message: &str) {
        self.text(format!(
            "[{id}] Failure {stage}: {}\n",
            message.escape_debug()
        ));
    }

    pub(in crate::engine) fn settled(&self, id: u64, outcome: &Outcome, elapsed: Duration) {
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

    pub(in crate::engine) fn tunnel_ended(&self, id: u64, tunnel: &TunnelSnapshot) {
        self.text(format!("[{id}] Tunnel ended {:?} · {} bytes client→upstream · {} bytes upstream→client · {:.1} ms\n", tunnel.state(), tunnel.client_to_upstream(), tunnel.upstream_to_client(), tunnel.duration().as_secs_f64() * 1000.0));
    }

    pub(in crate::engine) fn trailers(&self, id: u64, side: BodySide, headers: &HeaderMap) {
        self.text(format!("[{id}] {side} trailers\n{}", full_headers(headers)));
    }

    /// Writes body bytes while the session body budget lasts.
    ///
    /// Returns `false` when some of `bytes` were not written.
    pub(in crate::engine) fn body(&self, id: u64, side: BodySide, bytes: &[u8]) -> bool {
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

    pub(in crate::engine) fn body_omitted(&self, id: u64, side: BodySide) {
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

    pub(in crate::engine) fn body_end(&self, id: u64, side: BodySide, state: BodyLifecycle) {
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
        let busy = || {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "Session logging is busy; retry saving the log shortly",
            )
        };
        let sender = match self.sender.try_lock() {
            Ok(sender) => sender,
            Err(TryLockError::WouldBlock) => return Err(busy()),
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("The session journal queue is unavailable"));
            }
        };
        if let Some(message) = self.error() {
            return Err(io::Error::other(message));
        }
        match sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(busy()),
            Err(TrySendError::Disconnected(_)) => {
                self.record_failure(DISCONNECTED);
                Err(io::Error::other(DISCONNECTED))
            }
        }
    }
}

/// Sends chunks while the queue has space and returns the first one that
/// did not fit.
fn send_without_waiting(
    sender: &SyncSender<Event>,
    chunks: &mut Chunks<'_, u8>,
) -> Result<(), TrySendError<Event>> {
    chunks.try_for_each(|chunk| sender.try_send(Event::Append(chunk.to_vec())))
}

/// Sends `first` and the remaining chunks, waiting for queue space instead
/// of dropping any of them.
fn send_waiting(
    sender: &SyncSender<Event>,
    first: Option<Event>,
    chunks: Chunks<'_, u8>,
) -> Result<(), &'static str> {
    first
        .into_iter()
        .chain(chunks.map(|chunk| Event::Append(chunk.to_vec())))
        .try_for_each(|event| sender.send(event))
        .map_err(|_| DISCONNECTED)
}

/// Runs a wait for the journal without pinning a Tokio worker.
///
/// On a multi-threaded runtime the wait goes through `block_in_place`, which
/// hands the worker's queued tasks, timers and I/O to another thread, so other
/// connections keep running while this one waits for the disk.
fn while_waiting<R>(wait: impl FnOnce() -> R) -> R {
    use tokio::runtime::{Handle, RuntimeFlavor};

    let on_multi_thread_runtime = Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == RuntimeFlavor::MultiThread);
    if on_multi_thread_runtime {
        tokio::task::block_in_place(wait)
    } else {
        wait()
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

    /// Exports like the UI does: a busy queue is retried, not treated as loss.
    fn exported_when_idle(log: &FullLog, path: &Path) -> io::Result<String> {
        loop {
            match exported(log, path.to_path_buf()) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::yield_now(),
                result => return result,
            }
        }
    }

    #[test]
    fn previews_and_full_logs_share_redaction_without_changing_byte_display()
    -> Result<(), http::header::InvalidHeaderValue> {
        let mut headers = HeaderMap::new();
        for name in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "set-cookie",
        ] {
            headers.insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_static("secret"),
            );
        }
        headers.insert("x-bytes", http::HeaderValue::from_bytes(b"\xff")?);
        let preview = crate::engine::capture::capture_headers(&headers);
        let full = full_headers(&headers);
        for name in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "set-cookie",
        ] {
            assert_eq!(preview.get(name).next(), Some("[REDACTED]"));
            assert!(full.contains(&format!("{name}: [REDACTED]\n")));
        }
        assert!(!full.contains("secret"));
        assert_eq!(preview.get("x-bytes").next(), Some("\u{fffd}"));
        assert!(full.contains("x-bytes: \\xff\n"));
        Ok(())
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
        let response = crate::exchange::CapturedResponse::new(
            http::StatusCode::OK,
            http::Version::HTTP_11,
            "OK".to_owned(),
            crate::engine::capture::capture_headers(&HeaderMap::new()),
            crate::exchange::BodyPreview::new(0),
        );
        assert_output(
            |log| log.settled(4, &Outcome::Response(response), Duration::from_millis(2)),
            "[4] Response 200 · Response headers in 2.0 ms\n",
        )
    }

    #[test]
    fn tunnel_ended_format() -> io::Result<()> {
        let tunnel = crate::exchange::TunnelPreview::new();
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
    fn full_queue_waits_for_the_writer_instead_of_dropping() -> io::Result<()> {
        const EVENTS: usize = 64;
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = FullLog::from_parts(sender, Arc::new(Mutex::new(None)), SESSION_BODY_LIMIT_BYTES);
        let producer = {
            let log = log.clone();
            thread::spawn(move || {
                for index in 0..EVENTS {
                    log.text(format!("event {index}"));
                }
            })
        };
        // The queue holds one event, so the producer can only finish if every
        // send waits for this consumer instead of failing.
        for index in 0..EVENTS {
            let event = receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(io::Error::other)?;
            assert!(
                matches!(event, Event::Append(bytes) if bytes == format!("event {index}\n").into_bytes())
            );
        }
        producer
            .join()
            .map_err(|_| io::Error::other("producer panicked"))?;
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn full_queue_keeps_other_runtime_tasks_running() -> io::Result<()> {
        const WRITERS: usize = 16;
        const TICKS: u32 = 20;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = FullLog::from_parts(sender, Arc::new(Mutex::new(None)), SESSION_BODY_LIMIT_BYTES);
        // With no consumer, every writer task waits either for queue space or
        // for the sender lock held by a waiting writer.
        let (written, writers_done) = mpsc::channel();
        for index in 0..WRITERS {
            let log = log.clone();
            let written = written.clone();
            runtime.spawn(async move {
                log.text(format!("event {index}"));
                let _test_ended = written.send(());
            });
        }
        let (ticked, timer_done) = mpsc::channel();
        runtime.spawn(async move {
            for _ in 0..TICKS {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let _test_ended = ticked.send(());
        });
        let timer = timer_done.recv_timeout(Duration::from_secs(10));
        // The queue holds one event, so at most one writer has finished.
        let finished_early = writers_done.try_iter().count();
        assert!(
            finished_early <= 1,
            "writers must still be waiting while the queue is full"
        );
        assert!(
            timer.is_ok(),
            "a full journal queue stopped timers on other runtime tasks"
        );
        for _ in 0..WRITERS {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(io::Error::other)?;
        }
        for _ in finished_early..WRITERS {
            writers_done
                .recv_timeout(Duration::from_secs(10))
                .map_err(io::Error::other)?;
        }
        assert!(log.error().is_none());
        runtime.shutdown_timeout(Duration::from_secs(1));
        Ok(())
    }

    #[test]
    fn multi_chunk_entries_stay_contiguous_under_contention() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(1);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog::from_parts(sender, Arc::clone(&failure), SESSION_BODY_LIMIT_BYTES);
        let writer_thread = thread::spawn(move || run(&journal, writer, &receiver, &failure));
        let producers: Vec<_> = (b'a'..=b'd')
            .map(|letter| {
                let log = log.clone();
                thread::spawn(move || {
                    let entry = char::from(letter).to_string().repeat(EVENT_BYTES * 3);
                    for _ in 0..4 {
                        log.text(entry.clone());
                    }
                })
            })
            .collect();
        for producer in producers {
            producer
                .join()
                .map_err(|_| io::Error::other("producer panicked"))?;
        }
        let text = exported_when_idle(&log, &directory.path().join("contiguous.txt"))?;
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 16);
        for line in lines {
            assert_eq!(line.len(), EVENT_BYTES * 3);
            let first = line.as_bytes().first().copied();
            assert!(line.bytes().all(|byte| Some(byte) == first));
        }
        drop(log);
        writer_thread
            .join()
            .map_err(|_| io::Error::other("writer panicked"))?;
        Ok(())
    }

    #[test]
    fn export_queue_overflow_is_retryable_without_invalidating_log() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = FullLog::from_parts(sender, Arc::new(Mutex::new(None)), SESSION_BODY_LIMIT_BYTES);
        log.text("first".to_owned());
        let error = log.export(PathBuf::from("unused.txt"));
        assert!(matches!(error, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        assert!(log.error().is_none());
        assert!(receiver.try_recv().is_ok());
        assert!(log.export(PathBuf::from("unused.txt")).is_ok());
    }

    #[test]
    fn disconnected_writer_is_reported_and_blocks_export() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let log = FullLog::from_parts(sender, Arc::new(Mutex::new(None)), SESSION_BODY_LIMIT_BYTES);
        log.text("lost".to_owned());
        assert!(
            log.error()
                .is_some_and(|error| error.contains("writer disconnected"))
        );
        assert!(log.export(PathBuf::from("unused.txt")).is_err());
    }

    #[test]
    fn journal_write_failure_is_reported_to_export() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(File::open(journal.path())?);
        let (sender, receiver) = mpsc::sync_channel(4);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog::from_parts(sender, Arc::clone(&failure), SESSION_BODY_LIMIT_BYTES);
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
