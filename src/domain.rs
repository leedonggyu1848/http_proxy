use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError};
use std::time::Instant;

mod exchange;
mod tunnel;

pub use exchange::{
    CaptureSource, CapturedRequest, CapturedResponse, Exchange, Outcome, SessionId, Timing,
};
pub use tunnel::{TunnelPreview, TunnelSnapshot, TunnelState};

const STREAMING: u8 = 0;
const COMPLETE: u8 = 1;
const INTERRUPTED: u8 = 2;
const CANCELLED: u8 = 3;
const PREFIX: u8 = 0;
const TRUNCATED: u8 = 1;
const OMITTED: u8 = 2;

/// Stable identity of one captured exchange.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecordId(u64);

impl RecordId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn value(self) -> u64 {
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
    pub(crate) const fn new(name: String, value: String) -> Self {
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
    pub(crate) const fn new(entries: Vec<CapturedHeader>, omitted: bool) -> Self {
        Self { entries, omitted }
    }

    /// Returns retained fields in parsed header-map order.
    #[must_use]
    pub fn entries(&self) -> &[CapturedHeader] {
        &self.entries
    }

    /// Reports whether fields were omitted because the limit was reached.
    #[must_use]
    pub const fn omitted(&self) -> bool {
        self.omitted
    }
}

/// Memory shared by every body preview that retains more than the default
/// prefix. Each retained byte is reserved before it is copied and returned
/// when the preview is dropped.
#[derive(Debug)]
#[allow(
    clippy::redundant_pub_crate,
    reason = "The capture module shares this budget; unreachable_pub forbids pub."
)]
pub(crate) struct RetentionBudget {
    available: AtomicUsize,
}

impl RetentionBudget {
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
#[derive(Clone, Debug)]
pub struct BodyPreview {
    inner: Arc<BodyPreviewInner>,
}

impl BodyPreview {
    pub(crate) fn new(limit: usize) -> Self {
        Self::with_budget(limit, None)
    }

    /// Creates a preview that retains up to `limit` bytes, each reserved from
    /// `budget` when one is given. Bytes the budget cannot cover are not
    /// retained and the preview becomes [`BodyRetention::Truncated`].
    pub(crate) fn with_budget(limit: usize, budget: Option<&'static RetentionBudget>) -> Self {
        Self {
            inner: Arc::new(BodyPreviewInner {
                prefix: Mutex::new(Vec::with_capacity(limit.min(INITIAL_PREVIEW_CAPACITY))),
                observed_bytes: AtomicU64::new(0),
                lifecycle: AtomicU8::new(STREAMING),
                ended: OnceLock::new(),
                retention: AtomicU8::new(PREFIX),
                limit,
                budget,
            }),
        }
    }

    pub(crate) fn observe(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let increment = u64::try_from(data.len()).unwrap_or(u64::MAX);
        let _previous =
            self.inner
                .observed_bytes
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    Some(value.saturating_add(increment))
                });
        if self.inner.retention.load(Ordering::Acquire) == OMITTED {
            return;
        }

        match self.inner.prefix.try_lock() {
            Ok(mut prefix) => {
                if self.inner.retention.load(Ordering::Acquire) != PREFIX {
                    return;
                }
                let remaining = self.inner.limit.saturating_sub(prefix.len());
                let mut retained = remaining.min(data.len());
                if let Some(budget) = self.inner.budget {
                    retained = budget.reserve_up_to(retained);
                }
                let (retained_data, _) = data.split_at(retained);
                prefix.extend_from_slice(retained_data);
                if retained < data.len() {
                    self.inner.retention.store(TRUNCATED, Ordering::Release);
                }
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                self.inner.retention.store(OMITTED, Ordering::Release);
            }
        }
    }

    pub(crate) fn complete(&self) {
        self.end(COMPLETE);
    }

    pub(crate) fn interrupt(&self) {
        self.end(INTERRUPTED);
    }

    pub(crate) fn cancel(&self) {
        self.end(CANCELLED);
    }

    /// Records the first end of the stream and when it happened.
    fn end(&self, lifecycle: u8) {
        let ended = self.inner.lifecycle.compare_exchange(
            STREAMING,
            lifecycle,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        if ended.is_ok() {
            let _already_set = self.inner.ended.set(Instant::now()).is_err();
        }
    }

    /// Returns the number of body bytes observed so far without copying the preview.
    #[must_use]
    pub fn observed_bytes(&self) -> u64 {
        self.inner.observed_bytes.load(Ordering::Acquire)
    }

    /// Returns the stream lifecycle without copying the preview.
    #[must_use]
    pub fn lifecycle(&self) -> BodyLifecycle {
        lifecycle_from(self.inner.lifecycle.load(Ordering::Acquire))
    }

    /// Returns when the stream ended, or `None` while it is streaming.
    pub(crate) fn ended_at(&self) -> Option<Instant> {
        self.inner.ended.get().copied()
    }

    /// Returns a coherent snapshot, or `None` when a writer holds the preview.
    #[must_use]
    pub fn snapshot(&self) -> Option<BodySnapshot> {
        let retention = self.inner.retention.load(Ordering::Acquire);
        if retention == OMITTED {
            return Some(self.snapshot_with(Vec::new(), OMITTED));
        }

        match self.inner.prefix.try_lock() {
            Ok(prefix) => {
                let retention = self.inner.retention.load(Ordering::Acquire);
                let bytes = if retention == OMITTED {
                    Vec::new()
                } else {
                    prefix.clone()
                };
                Some(self.snapshot_with(bytes, retention))
            }
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => Some(self.snapshot_with(Vec::new(), OMITTED)),
        }
    }

    fn snapshot_with(&self, bytes: Vec<u8>, retention: u8) -> BodySnapshot {
        BodySnapshot {
            bytes,
            observed_bytes: self.inner.observed_bytes.load(Ordering::Acquire),
            lifecycle: self.lifecycle(),
            retention: match retention {
                TRUNCATED => BodyRetention::Truncated,
                OMITTED => BodyRetention::Omitted,
                _ => BodyRetention::Prefix,
            },
        }
    }
}

const fn lifecycle_from(value: u8) -> BodyLifecycle {
    match value {
        COMPLETE => BodyLifecycle::Complete,
        INTERRUPTED => BodyLifecycle::Interrupted,
        CANCELLED => BodyLifecycle::Cancelled,
        _ => BodyLifecycle::Streaming,
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

    /// Returns the stream lifecycle.
    #[must_use]
    pub const fn lifecycle(&self) -> BodyLifecycle {
        self.lifecycle
    }

    /// Returns how much preview data was retained.
    #[must_use]
    pub const fn retention(&self) -> BodyRetention {
        self.retention
    }
}

/// Lifecycle of the forwarded body stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyLifecycle {
    /// More frames may arrive.
    Streaming,
    /// End of stream was observed.
    Complete,
    /// The sending side failed before the end, for example an upstream
    /// server that closed the connection early.
    Interrupted,
    /// The receiving side stopped reading before the end, for example a
    /// client that disconnected or a proxy that stopped.
    Cancelled,
}

/// Retention result for a bounded body preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyRetention {
    /// All observed bytes fit in the preview.
    Prefix,
    /// More bytes were observed than retained.
    Truncated,
    /// Capture was skipped to avoid blocking forwarding.
    Omitted,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgeted_previews_share_and_return_memory() {
        static BUDGET: RetentionBudget = RetentionBudget::new(10);
        let first = BodyPreview::with_budget(8, Some(&BUDGET));
        first.observe(b"abcdef");
        assert_eq!(BUDGET.available(), 4);

        let second = BodyPreview::with_budget(8, Some(&BUDGET));
        second.observe(b"123456");
        let snapshot = second.snapshot();
        assert_eq!(
            snapshot.as_ref().map(BodySnapshot::bytes),
            Some(&b"1234"[..])
        );
        assert_eq!(
            snapshot.map(|snapshot| snapshot.retention()),
            Some(BodyRetention::Truncated)
        );
        assert_eq!(BUDGET.available(), 0);

        let view = first.clone();
        drop(first);
        assert_eq!(BUDGET.available(), 0, "a clone still holds the bytes");
        drop(view);
        drop(second);
        assert_eq!(BUDGET.available(), 10);
    }
}
