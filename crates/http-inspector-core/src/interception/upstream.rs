use super::capture_context::CaptureContext;
use super::handle::Engine;
use super::target::{ResolveError, resolve_upstream};
use super::{ProxyBody, failed_response};
use http::uri::Authority;
use http::{Response, StatusCode};
use http_inspector_exchange::FailureStage;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::time::timeout_at;

/// Why the proxy could not open an upstream connection.
pub(super) struct UpstreamFailure {
    stage: FailureStage,
    status: StatusCode,
    message: String,
}

impl UpstreamFailure {
    pub(super) const fn new(stage: FailureStage, status: StatusCode, message: String) -> Self {
        Self {
            stage,
            status,
            message,
        }
    }

    pub(super) fn connect_timeout(engine: &Engine) -> Self {
        Self {
            stage: FailureStage::ConnectTimeout,
            status: StatusCode::GATEWAY_TIMEOUT,
            message: format!(
                "the upstream connection was not established within {} ms",
                engine.connect_timeout.as_millis()
            ),
        }
    }

    pub(super) fn respond(&self, capture: &CaptureContext) -> Response<ProxyBody> {
        failed_response(capture, self.stage, self.status, &self.message)
    }
}

impl From<ResolveError> for UpstreamFailure {
    fn from(error: ResolveError) -> Self {
        match error {
            ResolveError::Lookup(error) => Self {
                stage: FailureStage::Connect,
                status: StatusCode::BAD_GATEWAY,
                message: error.to_string(),
            },
            ResolveError::Loop => Self {
                stage: FailureStage::Request,
                status: StatusCode::LOOP_DETECTED,
                message: "the request targets this proxy itself".to_owned(),
            },
        }
    }
}

pub(super) fn connect_deadline(engine: &Engine) -> tokio::time::Instant {
    tokio::time::Instant::now() + engine.connect_timeout
}

pub(super) async fn connect_upstream(
    authority: &Authority,
    default_port: u16,
    engine: &Engine,
) -> Result<TcpStream, UpstreamFailure> {
    let deadline = connect_deadline(engine);
    let addresses = resolve_within(deadline, authority, default_port, engine).await?;
    connect_within(deadline, &addresses, engine).await
}

pub(super) async fn resolve_within(
    deadline: tokio::time::Instant,
    authority: &Authority,
    default_port: u16,
    engine: &Engine,
) -> Result<Vec<SocketAddr>, UpstreamFailure> {
    timeout_at(
        deadline,
        resolve_upstream(authority, default_port, engine.local_addr),
    )
    .await
    .map_or_else(
        |_| Err(UpstreamFailure::connect_timeout(engine)),
        |result| result.map_err(UpstreamFailure::from),
    )
}

pub(super) async fn connect_within(
    deadline: tokio::time::Instant,
    addresses: &[SocketAddr],
    engine: &Engine,
) -> Result<TcpStream, UpstreamFailure> {
    timeout_at(deadline, connect_addresses(addresses))
        .await
        .unwrap_or_else(|_| Err(UpstreamFailure::connect_timeout(engine)))
}

async fn connect_addresses(addresses: &[SocketAddr]) -> Result<TcpStream, UpstreamFailure> {
    TcpStream::connect(addresses)
        .await
        .map_err(|error| UpstreamFailure {
            stage: FailureStage::Connect,
            status: StatusCode::BAD_GATEWAY,
            message: error.to_string(),
        })
}
