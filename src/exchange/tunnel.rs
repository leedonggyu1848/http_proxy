//! Live observation of a CONNECT tunnel that the proxy relays without decrypting.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{FailureStage, lock_recovering};

#[derive(Debug)]
struct TunnelInner {
    opened: Instant,
    client_to_upstream: AtomicU64,
    upstream_to_client: AtomicU64,
    end: Mutex<Option<TunnelEnd>>,
}

#[derive(Clone, Debug)]
struct TunnelEnd {
    at: Instant,
    state: TunnelState,
}

/// Shared, updating view of one CONNECT tunnel.
///
/// This is a read handle: only the relay that owns the tunnel writer
/// counts bytes and ends the tunnel.
///
/// ```compile_fail,E0599
/// fn close(tunnel: &http_proxy::exchange::TunnelPreview) {
///     tunnel.close();
/// }
/// ```
#[derive(Clone, Debug)]
pub struct TunnelPreview {
    inner: Arc<TunnelInner>,
}

/// Write side of one [`TunnelPreview`], owned by the relay.
#[derive(Debug)]
pub(crate) struct TunnelWriter {
    preview: TunnelPreview,
}

impl TunnelWriter {
    pub(crate) fn new() -> Self {
        Self {
            preview: TunnelPreview {
                inner: Arc::new(TunnelInner {
                    opened: Instant::now(),
                    client_to_upstream: AtomicU64::new(0),
                    upstream_to_client: AtomicU64::new(0),
                    end: Mutex::new(None),
                }),
            },
        }
    }

    /// Returns a read handle that follows this writer.
    pub(crate) fn preview(&self) -> TunnelPreview {
        self.preview.clone()
    }

    pub(crate) fn add_client_to_upstream(&self, bytes: usize) {
        add(&self.preview.inner.client_to_upstream, bytes);
    }

    pub(crate) fn add_upstream_to_client(&self, bytes: usize) {
        add(&self.preview.inner.upstream_to_client, bytes);
    }

    pub(crate) fn close(&self) {
        self.finish(TunnelState::Closed);
    }

    pub(crate) fn fail(&self, stage: FailureStage, message: String) {
        self.finish(TunnelState::Failed { stage, message });
    }

    /// Ends a tunnel that the proxy stopped relaying before either side finished.
    pub(crate) fn abandon(&self) {
        self.finish(TunnelState::Incomplete);
    }

    fn finish(&self, state: TunnelState) {
        let mut end = lock_recovering(&self.preview.inner.end);
        if end.is_none() {
            *end = Some(TunnelEnd {
                at: Instant::now(),
                state,
            });
        }
    }
}

impl TunnelPreview {
    /// Returns the transfer totals and state at this instant.
    #[must_use]
    pub fn snapshot(&self) -> TunnelSnapshot {
        let end = lock_recovering(&self.inner.end).clone();
        let (state, until) = end.map_or_else(
            || (TunnelState::Open, Instant::now()),
            |end| (end.state, end.at),
        );
        TunnelSnapshot {
            client_to_upstream: self.inner.client_to_upstream.load(Ordering::Relaxed),
            upstream_to_client: self.inner.upstream_to_client.load(Ordering::Relaxed),
            duration: until.saturating_duration_since(self.inner.opened),
            state,
        }
    }
}

fn add(counter: &AtomicU64, bytes: usize) {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    let _previous = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(bytes))
    });
}

/// Lifecycle of a CONNECT tunnel after the proxy answered `200`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TunnelState {
    /// Bytes may still flow.
    Open,
    /// Both sides finished without an error.
    Closed,
    /// The tunnel could not start or ended with an error.
    Failed {
        stage: FailureStage,
        message: String,
    },
    /// The proxy stopped while the tunnel was open.
    Incomplete,
}

impl std::fmt::Display for TunnelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => f.write_str("Open"),
            Self::Closed => f.write_str("Closed"),
            Self::Failed { stage, message } => write!(f, "Failed at {stage}: {message}"),
            Self::Incomplete => f.write_str("Incomplete: the proxy stopped while it was open"),
        }
    }
}

/// Immutable view of a tunnel at one instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TunnelSnapshot {
    client_to_upstream: u64,
    upstream_to_client: u64,
    duration: Duration,
    state: TunnelState,
}

impl TunnelSnapshot {
    /// Returns bytes relayed from the client to the upstream server.
    #[must_use]
    pub const fn client_to_upstream(&self) -> u64 {
        self.client_to_upstream
    }

    /// Returns bytes relayed from the upstream server to the client.
    #[must_use]
    pub const fn upstream_to_client(&self) -> u64 {
        self.upstream_to_client
    }

    /// Returns the time since the tunnel was accepted, up to its end when it ended.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    #[must_use]
    pub const fn state(&self) -> &TunnelState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_first_end_and_freezes_duration() {
        let tunnel = TunnelWriter::new();
        let view = tunnel.preview();
        tunnel.add_client_to_upstream(3);
        tunnel.add_upstream_to_client(5);
        assert_eq!(view.snapshot().state(), &TunnelState::Open);

        tunnel.close();
        tunnel.fail(FailureStage::Tunnel, "late".to_owned());
        tunnel.abandon();
        let first = view.snapshot();
        assert_eq!(first.state(), &TunnelState::Closed);
        assert_eq!(first.client_to_upstream(), 3);
        assert_eq!(first.upstream_to_client(), 5);
        assert_eq!(view.snapshot().duration(), first.duration());
    }

    #[test]
    fn formats_every_tunnel_state() {
        assert_eq!(TunnelState::Open.to_string(), "Open");
        assert_eq!(TunnelState::Closed.to_string(), "Closed");
        assert_eq!(
            TunnelState::Failed {
                stage: FailureStage::Tunnel,
                message: "reset".to_owned(),
            }
            .to_string(),
            "Failed at Tunnel relay: reset"
        );
        assert_eq!(
            TunnelState::Incomplete.to_string(),
            "Incomplete: the proxy stopped while it was open"
        );
    }
}
