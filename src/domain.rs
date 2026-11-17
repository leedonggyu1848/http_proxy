use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use http::Method;

const STREAMING: u8 = 0;
const COMPLETE: u8 = 1;
const INTERRUPTED: u8 = 2;
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

#[derive(Debug)]
struct BodyPreviewInner {
    prefix: Mutex<Vec<u8>>,
    observed_bytes: AtomicU64,
    lifecycle: AtomicU8,
    retention: AtomicU8,
    limit: usize,
}

/// Live, bounded observation of a streamed HTTP body.
#[derive(Clone, Debug)]
pub struct BodyPreview {
    inner: Arc<BodyPreviewInner>,
}

impl BodyPreview {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(BodyPreviewInner {
                prefix: Mutex::new(Vec::with_capacity(limit)),
                observed_bytes: AtomicU64::new(0),
                lifecycle: AtomicU8::new(STREAMING),
                retention: AtomicU8::new(PREFIX),
                limit,
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
                let retained = remaining.min(data.len());
                prefix.extend_from_slice(&data[..retained]);
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
        let _unchanged = self.inner.lifecycle.compare_exchange(
            STREAMING,
            COMPLETE,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn interrupt(&self) {
        let _unchanged = self.inner.lifecycle.compare_exchange(
            STREAMING,
            INTERRUPTED,
            Ordering::Release,
            Ordering::Relaxed,
        );
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
            lifecycle: match self.inner.lifecycle.load(Ordering::Acquire) {
                COMPLETE => BodyLifecycle::Complete,
                INTERRUPTED => BodyLifecycle::Interrupted,
                _ => BodyLifecycle::Streaming,
            },
            retention: match retention {
                TRUNCATED => BodyRetention::Truncated,
                OMITTED => BodyRetention::Omitted,
                _ => BodyRetention::Prefix,
            },
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
    /// The body errored or was dropped before completion.
    Interrupted,
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

/// One HTTP exchange observed by the proxy.
#[derive(Clone, Debug)]
pub struct CaptureRecord {
    id: RecordId,
    method: Method,
    target: String,
    request_headers: HeaderPreview,
    request_body: Option<BodyPreview>,
    outcome: CaptureOutcome,
    elapsed: Duration,
}

impl CaptureRecord {
    pub(crate) const fn new(
        id: RecordId,
        method: Method,
        target: String,
        request_headers: HeaderPreview,
        request_body: Option<BodyPreview>,
        outcome: CaptureOutcome,
        elapsed: Duration,
    ) -> Self {
        Self {
            id,
            method,
            target,
            request_headers,
            request_body,
            outcome,
            elapsed,
        }
    }

    /// Returns the stable exchange identity.
    #[must_use]
    pub const fn id(&self) -> RecordId {
        self.id
    }

    /// Returns the HTTP method.
    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.method
    }

    /// Returns the absolute request target or CONNECT authority.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns the original parsed request headers.
    #[must_use]
    pub const fn request_headers(&self) -> &HeaderPreview {
        &self.request_headers
    }

    /// Returns a live request-body preview when forwarding began.
    #[must_use]
    pub const fn request_body(&self) -> Option<&BodyPreview> {
        self.request_body.as_ref()
    }

    /// Returns the observed result.
    #[must_use]
    pub const fn outcome(&self) -> &CaptureOutcome {
        &self.outcome
    }

    /// Returns the time until response headers or tunnel connection arrived.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

/// Result observed for a captured request.
#[derive(Clone, Debug)]
pub enum CaptureOutcome {
    /// An HTTP response arrived.
    Response {
        status: u16,
        headers: HeaderPreview,
        body: BodyPreview,
    },
    /// The proxy established a TCP tunnel.
    TunnelEstablished,
    /// The request failed before a response arrived.
    Failed {
        stage: FailureStage,
        message: String,
    },
}

/// Stage at which a proxied request failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    /// The request target could not be parsed.
    Request,
    /// The upstream TCP connection failed.
    Connect,
    /// The upstream HTTP exchange failed.
    Response,
}
