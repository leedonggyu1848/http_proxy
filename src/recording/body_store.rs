//! A disk-backed store of the raw bytes of every captured body.
//!
//! The forwarding path only copies chunks into a bounded queue and never
//! waits for the disk. A writer thread appends them to one append-only data
//! file and records `(record, side) → [(offset, length)]` in an in-memory
//! index, so a single body can later be saved without scanning the session.
//! When the queue is full or a limit is reached, the body is marked
//! incomplete and forwarding continues.

mod bulk;
mod export;
mod file_name;
mod index;
mod location;
mod ranges;
mod writer;

use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub use bulk::{BodyLayout, BulkProgress, BulkSave, BulkSummary};
pub use export::SaveFormat;
pub use index::{StoreGap, StoredBody};
pub use ranges::{ContentRange, RangePlan};

/// Returns the byte range of a captured `206` response, if one is present.
#[must_use]
pub fn response_range(record: &Exchange) -> Option<ContentRange> {
    SaveBody::new(
        record,
        BodySide::Response,
        SaveFormat::AsReceived,
        PathBuf::new(),
    )?
    .content_range()
}

/// Reads complete bodies from the in-memory preview only, for sessions
/// without a body store.
///
/// A body whose preview kept only a prefix is reported as
/// [`ReadBodyOutcome::Incomplete`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewBodies;

impl BodySource for PreviewBodies {
    fn read(&self, record: &Exchange, side: BodySide, max_decoded_bytes: usize) -> ReadBodyOutcome {
        read_captured_body(record, side, None, max_decoded_bytes)
    }
}

/// Reads from the in-memory preview when it kept every byte, and otherwise
/// from the stored file.
impl BodySource for BodyStore {
    fn read(&self, record: &Exchange, side: BodySide, max_decoded_bytes: usize) -> ReadBodyOutcome {
        read_captured_body(record, side, Some(self), max_decoded_bytes)
    }
}

/// Reads one complete body of a finished exchange, decoded and bounded by
/// `max_decoded_bytes`.
///
/// The in-memory preview is used when it kept every byte; otherwise the
/// body is read from `store`. Reading from the store waits for its writer
/// and reads a file, so call this only from a background thread.
fn read_captured_body(
    exchange: &Exchange,
    side: BodySide,
    store: Option<&BodyStore>,
    max_decoded_bytes: usize,
) -> ReadBodyOutcome {
    let (preview, headers) = match (side, exchange.outcome()) {
        (BodySide::Request, _) => (exchange.request().body(), exchange.request().headers()),
        (BodySide::Response, Outcome::Response(response)) => {
            (Some(response.body()), response.headers())
        }
        (BodySide::Response, _) => return ReadBodyOutcome::NotFound,
    };
    let Some(preview) = preview else {
        return ReadBodyOutcome::Complete(Vec::new());
    };
    if preview.lifecycle() != BodyLifecycle::Complete {
        return ReadBodyOutcome::Incomplete;
    }
    if preview.observed_bytes() == 0 {
        return ReadBodyOutcome::Complete(Vec::new());
    }
    let encodings: Vec<String> = headers.get("content-encoding").map(str::to_owned).collect();
    if let Some(snapshot) = preview.snapshot()
        && snapshot.retention() == BodyRetention::Prefix
        && u64::try_from(snapshot.bytes().len()).ok() == Some(snapshot.observed_bytes())
    {
        return decode_complete_bytes(snapshot.bytes(), &encodings, max_decoded_bytes);
    }
    store.map_or(ReadBodyOutcome::Incomplete, |store| {
        store.read_complete_decoded(exchange.id(), side, &encodings, max_decoded_bytes)
    })
}

use self::export::{ContentEncoding, CopyPlan, decode_complete_bytes, read_decoded_segments};
use self::file_name::NameSource;
use self::index::{BodyKey, Index};
use self::location::SessionDirectory;
use self::writer::Command;
use super::lock_recovering;
use crate::exchange::BodySide;
use crate::exchange::{
    BodyLifecycle, BodyRetention, BodySource, Exchange, MediaType, Outcome, ReadBodyOutcome,
    RecordId,
};

/// Default limit on body bytes one session stores.
pub const BODY_STORE_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default free disk space below which no further bytes are stored.
pub const BODY_STORE_MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
/// Queued commands the writer may fall behind by.
const QUEUE_COMMANDS: usize = 4_096;
/// Queued body bytes the writer may fall behind by.
const QUEUE_BYTES: u64 = 64 * 1024 * 1024;
/// Longest wait for the writer before a save reads the index.
const BARRIER_TIMEOUT: Duration = Duration::from_secs(60);

/// State shared by store handles and the writer thread.
#[derive(Debug)]
struct Shared {
    index: Mutex<Index>,
    /// Body bytes accepted into the queue so far in this session.
    used_bytes: AtomicU64,
    limit_bytes: AtomicU64,
    limit_reached: AtomicBool,
    min_free_bytes: u64,
    queued_bytes: AtomicU64,
    disk_low: AtomicBool,
    failed: AtomicBool,
    error: Mutex<Option<String>>,
    closing: AtomicBool,
    data_path: PathBuf,
}

impl Shared {
    fn mark_gap(&self, key: BodyKey, gap: StoreGap) {
        lock_recovering(&self.index).mark_gap(key, gap);
    }

    fn record_write_error(&self, error: &io::Error) {
        self.failed.store(true, Ordering::Release);
        lock_recovering(&self.error)
            .get_or_insert_with(|| format!("Body store writing failed: {error}"));
    }

    /// Reserves `length` bytes of the session limit.
    fn reserve(&self, length: u64) -> bool {
        let limit = self.limit_bytes.load(Ordering::Acquire);
        let reserved = self
            .used_bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(length).filter(|&total| total <= limit)
            })
            .is_ok();
        if !reserved {
            self.limit_reached.store(true, Ordering::Release);
        }
        reserved
    }
}

/// Stops the writer and removes the session directory once the last
/// [`BodyStore`] handle is dropped.
#[derive(Debug)]
struct Owner {
    shared: Arc<Shared>,
    writer: Mutex<Option<JoinHandle<()>>>,
    directory: SessionDirectory,
}

impl Drop for Owner {
    fn drop(&mut self) {
        // Every sender is gone by now, so the writer drains what is queued
        // without writing it and exits.
        self.shared.closing.store(true, Ordering::Release);
        let writer = lock_recovering(&self.writer).take();
        if let Some(writer) = writer
            && writer.join().is_err()
        {
            tracing::error!("body store writer panicked");
        }
    }
}

/// A private, session-scoped store of captured body bytes.
///
/// Files live in a new `0700` directory inside the root passed to
/// [`BodyStore::open`]; the data file is `0600`. The directory is removed
/// when the last handle is dropped, and sessions left by a crashed run are
/// removed the next time a store opens in the same root. Stored bodies stay
/// available for the whole session, even after the UI stops showing their
/// records, because the session limit already bounds the disk use.
#[derive(Clone)]
pub struct BodyStore {
    // Declared before `owner` so the senders are dropped before the owner
    // joins the writer.
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    owner: Arc<Owner>,
}

impl fmt::Debug for BodyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyStore")
            .field("data_path", &self.shared.data_path)
            .finish_non_exhaustive()
    }
}

impl PartialEq for BodyStore {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for BodyStore {}

impl BodyStore {
    /// Opens a new session store in `root` with the default limits.
    pub fn open(root: &Path) -> io::Result<Self> {
        Self::with_limits(root, BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES)
    }

    /// Opens a new session store that keeps at most `limit_bytes` of bodies
    /// and stops storing when free disk space falls below `min_free_bytes`.
    pub fn with_limits(root: &Path, limit_bytes: u64, min_free_bytes: u64) -> io::Result<Self> {
        let directory = SessionDirectory::create(root)?;
        let file = directory.create_data_file()?;
        let shared = Arc::new(Shared {
            index: Mutex::new(Index::default()),
            used_bytes: AtomicU64::new(0),
            limit_bytes: AtomicU64::new(limit_bytes),
            limit_reached: AtomicBool::new(false),
            min_free_bytes,
            queued_bytes: AtomicU64::new(0),
            disk_low: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            error: Mutex::new(None),
            closing: AtomicBool::new(false),
            data_path: directory.data_path(),
        });
        let (sender, receiver) = mpsc::sync_channel(QUEUE_COMMANDS);
        let worker_shared = Arc::clone(&shared);
        let worker_directory = directory.path().to_path_buf();
        let writer = thread::Builder::new()
            .name("body-store".to_owned())
            .spawn(move || writer::run(&receiver, file, &worker_shared, &worker_directory))?;
        let owner = Arc::new(Owner {
            shared: Arc::clone(&shared),
            writer: Mutex::new(Some(writer)),
            directory,
        });
        Ok(Self {
            sender,
            shared,
            owner,
        })
    }

    /// Returns the private directory of this session.
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.owner.directory.path()
    }

    #[must_use]
    pub fn limit_bytes(&self) -> u64 {
        self.shared.limit_bytes.load(Ordering::Acquire)
    }

    /// Changes the session limit. Bodies that start or continue after the
    /// change use the new limit.
    pub fn set_limit_bytes(&self, limit_bytes: u64) {
        self.shared
            .limit_bytes
            .store(limit_bytes, Ordering::Release);
        if self.shared.used_bytes.load(Ordering::Acquire) < limit_bytes {
            self.shared.limit_reached.store(false, Ordering::Release);
        }
    }

    /// Returns the body bytes accepted for storage in this session.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.shared.used_bytes.load(Ordering::Acquire)
    }

    /// Reports whether the session limit stopped some body bytes from being stored.
    #[must_use]
    pub fn limit_reached(&self) -> bool {
        self.shared.limit_reached.load(Ordering::Acquire)
    }

    /// Reports whether low free disk space currently stops storing.
    #[must_use]
    pub fn low_disk_space(&self) -> bool {
        self.shared.disk_low.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn min_free_bytes(&self) -> u64 {
        self.shared.min_free_bytes
    }

    /// Returns the write error that stopped the store, if any.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        lock_recovering(&self.shared.error).clone()
    }

    /// Returns what the store holds for one body, or `None` when it never saw it.
    #[must_use]
    pub fn body(&self, record: RecordId, side: BodySide) -> Option<StoredBody> {
        let key = BodyKey {
            record: record.value(),
            side,
        };
        lock_recovering(&self.shared.index).info(key)
    }

    /// Reads and decompresses a complete stored body into memory, bounded by
    /// `max_decoded_bytes`.
    ///
    /// Intended for background analysis threads (never call on the UI or
    /// forwarding path, as this waits for the store writer barrier and reads
    /// from the session file).
    #[must_use]
    pub fn read_complete_decoded(
        &self,
        record: RecordId,
        side: BodySide,
        encodings: &[String],
        max_decoded_bytes: usize,
    ) -> ReadBodyOutcome {
        if self.wait_for_writer().is_err() {
            return ReadBodyOutcome::Unavailable;
        }
        let key = BodyKey {
            record: record.value(),
            side,
        };
        let Some((segments, body)) = lock_recovering(&self.shared.index).segments(key) else {
            return ReadBodyOutcome::NotFound;
        };
        if !body.is_complete() {
            return ReadBodyOutcome::Incomplete;
        }
        let layers = encoding_layers(encodings);
        let Ok(decode) = ContentEncoding::parse_all(&layers) else {
            return ReadBodyOutcome::DecodeFailed;
        };
        let limit_u64 = u64::try_from(max_decoded_bytes).unwrap_or(u64::MAX);
        if decode.is_empty() && body.stored_bytes() > limit_u64 {
            return ReadBodyOutcome::TooLarge;
        }
        let Ok(file) = File::open(&self.shared.data_path) else {
            return ReadBodyOutcome::Unavailable;
        };
        read_decoded_segments(file, segments, &decode, max_decoded_bytes)
    }

    /// Saves one stored body to a new file in the background.
    ///
    /// The returned receiver reports the saved file once every chunk
    /// accepted before this call is written. Existing files are never
    /// replaced; a numbered name is chosen instead.
    pub fn save(&self, request: SaveBody) -> io::Result<Receiver<io::Result<SavedBody>>> {
        let (sender, receiver) = mpsc::channel();
        let store = self.clone();
        thread::Builder::new()
            .name("body-save".to_owned())
            .spawn(move || {
                let _requester_gone = sender.send(store.save_now(&request));
            })?;
        Ok(receiver)
    }

    fn save_now(&self, request: &SaveBody) -> io::Result<SavedBody> {
        self.wait_for_writer()?;
        self.write_body(request, &request.directory, None)
    }

    /// Writes one stored body into `directory`; the writer must have caught up.
    ///
    /// `numbers` remembers the next free number for each name, so saving
    /// many bodies with one name does not retry every taken number.
    fn write_body(
        &self,
        request: &SaveBody,
        directory: &Path,
        numbers: Option<&mut NameNumbers>,
    ) -> io::Result<SavedBody> {
        let key = BodyKey {
            record: request.record.value(),
            side: request.side,
        };
        let (segments, body) = lock_recovering(&self.shared.index)
            .segments(key)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "This body was not stored; it was captured before the body store opened or was never forwarded",
                )
            })?;
        let layers = encoding_layers(&request.encodings);
        let (decode, kept) = match request.format {
            SaveFormat::Decoded => (ContentEncoding::parse_all(&layers)?, Vec::new()),
            SaveFormat::AsReceived => (Vec::new(), layers),
        };
        // A 206 body shorter or longer than its Content-Range is not the
        // range its name promises.
        let range_mismatch = request
            .content_range
            .filter(|range| body.is_complete() && range.byte_len() != body.stored_bytes())
            .map(|range| {
                format!(
                    "The body has {} bytes but Content-Range promises {}.",
                    body.stored_bytes(),
                    range.byte_len()
                )
            });
        let complete = body.is_complete() && range_mismatch.is_none();
        let name = file_name::file_name(&NameSource {
            record: request.record.value(),
            side: request.side,
            target: &request.target,
            disposition: request.disposition.as_deref(),
            media_type: request.media_type.as_ref(),
            kept_encodings: &kept,
            range: request
                .content_range
                .map(|range| (range.start(), range.end())),
            partial: !complete,
        });
        let plan = CopyPlan {
            data: File::open(&self.shared.data_path)?,
            segments,
            stored_bytes: body.stored_bytes(),
            decode,
            incomplete: !complete,
        };
        let key = directory.join(&name);
        let first = numbers
            .as_ref()
            .and_then(|numbers| numbers.get(&key).copied())
            .unwrap_or(0);
        let (path, bytes, sha256, attempt) = export::save(plan, directory, &name, first)?;
        if let Some(numbers) = numbers {
            numbers.insert(key, attempt.saturating_add(1));
        }
        Ok(SavedBody {
            path,
            bytes,
            sha256,
            complete,
            incomplete_reason: body.incomplete_reason().or(range_mismatch),
        })
    }

    /// Waits until the writer has written and indexed every earlier chunk.
    fn wait_for_writer(&self) -> io::Result<()> {
        let (reply, done) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Barrier(reply))
            .map_err(|_| io::Error::other("The body store writer stopped"))?;
        done.recv_timeout(BARRIER_TIMEOUT).map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "The body store writer did not catch up; try saving again",
            )
        })
    }

    /// Queues a copy of `data` without waiting for the writer.
    fn offer(&self, key: BodyKey, data: &[u8]) -> Result<(), StoreGap> {
        let shared = &self.shared;
        if shared.closing.load(Ordering::Acquire) {
            return Err(StoreGap::StoreClosed);
        }
        if shared.failed.load(Ordering::Acquire) {
            return Err(StoreGap::WriteFailed);
        }
        if shared.disk_low.load(Ordering::Acquire) {
            return Err(StoreGap::LowDiskSpace);
        }
        let length = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if !shared.reserve(length) {
            return Err(StoreGap::SessionLimit);
        }
        let release = |gap| {
            shared.used_bytes.fetch_sub(length, Ordering::AcqRel);
            shared.queued_bytes.fetch_sub(length, Ordering::AcqRel);
            gap
        };
        let queued = shared.queued_bytes.fetch_add(length, Ordering::AcqRel);
        if queued.saturating_add(length) > QUEUE_BYTES {
            return Err(release(StoreGap::QueueFull));
        }
        let command = Command::Chunk {
            key,
            bytes: data.to_vec(),
        };
        match self.sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(release(StoreGap::QueueFull)),
            Err(TrySendError::Disconnected(_)) => Err(release(StoreGap::StoreClosed)),
        }
    }

    fn update_index(&self, update: impl FnOnce(&mut Index)) {
        update(&mut lock_recovering(&self.shared.index));
    }
}

/// The next numbering attempt for each intended file path.
type NameNumbers = std::collections::HashMap<PathBuf, u32>;

/// Splits `Content-Encoding` values into lowercase layers in the order they
/// were applied, without `identity`.
fn encoding_layers(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("identity"))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Copies one forwarded body into the store.
///
/// After the first chunk that cannot be stored, later chunks are skipped so
/// the stored bytes are always an exact prefix of the body.
pub(crate) struct BodyRecorder {
    store: BodyStore,
    key: BodyKey,
    stopped: bool,
    ended: bool,
}

impl BodyRecorder {
    pub(crate) fn new(store: BodyStore, record: u64, side: BodySide) -> Self {
        let key = BodyKey { record, side };
        store.update_index(|index| index.begin(key));
        Self {
            store,
            key,
            stopped: false,
            ended: false,
        }
    }

    pub(crate) fn observe(&mut self, data: &[u8]) {
        if self.stopped || data.is_empty() {
            return;
        }
        if let Err(gap) = self.store.offer(self.key, data) {
            self.stopped = true;
            self.store.shared.mark_gap(self.key, gap);
        }
    }

    /// Records how the body ended. Only the first call has an effect.
    pub(crate) fn end(&mut self, lifecycle: BodyLifecycle) {
        if !self.ended {
            self.ended = true;
            let key = self.key;
            self.store.update_index(|index| index.end(key, lifecycle));
        }
    }
}

/// One body to save, with the captured headers its file name comes from.
#[derive(Clone, Debug)]
pub struct SaveBody {
    record: RecordId,
    side: BodySide,
    format: SaveFormat,
    directory: PathBuf,
    target: String,
    method: String,
    status: Option<u16>,
    disposition: Option<String>,
    media_type: Option<MediaType>,
    encodings: Vec<String>,
    /// The single byte range of a `206 Partial Content` response body.
    content_range: Option<ContentRange>,
    /// `ETag`, or else `Last-Modified`, of the response.
    validator: Option<String>,
}

impl SaveBody {
    /// Returns the byte range of a `206 Partial Content` response body.
    #[must_use]
    pub const fn content_range(&self) -> Option<ContentRange> {
        self.content_range
    }

    /// Describes saving one body of `exchange` into `directory`.
    ///
    /// Returns `None` for a response body when no response was received.
    #[must_use]
    pub fn new(
        exchange: &Exchange,
        side: BodySide,
        format: SaveFormat,
        directory: PathBuf,
    ) -> Option<Self> {
        let request = exchange.request();
        let status = match exchange.outcome() {
            Outcome::Response(response) => Some(response.status().as_u16()),
            _ => None,
        };
        let (headers, media_type) = match (side, exchange.outcome()) {
            (BodySide::Request, _) => (request.headers(), request.media_type()),
            (BodySide::Response, Outcome::Response(response)) => {
                (response.headers(), response.media_type())
            }
            (BodySide::Response, _) => return None,
        };
        Some(Self {
            record: exchange.id(),
            side,
            format,
            directory,
            target: request.target().to_owned(),
            method: request.method().as_str().to_owned(),
            status,
            disposition: headers.get("content-disposition").next().map(str::to_owned),
            media_type: media_type.cloned(),
            encodings: headers.get("content-encoding").map(str::to_owned).collect(),
            content_range: (side == BodySide::Response && status == Some(206))
                .then(|| {
                    headers
                        .get("content-range")
                        .next()
                        .and_then(ContentRange::parse)
                })
                .flatten(),
            validator: headers
                .get("etag")
                .next()
                .or_else(|| headers.get("last-modified").next())
                .map(str::to_owned),
        })
    }
}

/// A body file written by [`BodyStore::save`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBody {
    path: PathBuf,
    bytes: u64,
    sha256: String,
    complete: bool,
    incomplete_reason: Option<String>,
}

impl SavedBody {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Returns the SHA-256 of the written bytes in lowercase hexadecimal.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Explains why the saved body is incomplete, or `None` when it is complete.
    #[must_use]
    pub fn incomplete_reason(&self) -> Option<&str> {
        self.incomplete_reason.as_deref()
    }

    /// Reports whether the whole body was saved; otherwise the name ends in `.partial`.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(store: &BodyStore, record: u64) -> Option<StoredBody> {
        store.wait_for_writer().ok()?;
        store.body(RecordId::new(record), BodySide::Response)
    }

    #[test]
    fn records_chunks_and_the_end_of_a_body() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 1024, 0)?;
        let mut recorder = BodyRecorder::new(store.clone(), 1, BodySide::Response);
        recorder.observe(b"hello ");
        recorder.observe(b"");
        recorder.observe(b"world");
        recorder.end(BodyLifecycle::Complete);
        recorder.end(BodyLifecycle::Cancelled);
        let body = stored(&store, 1);
        assert_eq!(body.map(|body| body.stored_bytes()), Some(11));
        assert!(body.is_some_and(|body| body.is_complete()));
        assert_eq!(store.used_bytes(), 11);
        Ok(())
    }

    #[test]
    fn session_limit_keeps_an_exact_prefix_and_marks_the_gap() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 8, 0)?;
        let mut first = BodyRecorder::new(store.clone(), 1, BodySide::Response);
        first.observe(b"12345");
        first.observe(b"6789");
        first.observe(b"0");
        first.end(BodyLifecycle::Complete);
        let body = stored(&store, 1);
        assert_eq!(body.map(|body| body.stored_bytes()), Some(5));
        assert_eq!(
            body.and_then(|body| body.gap()),
            Some(StoreGap::SessionLimit)
        );
        assert!(store.limit_reached());
        store.set_limit_bytes(100);
        assert!(!store.limit_reached());
        let mut second = BodyRecorder::new(store.clone(), 2, BodySide::Response);
        second.observe(b"more");
        second.end(BodyLifecycle::Complete);
        assert!(stored(&store, 2).is_some_and(|body| body.is_complete()));
        Ok(())
    }

    #[test]
    fn full_queue_marks_the_body_without_waiting() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, 0)?;
        store
            .shared
            .queued_bytes
            .store(QUEUE_BYTES, Ordering::Release);
        let mut recorder = BodyRecorder::new(store.clone(), 3, BodySide::Response);
        recorder.observe(b"dropped");
        store.shared.queued_bytes.store(0, Ordering::Release);
        recorder.observe(b"also skipped");
        recorder.end(BodyLifecycle::Complete);
        let body = stored(&store, 3);
        assert_eq!(body.map(|body| body.stored_bytes()), Some(0));
        assert_eq!(body.and_then(|body| body.gap()), Some(StoreGap::QueueFull));
        assert_eq!(
            store.used_bytes(),
            0,
            "a rejected chunk releases its reservation"
        );
        Ok(())
    }

    #[test]
    fn low_disk_space_stops_storing() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, u64::MAX)?;
        store.wait_for_writer()?;
        assert!(store.low_disk_space());
        let mut recorder = BodyRecorder::new(store.clone(), 4, BodySide::Response);
        recorder.observe(b"bytes");
        assert_eq!(
            stored(&store, 4).and_then(|body| body.gap()),
            Some(StoreGap::LowDiskSpace)
        );
        Ok(())
    }

    #[test]
    fn dropping_the_last_handle_removes_the_session() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::open(root.path())?;
        let copy = store.clone();
        let directory = store.directory().to_path_buf();
        assert!(directory.join("data.bin").exists());
        drop(store);
        assert!(directory.exists(), "a remaining handle keeps the session");
        drop(copy);
        assert!(!directory.exists());
        Ok(())
    }

    #[test]
    fn unknown_bodies_are_not_found() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::open(root.path())?;
        assert_eq!(store.body(RecordId::new(9), BodySide::Request), None);
        assert!(stored(&store, 9).is_none());
        Ok(())
    }

    #[test]
    fn splits_encoding_layers() {
        assert_eq!(
            encoding_layers(&["GZIP, identity".to_owned(), " br ".to_owned()]),
            ["gzip", "br"]
        );
    }

    #[test]
    fn read_complete_decoded_handles_complete_gzip_incomplete_and_oversized() -> io::Result<()> {
        use std::io::Write as _;

        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 64 * 1024, 0)?;

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(br#"{"ok":true,"count":3}"#)?;
        let gz = encoder.finish()?;

        let mut rec1 = BodyRecorder::new(store.clone(), 10, BodySide::Response);
        rec1.observe(&gz);
        rec1.end(BodyLifecycle::Complete);

        assert_eq!(
            store.read_complete_decoded(
                RecordId::new(10),
                BodySide::Response,
                &["gzip".to_owned()],
                1024,
            ),
            ReadBodyOutcome::Complete(br#"{"ok":true,"count":3}"#.to_vec())
        );
        // Oversized decoded cap returns TooLarge.
        assert_eq!(
            store.read_complete_decoded(
                RecordId::new(10),
                BodySide::Response,
                &["gzip".to_owned()],
                8,
            ),
            ReadBodyOutcome::TooLarge
        );
        // Unsupported encoding returns DecodeFailed.
        assert_eq!(
            store.read_complete_decoded(
                RecordId::new(10),
                BodySide::Response,
                &["lz4".to_owned()],
                1024,
            ),
            ReadBodyOutcome::DecodeFailed
        );

        // Interrupted body returns Incomplete.
        let mut rec2 = BodyRecorder::new(store.clone(), 11, BodySide::Response);
        rec2.observe(br#"{"partial":"#);
        rec2.end(BodyLifecycle::Interrupted);
        assert_eq!(
            store.read_complete_decoded(RecordId::new(11), BodySide::Response, &[], 1024),
            ReadBodyOutcome::Incomplete
        );
        assert_eq!(
            store.read_complete_decoded(RecordId::new(99), BodySide::Response, &[], 1024),
            ReadBodyOutcome::NotFound
        );
        Ok(())
    }

    #[test]
    fn preview_bodies_read_only_previews_that_kept_every_byte()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::interception::{BODY_PREVIEW_BYTES, ExchangeBuilder, ResponseFixture};

        let respond = |body: Vec<u8>| {
            ExchangeBuilder::http(http::Method::GET, "http://example.test/")?.respond(
                ResponseFixture::new(http::StatusCode::OK).body(body, BodyLifecycle::Complete),
            )
        };
        let short = respond(b"hello".to_vec())?;
        assert_eq!(
            PreviewBodies.read(&short, BodySide::Response, 1024),
            ReadBodyOutcome::Complete(b"hello".to_vec())
        );
        assert_eq!(
            PreviewBodies.read(&short, BodySide::Response, 2),
            ReadBodyOutcome::TooLarge
        );
        let long = respond(vec![b'a'; BODY_PREVIEW_BYTES + 1])?;
        assert_eq!(
            PreviewBodies.read(&long, BodySide::Response, usize::MAX),
            ReadBodyOutcome::Incomplete
        );
        Ok(())
    }

    #[test]
    fn poisoned_index_recovers_recorded_bodies_and_accepts_new_updates() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 1024, 0)?;

        let mut first = BodyRecorder::new(store.clone(), 1, BodySide::Response);
        first.observe(b"hello");
        first.end(BodyLifecycle::Complete);
        store.wait_for_writer()?;

        crate::recording::poison(&store.shared.index);

        let body = store.body(RecordId::new(1), BodySide::Response);
        assert_eq!(body.map(|body| body.stored_bytes()), Some(5));
        assert!(body.is_some_and(|body| body.is_complete()));
        assert_eq!(
            store.read_complete_decoded(RecordId::new(1), BodySide::Response, &[], 1024),
            ReadBodyOutcome::Complete(b"hello".to_vec())
        );

        let mut second = BodyRecorder::new(store.clone(), 2, BodySide::Response);
        second.observe(b"world");
        second.end(BodyLifecycle::Complete);
        let second_body = stored(&store, 2);
        assert_eq!(second_body.map(|body| body.stored_bytes()), Some(5));
        assert!(second_body.is_some_and(|body| body.is_complete()));

        store.shared.mark_gap(
            BodyKey {
                record: 3,
                side: BodySide::Response,
            },
            StoreGap::WriteFailed,
        );
        assert_eq!(
            store
                .body(RecordId::new(3), BodySide::Response)
                .and_then(|body| body.gap()),
            Some(StoreGap::WriteFailed)
        );
        Ok(())
    }

    #[test]
    fn poisoned_error_slot_still_records_and_reports_write_errors() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 1024, 0)?;
        crate::recording::poison(&store.shared.error);

        assert_eq!(store.error(), None);
        store
            .shared
            .record_write_error(&io::Error::other("disk full"));
        assert!(
            store
                .error()
                .is_some_and(|message| message.contains("disk full"))
        );
        Ok(())
    }
}
