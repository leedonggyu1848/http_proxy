//! Per-node aggregate traffic metrics for the URL map.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use http::{Method, StatusCode};

use crate::Outcome;
use crate::analysis::url::IdKind;

/// Maximum distinct Content-Type essences retained per node to bound memory.
const MAX_CONTENT_TYPES_PER_NODE: usize = 16;
/// Maximum distinct query parameter names retained per node to bound memory.
const MAX_QUERY_PARAMS_PER_NODE: usize = 256;
/// Maximum custom (non-standard) HTTP method names retained per node.
const MAX_CUSTOM_METHODS_PER_NODE: usize = 8;

/// Coarse status bucket used for status bars, counts, and filtering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StatusBucket {
    /// Request has not settled yet.
    Pending,
    /// `1xx` or `2xx` response.
    S2xx,
    /// `3xx` redirect response.
    S3xx,
    /// `4xx` client error response.
    S4xx,
    /// `5xx` server error response.
    S5xx,
    /// Network/TLS failure, client cancellation, or incomplete shutdown.
    Failed,
    /// Opaque `CONNECT` tunnel or HTTPS inspection setup.
    Tunnel,
}

impl StatusBucket {
    /// Classifies an HTTP status code into its status bucket.
    #[must_use]
    pub const fn from_status(status: StatusCode) -> Self {
        match status.as_u16() {
            100..=299 => Self::S2xx,
            300..=399 => Self::S3xx,
            400..=499 => Self::S4xx,
            _ => Self::S5xx,
        }
    }

    /// Classifies an exchange [`Outcome`] into its status bucket.
    #[must_use]
    pub fn from_outcome(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::InFlight => Self::Pending,
            Outcome::Response(response) => Self::from_status(response.status()),
            Outcome::Tunnel(_) if outcome.is_failure() => Self::Failed,
            Outcome::Tunnel(_) | Outcome::InspectionEstablished => Self::Tunnel,
            Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => Self::Failed,
        }
    }
}

/// Counts of requests by status distribution bucket.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatusCounts {
    /// Unsettled requests still in flight.
    pub pending: u64,
    /// `1xx` and `2xx` responses.
    pub s2xx: u64,
    /// `3xx` responses.
    pub s3xx: u64,
    /// `4xx` responses.
    pub s4xx: u64,
    /// `5xx` responses.
    pub s5xx: u64,
    /// Failed, cancelled, or incomplete exchanges.
    pub failed: u64,
    /// Opaque `CONNECT` tunnels and HTTPS inspection sessions.
    pub tunnel: u64,
}

impl StatusCounts {
    /// Increments the counter for `bucket`.
    pub const fn increment(&mut self, bucket: StatusBucket) {
        let slot = self.slot_mut(bucket);
        *slot = slot.saturating_add(1);
    }

    /// Decrements the counter for `bucket`, saturating at zero.
    pub const fn decrement(&mut self, bucket: StatusBucket) {
        let slot = self.slot_mut(bucket);
        *slot = slot.saturating_sub(1);
    }

    /// Returns the total error count (`4xx + 5xx + failed`).
    #[must_use]
    pub const fn errors(self) -> u64 {
        self.s4xx
            .saturating_add(self.s5xx)
            .saturating_add(self.failed)
    }

    /// Returns the sum of all settled and unsettled status buckets.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.pending
            .saturating_add(self.s2xx)
            .saturating_add(self.s3xx)
            .saturating_add(self.s4xx)
            .saturating_add(self.s5xx)
            .saturating_add(self.failed)
            .saturating_add(self.tunnel)
    }

    /// Merges counts from `other` into `self`.
    pub const fn merge(&mut self, other: Self) {
        self.pending = self.pending.saturating_add(other.pending);
        self.s2xx = self.s2xx.saturating_add(other.s2xx);
        self.s3xx = self.s3xx.saturating_add(other.s3xx);
        self.s4xx = self.s4xx.saturating_add(other.s4xx);
        self.s5xx = self.s5xx.saturating_add(other.s5xx);
        self.failed = self.failed.saturating_add(other.failed);
        self.tunnel = self.tunnel.saturating_add(other.tunnel);
    }

    const fn slot_mut(&mut self, bucket: StatusBucket) -> &mut u64 {
        match bucket {
            StatusBucket::Pending => &mut self.pending,
            StatusBucket::S2xx => &mut self.s2xx,
            StatusBucket::S3xx => &mut self.s3xx,
            StatusBucket::S4xx => &mut self.s4xx,
            StatusBucket::S5xx => &mut self.s5xx,
            StatusBucket::Failed => &mut self.failed,
            StatusBucket::Tunnel => &mut self.tunnel,
        }
    }
}

/// Compact set of observed HTTP methods.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MethodSet {
    bits: u8,
    custom: BTreeSet<String>,
}

const BIT_GET: u8 = 1 << 0;
const BIT_POST: u8 = 1 << 1;
const BIT_PUT: u8 = 1 << 2;
const BIT_PATCH: u8 = 1 << 3;
const BIT_DELETE: u8 = 1 << 4;
const BIT_HEAD: u8 = 1 << 5;
const BIT_OPTIONS: u8 = 1 << 6;
const BIT_CONNECT: u8 = 1 << 7;

const STANDARD_METHODS: [(u8, &str); 8] = [
    (BIT_GET, "GET"),
    (BIT_POST, "POST"),
    (BIT_PUT, "PUT"),
    (BIT_PATCH, "PATCH"),
    (BIT_DELETE, "DELETE"),
    (BIT_HEAD, "HEAD"),
    (BIT_OPTIONS, "OPTIONS"),
    (BIT_CONNECT, "CONNECT"),
];

impl MethodSet {
    /// Adds `method` to the set.
    pub fn insert(&mut self, method: &Method) {
        self.insert_str(method.as_str());
    }

    /// Adds a method name to the set.
    pub fn insert_str(&mut self, method: &str) {
        match method {
            "GET" => self.bits |= BIT_GET,
            "POST" => self.bits |= BIT_POST,
            "PUT" => self.bits |= BIT_PUT,
            "PATCH" => self.bits |= BIT_PATCH,
            "DELETE" => self.bits |= BIT_DELETE,
            "HEAD" => self.bits |= BIT_HEAD,
            "OPTIONS" => self.bits |= BIT_OPTIONS,
            "CONNECT" => self.bits |= BIT_CONNECT,
            other => {
                if self.custom.len() < MAX_CUSTOM_METHODS_PER_NODE {
                    let _ = self.custom.insert(other.to_owned());
                }
            }
        }
    }

    /// Merges another method set into `self`.
    pub fn merge(&mut self, other: &Self) {
        self.bits |= other.bits;
        for name in &other.custom {
            if self.custom.len() >= MAX_CUSTOM_METHODS_PER_NODE {
                break;
            }
            let _ = self.custom.insert(name.clone());
        }
    }

    /// Returns observed method names in canonical order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for (bit, name) in STANDARD_METHODS {
            if self.bits & bit != 0 {
                out.push(name);
            }
        }
        for name in &self.custom {
            out.push(name.as_str());
        }
        out
    }

    /// Reports whether no methods have been observed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits == 0 && self.custom.is_empty()
    }
}

/// Cumulative traffic statistics for a URL map node or folded subtree.
///
/// Retains only structural metadata (counts, status distribution, method names,
/// Content-Type essences, query parameter *names*, timings, and byte totals).
/// Query values and body contents are never stored here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeStats {
    /// Total requests observed at this node (or across its subtree).
    pub request_count: u64,
    /// Distinct HTTP methods observed.
    pub methods: MethodSet,
    /// Status distribution counts.
    pub status: StatusCounts,
    /// Observed response Content-Type essences and their counts.
    pub content_types: BTreeMap<String, u64>,
    /// Total response/tunnel bytes observed.
    pub total_bytes: u64,
    /// Sum of completed response durations in microseconds.
    duration_sum_micros: u128,
    /// Number of completed duration samples.
    duration_samples: u64,
    /// Maximum observed duration in microseconds.
    max_duration_micros: u64,
    /// Earliest request wall-clock time observed.
    pub first_seen: Option<SystemTime>,
    /// Latest request wall-clock time observed.
    pub last_seen: Option<SystemTime>,
    /// Requests marked third-party via `Referer` or `Origin` eTLD+1 mismatch.
    pub third_party_hits: u64,
    /// Query parameter names (never values) and how many times each appeared.
    pub query_params: BTreeMap<String, u64>,
    /// Whether an opaque `CONNECT` tunnel was observed here.
    pub has_tunnel: bool,
    /// Requests folded into this ancestor because the global node cap was reached.
    pub overflow_merged: u64,
    /// Observed dynamic ID value formats when folded into `{id}`.
    pub observed_id_kinds: BTreeSet<IdKind>,
}

impl NodeStats {
    /// Records the arrival of a request on this node.
    pub fn record_request(
        &mut self,
        method: &Method,
        initial_bucket: StatusBucket,
        started_at: SystemTime,
        third_party: bool,
        is_tunnel: bool,
    ) {
        self.request_count = self.request_count.saturating_add(1);
        self.methods.insert(method);
        self.status.increment(initial_bucket);
        if third_party {
            self.third_party_hits = self.third_party_hits.saturating_add(1);
        }
        if is_tunnel {
            self.has_tunnel = true;
        }
        self.observe_time(started_at);
    }

    /// Records query parameter names (never values).
    pub fn record_query_params(&mut self, names: &[String]) {
        for name in names {
            if let Some(count) = self.query_params.get_mut(name) {
                *count = count.saturating_add(1);
            } else if self.query_params.len() < MAX_QUERY_PARAMS_PER_NODE {
                let _ = self.query_params.insert(name.clone(), 1);
            }
        }
    }

    /// Transitions one request's status bucket from `previous` to `next`.
    pub const fn transition_status(&mut self, previous: StatusBucket, next: StatusBucket) {
        if previous as u8 == next as u8 {
            return;
        }
        self.status.decrement(previous);
        self.status.increment(next);
    }

    /// Records a response `Content-Type` essence (e.g. `application/json`).
    pub fn record_content_type(&mut self, essence: &str) {
        if essence.is_empty() {
            return;
        }
        if let Some(count) = self.content_types.get_mut(essence) {
            *count = count.saturating_add(1);
        } else if self.content_types.len() < MAX_CONTENT_TYPES_PER_NODE {
            let _ = self.content_types.insert(essence.to_owned(), 1);
        }
    }

    /// Adds newly observed transferred bytes.
    pub const fn add_bytes(&mut self, delta: u64) {
        self.total_bytes = self.total_bytes.saturating_add(delta);
    }

    /// Records one response duration sample.
    pub fn record_duration(&mut self, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        self.duration_sum_micros = self.duration_sum_micros.saturating_add(u128::from(micros));
        self.duration_samples = self.duration_samples.saturating_add(1);
        if micros > self.max_duration_micros {
            self.max_duration_micros = micros;
        }
    }

    /// Returns the most frequently observed Content-Type essence, if any.
    #[must_use]
    pub fn primary_content_type(&self) -> Option<&str> {
        self.content_types
            .iter()
            .max_by(|(left_name, left_count), (right_name, right_count)| {
                left_count
                    .cmp(right_count)
                    .then_with(|| right_name.cmp(left_name))
            })
            .map(|(name, _)| name.as_str())
    }

    /// Returns the average observed duration in milliseconds, if any sample exists.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "millisecond display precision")]
    pub fn avg_duration_ms(&self) -> Option<f64> {
        if self.duration_samples == 0 {
            return None;
        }
        let avg_micros = (self.duration_sum_micros as f64) / (self.duration_samples as f64);
        Some(avg_micros / 1_000.0)
    }

    /// Returns the maximum observed duration in milliseconds, if any sample exists.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "millisecond display precision")]
    pub fn max_duration_ms(&self) -> Option<f64> {
        if self.duration_samples == 0 {
            return None;
        }
        Some((self.max_duration_micros as f64) / 1_000.0)
    }

    /// Reports whether any third-party request hit this node.
    #[must_use]
    pub const fn is_third_party(&self) -> bool {
        self.third_party_hits > 0
    }

    /// Reports whether any first-party (or top-level) request hit this node.
    #[must_use]
    pub const fn has_first_party(&self) -> bool {
        self.request_count > self.third_party_hits
    }

    /// Reports whether this node represents only non-failing, settled opaque
    /// `CONNECT` tunnels.
    #[must_use]
    pub const fn is_tunnel_only(&self) -> bool {
        self.has_tunnel
            && self.status.s2xx == 0
            && self.status.s3xx == 0
            && self.status.s4xx == 0
            && self.status.s5xx == 0
            && self.status.failed == 0
            && self.status.pending == 0
    }

    /// Merges another `NodeStats` into `self` (used when folding ID nodes).
    pub fn merge(&mut self, other: &Self) {
        self.request_count = self.request_count.saturating_add(other.request_count);
        self.methods.merge(&other.methods);
        self.status.merge(other.status);
        for (ct, &count) in &other.content_types {
            if let Some(existing) = self.content_types.get_mut(ct) {
                *existing = existing.saturating_add(count);
            } else if self.content_types.len() < MAX_CONTENT_TYPES_PER_NODE {
                let _ = self.content_types.insert(ct.clone(), count);
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(other.total_bytes);
        self.duration_sum_micros = self
            .duration_sum_micros
            .saturating_add(other.duration_sum_micros);
        self.duration_samples = self.duration_samples.saturating_add(other.duration_samples);
        self.max_duration_micros = self.max_duration_micros.max(other.max_duration_micros);
        if let Some(first) = other.first_seen {
            self.observe_time(first);
        }
        if let Some(last) = other.last_seen {
            self.observe_time(last);
        }
        self.third_party_hits = self.third_party_hits.saturating_add(other.third_party_hits);
        for (name, &count) in &other.query_params {
            if let Some(existing) = self.query_params.get_mut(name) {
                *existing = existing.saturating_add(count);
            } else if self.query_params.len() < MAX_QUERY_PARAMS_PER_NODE {
                let _ = self.query_params.insert(name.clone(), count);
            }
        }
        self.has_tunnel |= other.has_tunnel;
        self.overflow_merged = self.overflow_merged.saturating_add(other.overflow_merged);
        self.observed_id_kinds
            .extend(other.observed_id_kinds.iter().copied());
    }

    fn observe_time(&mut self, at: SystemTime) {
        self.first_seen = Some(self.first_seen.map_or(at, |existing| existing.min(at)));
        self.last_seen = Some(self.last_seen.map_or(at, |existing| existing.max(at)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_stats_timings_and_primary_content_type() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let later = now + Duration::from_secs(10);

        let mut a = NodeStats::default();
        a.record_request(&Method::GET, StatusBucket::S2xx, now, false, false);
        a.record_content_type("application/json");
        a.record_content_type("application/json");
        a.record_content_type("text/plain");
        a.add_bytes(100);
        a.record_duration(Duration::from_millis(20));

        let mut b = NodeStats::default();
        b.record_request(&Method::POST, StatusBucket::S4xx, later, true, false);
        b.record_content_type("text/plain");
        b.record_content_type("text/plain");
        b.add_bytes(250);
        b.record_duration(Duration::from_millis(60));

        a.merge(&b);
        assert_eq!(a.request_count, 2);
        assert_eq!(a.methods.names(), ["GET", "POST"]);
        assert_eq!(a.status.s2xx, 1);
        assert_eq!(a.status.s4xx, 1);
        assert_eq!(a.status.errors(), 1);
        assert_eq!(a.total_bytes, 350);
        assert_eq!(a.primary_content_type(), Some("text/plain"));
        assert_eq!(a.avg_duration_ms(), Some(40.0));
        assert_eq!(a.max_duration_ms(), Some(60.0));
        assert_eq!(a.first_seen, Some(now));
        assert_eq!(a.last_seen, Some(later));
        assert!(a.is_third_party());
    }

    #[test]
    fn failed_or_pending_tunnel_is_not_tunnel_only() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        let mut clean_tunnel = NodeStats::default();
        clean_tunnel.record_request(&Method::CONNECT, StatusBucket::Tunnel, now, false, true);
        assert!(clean_tunnel.is_tunnel_only());

        let mut failed_tunnel = NodeStats::default();
        failed_tunnel.record_request(&Method::CONNECT, StatusBucket::Failed, now, false, true);
        assert!(!failed_tunnel.is_tunnel_only());
        assert_eq!(failed_tunnel.status.errors(), 1);

        let mut pending_tunnel = NodeStats::default();
        pending_tunnel.record_request(&Method::CONNECT, StatusBucket::Pending, now, false, true);
        assert!(!pending_tunnel.is_tunnel_only());
    }
}
