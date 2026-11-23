use crate::engine::tls::HttpsInspection;
use crate::recording::body_store::BodyStore;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_MAX_CONNECTIONS: usize = 256;

/// Settings used to start the local proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyConfig {
    pub(super) bind_addr: SocketAddr,
    pub(super) event_capacity: usize,
    pub(super) https_inspection: Option<HttpsInspection>,
    pub(super) connect_timeout: Duration,
    pub(super) response_timeout: Duration,
    pub(super) max_connections: usize,
    pub(super) body_store: Option<BodyStore>,
}

impl ProxyConfig {
    /// Creates a loopback-only configuration. Port zero selects an available port.
    #[must_use]
    pub const fn loopback(port: u16) -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            event_capacity: 512,
            https_inspection: None,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            body_store: None,
        }
    }

    /// Limits name resolution plus TCP connection to an upstream server.
    #[must_use]
    pub const fn with_connect_timeout(mut self, limit: Duration) -> Self {
        self.connect_timeout = limit;
        self
    }

    /// Limits the wait from sending a request upstream to receiving response headers.
    #[must_use]
    pub const fn with_response_timeout(mut self, limit: Duration) -> Self {
        self.response_timeout = limit;
        self
    }

    /// Limits capture events waiting for the UI. Events beyond the limit are
    /// counted by [`super::ProxyHandle::dropped_records`] instead of blocking forwarding.
    /// Zero is treated as one.
    #[must_use]
    pub const fn with_event_capacity(mut self, limit: usize) -> Self {
        self.event_capacity = if limit == 0 { 1 } else { limit };
        self
    }

    /// Limits concurrently served client connections, including open tunnels.
    ///
    /// Further clients wait in the listen backlog until a connection closes.
    /// Zero is treated as one.
    #[must_use]
    pub const fn with_max_connections(mut self, limit: usize) -> Self {
        self.max_connections = if limit == 0 { 1 } else { limit };
        self
    }

    /// Copies every forwarded body into `store` so it can be saved later.
    ///
    /// Copying never waits for the disk; bodies the store cannot keep up
    /// with are marked incomplete there while forwarding continues.
    #[must_use]
    pub fn with_body_store(mut self, store: BodyStore) -> Self {
        self.body_store = Some(store);
        self
    }

    /// Enables HTTPS inspection for this proxy run.
    #[must_use]
    pub fn with_https_inspection(mut self, settings: HttpsInspection) -> Self {
        self.https_inspection = Some(settings);
        self
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self::loopback(crate::DEFAULT_PORT)
    }
}
