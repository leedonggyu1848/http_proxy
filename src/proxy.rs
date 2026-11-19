use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{HOST, UPGRADE};
use http::uri::Authority;
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1 as client_http1;
use hyper::ext::ReasonPhrase;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::time::{timeout, timeout_at};

use crate::FullLog;
use crate::capture::{BodyLog, full_headers};
use crate::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, ObservedBody, TARGET_BYTES, bounded_text, capture_headers,
};
use crate::{BodyPreview, RecordId};
use crate::{
    CaInfo, Destination, HttpsInspection, HttpsSetupError, PreparedInspection, TlsInspector,
    UpstreamTlsError,
};
use crate::{
    CaptureSource, CapturedRequest, CapturedResponse, Exchange, FailureStage, Outcome, SessionId,
    TunnelPreview, TunnelSnapshot,
};

mod headers;
mod status;
mod target;
mod tunnel;

use headers::remove_hop_by_hop_headers;
pub use status::EngineState;
use status::{AcceptAction, EngineStatus, accept_action};
use target::{ResolveError, resolve_upstream, upstream_target};

type ProxyBody = BoxBody<Bytes, hyper::Error>;

static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(1);
const TLS_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_MAX_CONNECTIONS: usize = 256;

/// Keeps one client connection, and any tunnel it upgrades into, counted
/// against the connection limit.
type ConnectionPermit = Arc<OwnedSemaphorePermit>;

/// Shared state that every connection handled by one proxy run uses.
struct Engine {
    events: EventSink,
    session: SessionId,
    stopped: Arc<AtomicBool>,
    inspector: Option<Arc<TlsInspector>>,
    local_addr: SocketAddr,
    connect_timeout: Duration,
    response_timeout: Duration,
}

/// Callback run after an exchange was queued, for example to wake a UI.
type EventNotifier = Box<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct EventSink {
    sender: mpsc::Sender<Exchange>,
    dropped: Arc<AtomicU64>,
    notifier: Arc<OnceLock<EventNotifier>>,
    log: Option<FullLog>,
}

impl EventSink {
    fn send(&self, exchange: Exchange) {
        match self.sender.try_send(exchange) {
            Ok(()) => {
                if let Some(notify) = self.notifier.get() {
                    notify();
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

/// Engine-side owner of one exchange. Dropping it before the exchange
/// settles records why the request never finished.
struct CaptureContext {
    exchange: Exchange,
    log: Option<FullLog>,
    stopped: Arc<AtomicBool>,
}

impl CaptureContext {
    /// Publishes a new in-flight exchange for `request`.
    fn start(
        request: &Request<Incoming>,
        source: CaptureSource,
        target: &str,
        engine: &Engine,
    ) -> Self {
        let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        let log = engine.events.log.clone();
        if let Some(log) = log.as_ref() {
            log.text(format!(
                "[{id}] Request {} {target}\n{}",
                request.method(),
                full_headers(request.headers())
            ));
        }
        let exchange = Exchange::new(
            RecordId::new(id),
            engine.session,
            source,
            CapturedRequest::new(
                request.method().clone(),
                bounded_text(target, TARGET_BYTES),
                capture_headers(request.headers()),
            ),
        );
        engine.events.send(exchange.clone());
        Self {
            exchange,
            log,
            stopped: Arc::clone(&engine.stopped),
        }
    }

    fn id(&self) -> u64 {
        self.exchange.id().value()
    }

    fn body_log(&self, side: &'static str) -> Option<BodyLog> {
        self.log
            .as_ref()
            .map(|log| BodyLog::new(log.clone(), self.id(), side))
    }

    /// Records a failure and returns the bounded message shown to the client.
    fn fail(&self, stage: FailureStage, message: &str) -> String {
        if let Some(log) = self.log.as_ref() {
            log.text(format!(
                "[{}] Failure {stage:?}: {}\n",
                self.id(),
                message.escape_debug()
            ));
        }
        let message = bounded_text(message, ERROR_BYTES);
        self.settle(Outcome::Failed {
            stage,
            message: message.clone(),
        });
        message
    }

    fn settle(&self, outcome: Outcome) {
        let summary = outcome_summary(&outcome);
        let timing_label = if matches!(outcome, Outcome::Response(_)) {
            "Response headers in"
        } else {
            "Settled after"
        };
        if self.exchange.settle(outcome)
            && let Some(log) = self.log.as_ref()
        {
            log.text(format!(
                "[{}] {summary} · {timing_label} {:.1} ms\n",
                self.id(),
                self.exchange.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }

    fn tunnel_ended(&self, tunnel: &TunnelSnapshot) {
        if let Some(log) = self.log.as_ref() {
            log.text(format!(
                "[{}] Tunnel ended {:?} · {} bytes client→upstream · {} bytes upstream→client · {:.1} ms\n",
                self.id(),
                tunnel.state(),
                tunnel.client_to_upstream(),
                tunnel.upstream_to_client(),
                tunnel.duration().as_secs_f64() * 1000.0
            ));
        }
    }
}

impl Drop for CaptureContext {
    fn drop(&mut self) {
        if self.exchange.outcome().is_in_flight() {
            // The engine sets `stopped` before shutting its runtime down, so
            // any request dropped after that was cut off by the stop rather
            // than abandoned by its client.
            if self.stopped.load(Ordering::Acquire) {
                self.settle(Outcome::Incomplete);
            } else {
                self.settle(Outcome::Cancelled);
            }
        }
    }
}

fn outcome_summary(outcome: &Outcome) -> String {
    match outcome {
        Outcome::InFlight => "In flight".to_owned(),
        Outcome::Response(response) => format!("Response {}", response.status().as_u16()),
        Outcome::Tunnel(_) => {
            "CONNECT tunnel established (encrypted contents not inspected)".to_owned()
        }
        Outcome::InspectionEstablished => "HTTPS inspection established".to_owned(),
        Outcome::Failed { stage, .. } => format!("Failed {stage:?}"),
        Outcome::Cancelled => "Cancelled by the client".to_owned(),
        Outcome::Incomplete => "Incomplete because the proxy stopped".to_owned(),
    }
}

/// Settings used to start the local proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyConfig {
    bind_addr: SocketAddr,
    event_capacity: usize,
    https_inspection: Option<HttpsInspection>,
    connect_timeout: Duration,
    response_timeout: Duration,
    max_connections: usize,
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
    /// counted by [`ProxyHandle::dropped_records`] instead of blocking forwarding.
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

    /// Enables HTTPS inspection for this proxy run.
    #[must_use]
    pub fn with_https_inspection(mut self, settings: HttpsInspection) -> Self {
        self.https_inspection = Some(settings);
        self
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self::loopback(8080)
    }
}

/// A running proxy and its capture event stream.
pub struct ProxyHandle {
    local_addr: SocketAddr,
    events: mpsc::Receiver<Exchange>,
    dropped: Arc<AtomicU64>,
    notifier: Arc<OnceLock<EventNotifier>>,
    ca_info: Option<CaInfo>,
    shutdown: watch::Sender<bool>,
    status: EngineStatus,
    thread: Option<JoinHandle<()>>,
}

impl ProxyHandle {
    /// Starts a proxy on the configured loopback address.
    pub fn start(config: ProxyConfig) -> Result<Self, StartError> {
        Self::start_inner(config, None)
    }

    /// Starts a proxy that also records every observed exchange to a full-session log.
    pub fn start_recording(config: ProxyConfig, log: FullLog) -> Result<Self, StartError> {
        Self::start_inner(config, Some(log))
    }

    fn start_inner(config: ProxyConfig, log: Option<FullLog>) -> Result<Self, StartError> {
        let ProxyConfig {
            bind_addr,
            event_capacity,
            https_inspection,
            connect_timeout,
            response_timeout,
            max_connections,
        } = config;
        let (inspector, ca_info) = match https_inspection.as_ref() {
            Some(settings) => {
                let (inspector, info) = TlsInspector::open(settings).map_err(StartError::Https)?;
                (Some(inspector), Some(info))
            }
            None => (None, None),
        };
        let listener = StdTcpListener::bind(bind_addr).map_err(StartError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(StartError::ConfigureListener)?;
        let local_addr = listener.local_addr().map_err(StartError::LocalAddress)?;
        let runtime = RuntimeBuilder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(StartError::Runtime)?;
        let (event_sender, events) = mpsc::channel(event_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let notifier = Arc::new(OnceLock::new());
        let event_sink = EventSink {
            sender: event_sender,
            dropped: Arc::clone(&dropped),
            notifier: Arc::clone(&notifier),
            log,
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(Engine {
            events: event_sink,
            session: SessionId::next(),
            stopped: Arc::clone(&stopped),
            inspector,
            local_addr,
            connect_timeout,
            response_timeout,
        });
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let status = EngineStatus::default();
        let engine_status = status.clone();
        let thread = thread::Builder::new()
            .name("http-inspector-proxy".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let result = run(listener, engine, max_connections, shutdown_receiver).await;
                    if let Err(error) = result {
                        tracing::error!(%error, "proxy server stopped");
                        engine_status.fail(error.to_string());
                    }
                });
                stopped.store(true, Ordering::Release);
                drop(runtime);
            })
            .map_err(StartError::Thread)?;

        Ok(Self {
            local_addr,
            events,
            dropped,
            notifier,
            ca_info,
            shutdown,
            status,
            thread: Some(thread),
        })
    }

    /// Returns the address that clients use as their HTTP proxy.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Receives the next started exchange without waiting.
    ///
    /// Exchanges arrive when their request starts; their outcome keeps
    /// updating in place afterwards.
    pub fn try_receive(&mut self) -> Option<Exchange> {
        self.events.try_recv().ok()
    }

    /// Waits for the next started exchange.
    pub async fn receive(&mut self) -> Option<Exchange> {
        self.events.recv().await
    }

    /// Runs `notify` on the engine's threads after each queued exchange so a
    /// UI can wake up instead of polling. Only the first call has an effect;
    /// it returns `false` when a notifier was already set. `notify` must
    /// return quickly because it runs on the forwarding path.
    pub fn notify_on_event(&self, notify: impl Fn() + Send + Sync + 'static) -> bool {
        self.notifier.set(Box::new(notify)).is_ok()
    }

    /// Returns the number of exchanges omitted because the event queue was full.
    #[must_use]
    pub fn dropped_records(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Reports whether the engine is still accepting connections.
    #[must_use]
    pub fn state(&self) -> EngineState {
        let finished = self.thread.as_ref().is_none_or(JoinHandle::is_finished);
        self.status.state(finished)
    }

    /// Returns public CA metadata when HTTPS inspection is enabled.
    #[must_use]
    pub const fn ca_info(&self) -> Option<&CaInfo> {
        self.ca_info.as_ref()
    }

    /// Stops the listener and waits for its thread to finish.
    pub fn stop(mut self) -> Result<(), StopError> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<(), StopError> {
        let _receiver_closed = self.shutdown.send(true).is_err();
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| StopError::Thread)?;
        }
        Ok(())
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        if let Err(error) = self.stop_inner() {
            tracing::error!(%error, "failed to stop proxy thread");
        }
    }
}

/// Error returned while the proxy starts.
#[derive(Debug, Error)]
pub enum StartError {
    #[error("failed to bind the proxy listener: {0}")]
    Bind(io::Error),
    #[error("failed to configure the proxy listener: {0}")]
    ConfigureListener(io::Error),
    #[error("failed to read the proxy listener address: {0}")]
    LocalAddress(io::Error),
    #[error("failed to create the Tokio runtime: {0}")]
    Runtime(io::Error),
    #[error("failed to create the proxy thread: {0}")]
    Thread(io::Error),
    #[error("failed to prepare HTTPS inspection: {0}")]
    Https(HttpsSetupError),
}

/// Error returned while the proxy stops.
#[derive(Debug, Error)]
pub enum StopError {
    #[error("the proxy thread panicked")]
    Thread,
}

async fn run(
    listener: StdTcpListener,
    engine: Arc<Engine>,
    max_connections: usize,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let connections = Arc::new(Semaphore::new(max_connections));
    let mut failures = AcceptFailures::default();
    loop {
        let permit = tokio::select! {
            () = wait_for_shutdown(&mut shutdown) => return Ok(()),
            permit = Arc::clone(&connections).acquire_owned() => match permit {
                Ok(permit) => Arc::new(permit),
                Err(_) => return Ok(()),
            },
        };
        tokio::select! {
            () = wait_for_shutdown(&mut shutdown) => return Ok(()),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    failures.reset();
                    let connection_engine = Arc::clone(&engine);
                    tokio::spawn(async move {
                        serve_connection(stream, connection_engine, permit).await;
                    });
                }
                Err(error) => {
                    drop(permit);
                    if accept_action(&error) == AcceptAction::Stop {
                        return Err(error);
                    }
                    failures.report(&error);
                    tokio::select! {
                        () = wait_for_shutdown(&mut shutdown) => return Ok(()),
                        () = tokio::time::sleep(ACCEPT_RETRY_DELAY) => {}
                    }
                }
            },
        }
    }
}

/// Logs the first failure of a run of accept errors loudly and the rest quietly.
#[derive(Default)]
struct AcceptFailures {
    consecutive: u64,
}

impl AcceptFailures {
    fn report(&mut self, error: &io::Error) {
        self.consecutive = self.consecutive.saturating_add(1);
        if self.consecutive == 1 {
            tracing::warn!(%error, "accepting a client failed; retrying");
        } else {
            tracing::debug!(%error, count = self.consecutive, "accepting a client failed again");
        }
    }

    const fn reset(&mut self) {
        self.consecutive = 0;
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    let _sender_dropped = shutdown.wait_for(|stopping| *stopping).await.is_err();
}

fn server_builder() -> server_http1::Builder {
    let mut builder = server_http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    builder
}

async fn serve_connection(stream: TcpStream, engine: Arc<Engine>, permit: ConnectionPermit) {
    let service =
        service_fn(move |request| proxy_request(request, Arc::clone(&engine), Arc::clone(&permit)));
    let connection = server_builder()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(error) = connection.await {
        tracing::debug!(%error, "client connection ended");
    }
}

async fn proxy_request(
    mut request: Request<Incoming>,
    engine: Arc<Engine>,
    permit: ConnectionPermit,
) -> Result<Response<ProxyBody>, Infallible> {
    let target = request.uri().to_string();
    let response = if request.method() == Method::CONNECT {
        let capture = CaptureContext::start(&request, CaptureSource::Connect, &target, &engine);
        connect_tunnel(&mut request, &engine, capture, permit).await
    } else {
        let capture = CaptureContext::start(&request, CaptureSource::HttpProxy, &target, &engine);
        forward_http(request, &engine, &capture).await
    };

    Ok(response)
}

async fn connect_tunnel(
    request: &mut Request<Incoming>,
    engine: &Arc<Engine>,
    capture: CaptureContext,
    permit: ConnectionPermit,
) -> Response<ProxyBody> {
    let Some(authority) = request.uri().authority().cloned() else {
        return failed_response(
            &capture,
            FailureStage::Request,
            StatusCode::BAD_REQUEST,
            "CONNECT requires an authority",
        );
    };

    let connect_started = Instant::now();
    let deadline = connect_deadline(engine);
    let addresses = match resolve_within(deadline, &authority, 443, engine).await {
        Ok(addresses) => addresses,
        Err(failure) => return failure.respond(&capture),
    };

    if let Some(inspector) = engine.inspector.clone() {
        return connect_inspected(request, engine, capture, inspector, &authority, permit);
    }

    let upstream = match connect_within(deadline, &addresses, engine).await {
        Ok(stream) => stream,
        Err(failure) => return failure.respond(&capture),
    };
    capture.exchange.record_connect(connect_started.elapsed());

    let tunnel = TunnelPreview::new();
    capture.settle(Outcome::Tunnel(tunnel.clone()));
    let on_upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        let _permit = permit;
        let client = async { on_upgrade.await.map(TokioIo::new) };
        tunnel::relay(client, upstream, tunnel.clone()).await;
        capture.tunnel_ended(&tunnel.snapshot());
    });

    empty_response(StatusCode::OK)
}

fn connect_inspected(
    request: &mut Request<Incoming>,
    engine: &Arc<Engine>,
    capture: CaptureContext,
    inspector: Arc<TlsInspector>,
    authority: &Authority,
    permit: ConnectionPermit,
) -> Response<ProxyBody> {
    let destination = match Destination::from_authority(authority) {
        Ok(destination) => Arc::new(destination),
        Err(error) => {
            return failed_response(
                &capture,
                FailureStage::Request,
                StatusCode::BAD_REQUEST,
                &error.to_string(),
            );
        }
    };
    let prepared = match inspector.prepare(&destination) {
        Ok(prepared) => prepared,
        Err(error) => {
            return failed_response(
                &capture,
                FailureStage::ClientTls,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    let on_upgrade = hyper::upgrade::on(request);
    let connection_engine = Arc::clone(engine);
    tokio::spawn(async move {
        let _permit = permit;
        run_inspected_connection(
            on_upgrade,
            prepared,
            destination,
            inspector,
            connection_engine,
            capture,
        )
        .await;
    });

    empty_response(StatusCode::OK)
}

async fn run_inspected_connection(
    on_upgrade: hyper::upgrade::OnUpgrade,
    prepared: PreparedInspection,
    destination: Arc<Destination>,
    inspector: Arc<TlsInspector>,
    engine: Arc<Engine>,
    capture: CaptureContext,
) {
    let upgraded = match timeout(TLS_TIMEOUT, on_upgrade).await {
        Ok(Ok(upgraded)) => upgraded,
        Ok(Err(error)) => {
            capture.fail(FailureStage::Upgrade, &error.to_string());
            return;
        }
        Err(_) => {
            capture.fail(FailureStage::Upgrade, "CONNECT upgrade timed out");
            return;
        }
    };
    let tls = match timeout(TLS_TIMEOUT, prepared.accept(TokioIo::new(upgraded))).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            capture.fail(FailureStage::ClientTls, &error.to_string());
            return;
        }
        Err(_) => {
            capture.fail(FailureStage::ClientTls, "client TLS handshake timed out");
            return;
        }
    };

    capture.settle(Outcome::InspectionEstablished);
    let service = service_fn(move |request| {
        proxy_https_request(
            request,
            Arc::clone(&engine),
            Arc::clone(&inspector),
            Arc::clone(&destination),
        )
    });
    let connection = server_builder()
        .serve_connection(TokioIo::new(tls), service)
        .with_upgrades();
    if let Err(error) = connection.await {
        tracing::debug!(%error, "inspected HTTPS connection ended");
    }
}

async fn proxy_https_request(
    request: Request<Incoming>,
    engine: Arc<Engine>,
    inspector: Arc<TlsInspector>,
    destination: Arc<Destination>,
) -> Result<Response<ProxyBody>, Infallible> {
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let target = destination.absolute_target(path);
    let capture = CaptureContext::start(&request, CaptureSource::InspectedHttps, &target, &engine);

    let response = match validate_inner_request(&request, &destination) {
        Ok(()) => forward_https(request, &engine, &capture, &inspector, &destination).await,
        Err(message) => failed_response(
            &capture,
            FailureStage::Request,
            StatusCode::BAD_REQUEST,
            message,
        ),
    };
    Ok(response)
}

fn validate_inner_request(
    request: &Request<Incoming>,
    destination: &Destination,
) -> Result<(), &'static str> {
    if request.method() == Method::CONNECT {
        return Err("nested CONNECT is not supported");
    }
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return Err("HTTPS requests inside CONNECT must use origin-form targets");
    }
    if request.headers().contains_key(UPGRADE) {
        return Err("protocol upgrades inside inspected HTTPS are not supported");
    }
    let authority = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
        .ok_or("the inspected request has no valid Host header")?;
    if !destination.matches_authority(&authority) {
        return Err("the inspected Host header does not match the CONNECT authority");
    }
    Ok(())
}

async fn forward_http(
    request: Request<Incoming>,
    engine: &Engine,
    capture: &CaptureContext,
) -> Response<ProxyBody> {
    let target_parts = match upstream_target(&request) {
        Ok(parts) => parts,
        Err(message) => {
            return failed_response(
                capture,
                FailureStage::Request,
                StatusCode::BAD_REQUEST,
                message,
            );
        }
    };

    let connect_started = Instant::now();
    let stream = match connect_upstream(&target_parts.authority, 80, engine).await {
        Ok(stream) => stream,
        Err(failure) => return failure.respond(capture),
    };
    capture.exchange.record_connect(connect_started.elapsed());

    forward_over_stream(request, stream, engine, capture, target_parts.path).await
}

async fn forward_https(
    request: Request<Incoming>,
    engine: &Engine,
    capture: &CaptureContext,
    inspector: &TlsInspector,
    destination: &Destination,
) -> Response<ProxyBody> {
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| Uri::from_static("/"), |value| value.clone().into());
    let connect_started = Instant::now();
    let stream = match timeout(TLS_TIMEOUT, inspector.connect(destination)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(UpstreamTlsError::Connect(error))) => {
            return failed_response(
                capture,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
        Ok(Err(UpstreamTlsError::Handshake(error))) => {
            return failed_response(
                capture,
                FailureStage::UpstreamTls,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
        Err(_) => {
            return failed_response(
                capture,
                FailureStage::UpstreamTls,
                StatusCode::GATEWAY_TIMEOUT,
                "upstream TLS connection timed out",
            );
        }
    };
    capture.exchange.record_connect(connect_started.elapsed());
    forward_over_stream(request, stream, engine, capture, path).await
}

async fn forward_over_stream<IO>(
    mut request: Request<Incoming>,
    stream: IO,
    engine: &Engine,
    capture: &CaptureContext,
    path: Uri,
) -> Response<ProxyBody>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handshake = client_http1::handshake(TokioIo::new(stream)).await;
    let (mut sender, connection) = match handshake {
        Ok(parts) => parts,
        Err(error) => {
            return failed_response(
                capture,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "upstream HTTP connection ended");
        }
    });

    *request.uri_mut() = path;
    remove_hop_by_hop_headers(request.headers_mut());
    let request_body = BodyPreview::new(BODY_PREVIEW_BYTES);
    capture.exchange.attach_request_body(request_body.clone());
    let request =
        request.map(|body| ObservedBody::new(body, request_body, capture.body_log("Request")));

    let Ok(response) = timeout(engine.response_timeout, sender.send_request(request)).await else {
        return failed_response(
            capture,
            FailureStage::ResponseTimeout,
            StatusCode::GATEWAY_TIMEOUT,
            &format!(
                "no response headers within {} ms",
                engine.response_timeout.as_millis()
            ),
        );
    };
    match response {
        Ok(mut response) => {
            if let Some(log) = capture.log.as_ref() {
                log.text(format!(
                    "[{}] Response headers\n{}",
                    capture.id(),
                    full_headers(response.headers())
                ));
            }
            let response_body = BodyPreview::new(BODY_PREVIEW_BYTES);
            capture.settle(Outcome::Response(captured_response(
                &response,
                response_body.clone(),
            )));
            remove_hop_by_hop_headers(response.headers_mut());
            response
                .map(|body| ObservedBody::new(body, response_body, capture.body_log("Response")))
                .map(BodyExt::boxed)
        }
        Err(error) => failed_response(
            capture,
            FailureStage::Response,
            StatusCode::BAD_GATEWAY,
            &error.to_string(),
        ),
    }
}

fn captured_response<B>(response: &Response<B>, body: BodyPreview) -> CapturedResponse {
    let status = response.status();
    let reason = response.extensions().get::<ReasonPhrase>().map_or_else(
        || status.canonical_reason().unwrap_or_default().to_owned(),
        |phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned(),
    );
    CapturedResponse::new(
        status,
        response.version(),
        bounded_text(&reason, ERROR_BYTES),
        capture_headers(response.headers()),
        body,
    )
}

/// Why the proxy could not open an upstream connection.
struct UpstreamFailure {
    stage: FailureStage,
    status: StatusCode,
    message: String,
}

impl UpstreamFailure {
    fn connect_timeout(engine: &Engine) -> Self {
        Self {
            stage: FailureStage::ConnectTimeout,
            status: StatusCode::GATEWAY_TIMEOUT,
            message: format!(
                "the upstream connection was not established within {} ms",
                engine.connect_timeout.as_millis()
            ),
        }
    }

    fn respond(&self, capture: &CaptureContext) -> Response<ProxyBody> {
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

fn connect_deadline(engine: &Engine) -> tokio::time::Instant {
    tokio::time::Instant::now() + engine.connect_timeout
}

async fn connect_upstream(
    authority: &Authority,
    default_port: u16,
    engine: &Engine,
) -> Result<TcpStream, UpstreamFailure> {
    let deadline = connect_deadline(engine);
    let addresses = resolve_within(deadline, authority, default_port, engine).await?;
    connect_within(deadline, &addresses, engine).await
}

async fn resolve_within(
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

async fn connect_within(
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

fn failed_response(
    capture: &CaptureContext,
    stage: FailureStage,
    status: StatusCode,
    message: &str,
) -> Response<ProxyBody> {
    let message = capture.fail(stage, message);
    text_response(status, &message)
}

fn empty_response(status: StatusCode) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .body(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        .unwrap_or_else(|_| {
            Response::new(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        })
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::copy_from_slice(message.as_bytes()))
                .map_err(infallible_to_hyper)
                .boxed(),
        )
        .unwrap_or_else(|_| {
            Response::new(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        })
}

const fn infallible_to_hyper(error: Infallible) -> hyper::Error {
    match error {}
}
