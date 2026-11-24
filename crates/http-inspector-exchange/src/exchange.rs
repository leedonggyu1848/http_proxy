use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::Instant;

mod body_side;
mod body_source;
mod byte_size;
mod image_format;
mod media_type;
#[cfg(feature = "test-support")]
mod memory_body_source;
mod record;
mod tunnel;
mod wakers;

/// Acquires `mutex`, recovering the inner value if a previous holder panicked.
///
/// Use only for locks whose guarded data maintains its invariants across a
/// panic during mutation.
fn lock_recovering<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub use body_side::BodySide;
pub use body_source::{BodySource, ReadBodyOutcome};
pub use byte_size::{ByteSize, ExactByteSize};
pub use image_format::ImageFormat;
pub use media_type::MediaType;
#[cfg(feature = "test-support")]
pub use memory_body_source::MemoryBodySource;
pub use record::{
    CaptureSource, CapturedRequest, CapturedResponse, Exchange, ExchangeWriter, Outcome, SessionId,
    Timing,
};
pub use tunnel::{TunnelPreview, TunnelSnapshot, TunnelState, TunnelWriter};

static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity of one captured exchange.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecordId(u64);

impl RecordId {
    /// Creates a record identity with a specific numeric value for tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Allocates an identity no other exchange in this process has.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the numeric record identity used in logs and replay links.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// One bounded HTTP header field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedHeader {
    name: String,
    value: String,
}

impl CapturedHeader {
    /// Creates a captured header entry.
    #[must_use]
    pub const fn new(name: String, value: String) -> Self {
        Self { name, value }
    }

    /// Returns the normalized header name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the display value or a redaction marker.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Bounded headers captured on one side of an exchange.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeaderPreview {
    entries: Vec<CapturedHeader>,
    omitted: bool,
}

impl HeaderPreview {
    /// Creates a bounded header preview.
    #[must_use]
    pub const fn new(entries: Vec<CapturedHeader>, omitted: bool) -> Self {
        Self { entries, omitted }
    }

    /// Returns retained fields in parsed header-map order.
    #[must_use]
    pub fn entries(&self) -> &[CapturedHeader] {
        &self.entries
    }

    /// Returns all retained values for a header name, ignoring ASCII case.
    pub fn get(&self, name: &str) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(move |header| header.name().eq_ignore_ascii_case(name))
            .map(CapturedHeader::value)
    }

    /// Reports whether fields were omitted because the limit was reached.
    #[must_use]
    pub const fn omitted(&self) -> bool {
        self.omitted
    }
}

/// Recognized HTTP authentication scheme name extracted at capture time
/// without retaining any credential, token, or cookie value.
///
/// Closed enum guarantees by construction that raw secrets can never enter
/// endpoint schema aggregates or UI views.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AuthScheme {
    /// `Authorization: Bearer ...`
    Bearer,
    /// `Authorization: Basic ...`
    Basic,
    /// `Authorization: Digest ...`
    Digest,
    /// `Authorization: OAuth ...`
    OAuth,
    /// `Authorization: AWS4-HMAC-SHA256 ...`
    AwsSigV4,
    /// `Authorization: Token ...` or `X-Auth-Token` / `X-Access-Token` header.
    Token,
    /// `Authorization: ApiKey ...` or `X-API-Key` / `Api-Key` / `ApiKey` header.
    ApiKey,
    /// `Cookie` header present (cookie names and values are never stored).
    Cookie,
    /// `Authorization` header with another or unrecognized scheme format.
    AuthorizationOther,
    /// `Proxy-Authorization` header present.
    ProxyAuthorization,
}

impl AuthScheme {
    /// Human-readable display label shown in the Map detail panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer",
            Self::Basic => "Basic",
            Self::Digest => "Digest",
            Self::OAuth => "OAuth",
            Self::AwsSigV4 => "AWS4-HMAC-SHA256",
            Self::Token => "Token",
            Self::ApiKey => "API Key",
            Self::Cookie => "Cookie",
            Self::AuthorizationOther => "Authorization (other)",
            Self::ProxyAuthorization => "Proxy-Authorization",
        }
    }
}

impl std::fmt::Display for AuthScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Memory shared by every body preview that retains more than the default
/// prefix. Each retained byte is reserved before it is copied and returned
/// when the preview is dropped.
#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide type inside private module; `pub` would trip `unreachable_pub`"
)]
pub(crate) struct RetentionBudget {
    available: AtomicUsize,
}

impl RetentionBudget {
    /// Creates a shared retention budget of `bytes`.
    #[must_use]
    pub(crate) const fn new(bytes: usize) -> Self {
        Self {
            available: AtomicUsize::new(bytes),
        }
    }

    /// Reserves as many of `wanted` bytes as remain and returns that count.
    fn reserve_up_to(&self, wanted: usize) -> usize {
        self.available
            .try_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                Some(available - available.min(wanted))
            })
            .map_or(0, |previous| previous.min(wanted))
    }

    fn release(&self, bytes: usize) {
        let _previous = self.available.fetch_add(bytes, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.available.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct BodyPreviewInner {
    prefix: Mutex<Vec<u8>>,
    observed_bytes: AtomicU64,
    lifecycle: AtomicU8,
    ended: OnceLock<Instant>,
    retention: AtomicU8,
    limit: usize,
    budget: Option<&'static RetentionBudget>,
}

impl Drop for BodyPreviewInner {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            let retained = self
                .prefix
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .len();
            budget.release(retained);
        }
    }
}

/// Initial allocation for a preview; larger budgeted previews grow on demand.
const INITIAL_PREVIEW_CAPACITY: usize = 8 * 1024;

/// Live, bounded observation of a streamed HTTP body.
///
/// This is a read handle: only the code forwarding the body owns the writer
/// that observes bytes and ends the stream.
///
/// ```compile_fail,E0599
/// fn observe(preview: &http_inspector_exchange::BodyPreview) {
///     preview.observe(b"bytes");
/// }
/// ```
#[derive(Clone, Debug)]
pub struct BodyPreview {
    inner: Arc<BodyPreviewInner>,
}

/// Write side of one [`BodyPreview`], owned by the code that forwards the body.
///
/// Readers only ever receive [`BodyPreview`] handles, so observing bytes or
/// ending the stream requires owning this writer.
#[derive(Debug)]
pub struct BodyPreviewWriter {
    preview: BodyPreview,
}

impl BodyPreviewWriter {
    /// Creates a preview that retains up to `limit` bytes without a shared budget.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self::with_budget(limit, None)
    }

    /// Creates a preview that retains up to `limit` bytes, each reserved from
    /// `budget` when one is given. Bytes the budget cannot cover are not
    /// retained and the preview becomes [`BodyRetention::Truncated`].
    #[must_use]
    pub(crate) fn with_budget(limit: usize, budget: Option<&'static RetentionBudget>) -> Self {
        Self {
            preview: BodyPreview {
                inner: Arc::new(BodyPreviewInner {
                    prefix: Mutex::new(Vec::with_capacity(limit.min(INITIAL_PREVIEW_CAPACITY))),
                    observed_bytes: AtomicU64::new(0),
                    lifecycle: AtomicU8::new(BodyLifecycle::Streaming.to_u8()),
                    ended: OnceLock::new(),
                    retention: AtomicU8::new(BodyRetention::Prefix.to_u8()),
                    limit,
                    budget,
                }),
            },
        }
    }

    /// Returns a read handle that follows this writer.
    #[must_use]
    pub fn preview(&self) -> BodyPreview {
        self.preview.clone()
    }

    /// Records observed body bytes into the bounded preview.
    pub fn observe(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let inner = &self.preview.inner;
        let increment = u64::try_from(data.len()).unwrap_or(u64::MAX);
        let _previous =
            inner
                .observed_bytes
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    Some(value.saturating_add(increment))
                });
        if BodyRetention::from_u8(inner.retention.load(Ordering::Acquire)) == BodyRetention::Omitted
        {
            return;
        }

        match inner.prefix.try_lock() {
            Ok(mut prefix) => {
                if BodyRetention::from_u8(inner.retention.load(Ordering::Acquire))
                    != BodyRetention::Prefix
                {
                    return;
                }
                let remaining = inner.limit.saturating_sub(prefix.len());
                let mut retained = remaining.min(data.len());
                if let Some(budget) = inner.budget {
                    retained = budget.reserve_up_to(retained);
                }
                let (retained_data, _) = data.split_at(retained);
                prefix.extend_from_slice(retained_data);
                if retained < data.len() {
                    inner
                        .retention
                        .store(BodyRetention::Truncated.to_u8(), Ordering::Release);
                }
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                inner
                    .retention
                    .store(BodyRetention::Omitted.to_u8(), Ordering::Release);
            }
        }
    }

    /// Marks the body stream cleanly complete.
    pub fn complete(&self) {
        self.end(BodyLifecycle::Complete);
    }

    /// Marks the body stream interrupted by a sender error.
    pub fn interrupt(&self) {
        self.end(BodyLifecycle::Interrupted);
    }

    /// Marks the body stream cancelled before completion.
    pub fn cancel(&self) {
        self.end(BodyLifecycle::Cancelled);
    }

    /// Records the first end of the stream and when it happened.
    fn end(&self, lifecycle: BodyLifecycle) {
        let inner = &self.preview.inner;
        let ended = inner.lifecycle.compare_exchange(
            BodyLifecycle::Streaming.to_u8(),
            lifecycle.to_u8(),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        if ended.is_ok() {
            let _already_set = inner.ended.set(Instant::now()).is_err();
        }
    }
}

impl BodyPreview {
    /// Returns the number of body bytes observed so far without copying the preview.
    #[must_use]
    pub fn observed_bytes(&self) -> u64 {
        self.inner.observed_bytes.load(Ordering::Acquire)
    }

    /// Returns the stream lifecycle without copying the preview.
    #[must_use]
    pub fn lifecycle(&self) -> BodyLifecycle {
        BodyLifecycle::from_u8(self.inner.lifecycle.load(Ordering::Acquire))
    }

    /// Returns when the stream ended, or `None` while it is streaming.
    pub(in crate::exchange) fn ended_at(&self) -> Option<Instant> {
        self.inner.ended.get().copied()
    }

    /// Returns a coherent snapshot, or `None` when a writer holds the preview.
    #[must_use]
    pub fn snapshot(&self) -> Option<BodySnapshot> {
        let retention = BodyRetention::from_u8(self.inner.retention.load(Ordering::Acquire));
        if retention == BodyRetention::Omitted {
            return Some(self.snapshot_with(Vec::new(), BodyRetention::Omitted));
        }

        match self.inner.prefix.try_lock() {
            Ok(prefix) => {
                let retention =
                    BodyRetention::from_u8(self.inner.retention.load(Ordering::Acquire));
                let bytes = if retention == BodyRetention::Omitted {
                    Vec::new()
                } else {
                    prefix.clone()
                };
                Some(self.snapshot_with(bytes, retention))
            }
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                Some(self.snapshot_with(Vec::new(), BodyRetention::Omitted))
            }
        }
    }

    fn snapshot_with(&self, bytes: Vec<u8>, retention: BodyRetention) -> BodySnapshot {
        BodySnapshot {
            bytes,
            observed_bytes: self.inner.observed_bytes.load(Ordering::Acquire),
            lifecycle: self.lifecycle(),
            retention,
        }
    }
}

/// Immutable view of a body preview at one instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodySnapshot {
    bytes: Vec<u8>,
    observed_bytes: u64,
    lifecycle: BodyLifecycle,
    retention: BodyRetention,
}

impl BodySnapshot {
    /// Returns retained body bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the number of body bytes observed so far.
    #[must_use]
    pub const fn observed_bytes(&self) -> u64 {
        self.observed_bytes
    }

    #[must_use]
    pub const fn lifecycle(&self) -> BodyLifecycle {
        self.lifecycle
    }

    #[must_use]
    pub const fn retention(&self) -> BodyRetention {
        self.retention
    }
}

/// Lifecycle of the forwarded body stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BodyLifecycle {
    /// More frames may arrive.
    Streaming = 0,
    /// End of stream was observed.
    Complete = 1,
    /// The sending side failed before the end, for example an upstream
    /// server that closed the connection early.
    Interrupted = 2,
    /// The receiving side stopped reading before the end, for example a
    /// client that disconnected or a proxy that stopped.
    Cancelled = 3,
}

impl BodyLifecycle {
    const fn to_u8(self) -> u8 {
        self as u8
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Complete,
            2 => Self::Interrupted,
            3 => Self::Cancelled,
            _ => Self::Streaming,
        }
    }
}

/// Retention result for a bounded body preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BodyRetention {
    /// All observed bytes fit in the preview.
    Prefix = 0,
    /// More bytes were observed than retained.
    Truncated = 1,
    /// Capture was skipped to avoid blocking forwarding.
    Omitted = 2,
}

impl BodyRetention {
    const fn to_u8(self) -> u8 {
        self as u8
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Truncated,
            2 => Self::Omitted,
            _ => Self::Prefix,
        }
    }
}

/// Stage at which a proxied request failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    /// The request target could not be parsed.
    Request,
    /// The upstream TCP connection failed.
    Connect,
    /// Resolving or connecting to the upstream exceeded the connect timeout.
    ConnectTimeout,
    /// The upstream HTTP exchange failed.
    Response,
    /// Response headers did not arrive within the response timeout.
    ResponseTimeout,
    /// The CONNECT stream could not be upgraded.
    Upgrade,
    /// The client-side TLS handshake failed.
    ClientTls,
    /// The upstream TLS handshake or verification failed.
    UpstreamTls,
    /// Relaying bytes through an established tunnel failed.
    Tunnel,
}

impl std::fmt::Display for FailureStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Request => "Request",
            Self::Connect => "Connection",
            Self::ConnectTimeout => "Connection timeout",
            Self::Response => "Response",
            Self::ResponseTimeout => "Response timeout",
            Self::Upgrade => "CONNECT upgrade",
            Self::ClientTls => "Client TLS handshake",
            Self::UpstreamTls => "Upstream TLS handshake",
            Self::Tunnel => "Tunnel relay",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_headers_preserve_duplicates_and_ignore_name_case() {
        let headers = HeaderPreview::new(
            vec![
                CapturedHeader::new("content-encoding".into(), "gzip".into()),
                CapturedHeader::new("content-encoding".into(), "br".into()),
            ],
            true,
        );
        assert_eq!(
            headers.get("Content-Encoding").collect::<Vec<_>>(),
            ["gzip", "br"]
        );
        assert_eq!(headers.get("missing").next(), None);
        assert_eq!(HeaderPreview::default().get("content-type").next(), None);
    }

    #[test]
    fn body_lifecycle_round_trips_through_u8() {
        for (encoded, state) in [
            (0, BodyLifecycle::Streaming),
            (1, BodyLifecycle::Complete),
            (2, BodyLifecycle::Interrupted),
            (3, BodyLifecycle::Cancelled),
        ] {
            assert_eq!(state.to_u8(), encoded);
            assert_eq!(BodyLifecycle::from_u8(state.to_u8()), state);
        }
    }

    #[test]
    fn body_retention_round_trips_through_u8() {
        for (encoded, state) in [
            (0, BodyRetention::Prefix),
            (1, BodyRetention::Truncated),
            (2, BodyRetention::Omitted),
        ] {
            assert_eq!(state.to_u8(), encoded);
            assert_eq!(BodyRetention::from_u8(state.to_u8()), state);
        }
    }

    #[test]
    fn budgeted_previews_share_and_return_memory() {
        static BUDGET: RetentionBudget = RetentionBudget::new(10);
        let first = BodyPreviewWriter::with_budget(8, Some(&BUDGET));
        first.observe(b"abcdef");
        assert_eq!(BUDGET.available(), 4);

        let second = BodyPreviewWriter::with_budget(8, Some(&BUDGET));
        second.observe(b"123456");
        let snapshot = second.preview().snapshot();
        assert_eq!(
            snapshot.as_ref().map(BodySnapshot::bytes),
            Some(&b"1234"[..])
        );
        assert_eq!(
            snapshot.map(|snapshot| snapshot.retention()),
            Some(BodyRetention::Truncated)
        );
        assert_eq!(BUDGET.available(), 0);

        let view = first.preview();
        drop(first);
        assert_eq!(BUDGET.available(), 0, "a clone still holds the bytes");
        drop(view);
        drop(second);
        assert_eq!(BUDGET.available(), 10);
    }
}
