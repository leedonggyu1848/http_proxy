use std::time::Duration;

use http::Method;

/// One HTTP exchange observed by the proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRecord {
    method: Method,
    target: String,
    outcome: CaptureOutcome,
    elapsed: Duration,
}

impl CaptureRecord {
    pub(crate) const fn new(
        method: Method,
        target: String,
        outcome: CaptureOutcome,
        elapsed: Duration,
    ) -> Self {
        Self {
            method,
            target,
            outcome,
            elapsed,
        }
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

    /// Returns the observed result.
    #[must_use]
    pub const fn outcome(&self) -> &CaptureOutcome {
        &self.outcome
    }

    /// Returns the time until the response headers or tunnel connection arrived.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

/// Result observed for a captured request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureOutcome {
    /// An HTTP response arrived.
    Response { status: u16 },
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
