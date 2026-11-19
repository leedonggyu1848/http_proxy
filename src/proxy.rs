use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

use crate::FullLog;
use crate::capture::{BodyLog, full_headers};
use crate::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, ObservedBody, TARGET_BYTES, bounded_text, capture_headers,
};
use crate::{BodyPreview, HeaderPreview, RecordId};
use crate::{
    CaInfo, Destination, HttpsInspection, HttpsSetupError, PreparedInspection, TlsInspector,
    UpstreamTlsError,
};
use crate::{CaptureOutcome, CaptureRecord, FailureStage};

mod headers;

use headers::remove_hop_by_hop_headers;

type ProxyBody = BoxBody<Bytes, hyper::Error>;

static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(1);
const TLS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct EventSink {
    sender: mpsc::Sender<CaptureRecord>,
    dropped: Arc<AtomicU64>,
    log: Option<FullLog>,
}

impl EventSink {
    fn send(&self, record: CaptureRecord) {
        match self.sender.try_send(record) {
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

struct CaptureContext {
    id: RecordId,
    method: Method,
    target: String,
    request_headers: HeaderPreview,
    started: Instant,
    log: Option<FullLog>,
}

impl CaptureContext {
    fn new(request: &Request<Incoming>, started: Instant, log: Option<FullLog>) -> Self {
        Self::new_with_target(request, started, &request.uri().to_string(), log)
    }

    fn new_with_target(
        request: &Request<Incoming>,
        started: Instant,
        target: &str,
        log: Option<FullLog>,
    ) -> Self {
        let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        if let Some(log) = log.as_ref() {
            log.text(format!(
                "[{id}] Request {} {target}\n{}",
                request.method(),
                full_headers(request.headers())
            ));
        }
        Self {
            id: RecordId::new(id),
            method: request.method().clone(),
            target: bounded_text(target, TARGET_BYTES),
            request_headers: capture_headers(request.headers()),
            started,
            log,
        }
    }

    fn body_log(&self, side: &'static str) -> Option<BodyLog> {
        self.log
            .as_ref()
            .map(|log| BodyLog::new(log.clone(), self.id.value(), side))
    }

    fn failure(&self, stage: FailureStage, message: &str) {
        if let Some(log) = self.log.as_ref() {
            log.text(format!(
                "[{}] Failure {stage:?}: {}\n",
                self.id.value(),
                message.escape_debug()
            ));
        }
    }

    fn record(&self, request_body: Option<BodyPreview>, outcome: CaptureOutcome) -> CaptureRecord {
        if let Some(log) = self.log.as_ref() {
            let result = match &outcome {
                CaptureOutcome::Response { status, .. } => format!("Response {status}"),
                CaptureOutcome::TunnelEstablished => {
                    "CONNECT tunnel established (encrypted contents not inspected)".to_owned()
                }
                CaptureOutcome::InspectionEstablished => "HTTPS inspection established".to_owned(),
                CaptureOutcome::Failed { stage, .. } => format!("Failed {stage:?}"),
            };
            log.text(format!(
                "[{}] {result} · Response headers in {:.1} ms\n",
                self.id.value(),
                self.started.elapsed().as_secs_f64() * 1000.0
            ));
        }
        CaptureRecord {
            id: self.id,
            method: self.method.clone(),
            target: self.target.clone(),
            request_headers: self.request_headers.clone(),
            request_body,
            outcome,
            elapsed: self.started.elapsed(),
        }
    }
}

/// Settings used to start the local proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyConfig {
    bind_addr: SocketAddr,
    event_capacity: usize,
    https_inspection: Option<HttpsInspection>,
}

impl ProxyConfig {
    /// Creates a loopback-only configuration. Port zero selects an available port.
    #[must_use]
    pub const fn loopback(port: u16) -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            event_capacity: 512,
            https_inspection: None,
        }
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
    events: mpsc::Receiver<CaptureRecord>,
    dropped: Arc<AtomicU64>,
    ca_info: Option<CaInfo>,
    shutdown: watch::Sender<bool>,
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
        let event_sink = EventSink {
            sender: event_sender,
            dropped: Arc::clone(&dropped),
            log,
        };
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let thread = thread::Builder::new()
            .name("http-inspector-proxy".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let result = run(listener, event_sink, inspector, shutdown_receiver).await;
                    if let Err(error) = result {
                        tracing::error!(%error, "proxy server stopped");
                    }
                });
            })
            .map_err(StartError::Thread)?;

        Ok(Self {
            local_addr,
            events,
            dropped,
            ca_info,
            shutdown,
            thread: Some(thread),
        })
    }

    /// Returns the address that clients use as their HTTP proxy.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Receives the next captured request without waiting.
    pub fn try_receive(&mut self) -> Option<CaptureRecord> {
        self.events.try_recv().ok()
    }

    /// Waits for the next captured request.
    pub async fn receive(&mut self) -> Option<CaptureRecord> {
        self.events.recv().await
    }

    /// Returns the number of records omitted because the event queue was full.
    #[must_use]
    pub fn dropped_records(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
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
    events: EventSink,
    inspector: Option<Arc<TlsInspector>>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let connection_events = events.clone();
                let connection_inspector = inspector.clone();
                tokio::spawn(async move {
                    serve_connection(stream, connection_events, connection_inspector).await;
                });
            }
        }
    }
}

async fn serve_connection(
    stream: TcpStream,
    events: EventSink,
    inspector: Option<Arc<TlsInspector>>,
) {
    let service =
        service_fn(move |request| proxy_request(request, events.clone(), inspector.clone()));
    let connection = server_http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(error) = connection.await {
        tracing::debug!(%error, "client connection ended");
    }
}

async fn proxy_request(
    mut request: Request<Incoming>,
    events: EventSink,
    inspector: Option<Arc<TlsInspector>>,
) -> Result<Response<ProxyBody>, Infallible> {
    let started = Instant::now();
    let capture = CaptureContext::new(&request, started, events.log.clone());

    let response = if request.method() == Method::CONNECT {
        connect_tunnel(&mut request, &events, capture, inspector).await
    } else {
        forward_http(request, &events, &capture).await
    };

    Ok(response)
}

async fn connect_tunnel(
    request: &mut Request<Incoming>,
    events: &EventSink,
    capture: CaptureContext,
    inspector: Option<Arc<TlsInspector>>,
) -> Response<ProxyBody> {
    let Some(authority) = request.uri().authority().cloned() else {
        return failed_response(
            events,
            &capture,
            None,
            FailureStage::Request,
            StatusCode::BAD_REQUEST,
            "CONNECT requires an authority",
        );
    };

    if let Some(inspector) = inspector {
        return connect_inspected(request, events, capture, inspector, &authority);
    }

    let upstream = match TcpStream::connect(authority.as_str()).await {
        Ok(stream) => stream,
        Err(error) => {
            return failed_response(
                events,
                &capture,
                None,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    let on_upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        if let Ok(upgraded) = on_upgrade.await {
            let mut client = TokioIo::new(upgraded);
            let mut upstream = upstream;
            if let Err(error) = copy_bidirectional(&mut client, &mut upstream).await {
                tracing::debug!(%error, "CONNECT tunnel ended");
            }
        }
    });

    events.send(capture.record(None, CaptureOutcome::TunnelEstablished));

    empty_response(StatusCode::OK)
}

fn connect_inspected(
    request: &mut Request<Incoming>,
    events: &EventSink,
    capture: CaptureContext,
    inspector: Arc<TlsInspector>,
    authority: &Authority,
) -> Response<ProxyBody> {
    let destination = match Destination::from_authority(authority) {
        Ok(destination) => Arc::new(destination),
        Err(error) => {
            return failed_response(
                events,
                &capture,
                None,
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
                events,
                &capture,
                None,
                FailureStage::ClientTls,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    let on_upgrade = hyper::upgrade::on(request);
    let connection_events = events.clone();
    tokio::spawn(async move {
        run_inspected_connection(
            on_upgrade,
            prepared,
            destination,
            inspector,
            connection_events,
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
    events: EventSink,
    capture: CaptureContext,
) {
    let upgraded = match timeout(TLS_TIMEOUT, on_upgrade).await {
        Ok(Ok(upgraded)) => upgraded,
        Ok(Err(error)) => {
            send_connection_failure(&events, &capture, FailureStage::Upgrade, &error.to_string());
            return;
        }
        Err(_) => {
            send_connection_failure(
                &events,
                &capture,
                FailureStage::Upgrade,
                "CONNECT upgrade timed out",
            );
            return;
        }
    };
    let tls = match timeout(TLS_TIMEOUT, prepared.accept(TokioIo::new(upgraded))).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            send_connection_failure(
                &events,
                &capture,
                FailureStage::ClientTls,
                &error.to_string(),
            );
            return;
        }
        Err(_) => {
            send_connection_failure(
                &events,
                &capture,
                FailureStage::ClientTls,
                "client TLS handshake timed out",
            );
            return;
        }
    };

    events.send(capture.record(None, CaptureOutcome::InspectionEstablished));
    let service = service_fn(move |request| {
        proxy_https_request(
            request,
            events.clone(),
            Arc::clone(&inspector),
            Arc::clone(&destination),
        )
    });
    let connection = server_http1::Builder::new()
        .serve_connection(TokioIo::new(tls), service)
        .with_upgrades();
    if let Err(error) = connection.await {
        tracing::debug!(%error, "inspected HTTPS connection ended");
    }
}

async fn proxy_https_request(
    request: Request<Incoming>,
    events: EventSink,
    inspector: Arc<TlsInspector>,
    destination: Arc<Destination>,
) -> Result<Response<ProxyBody>, Infallible> {
    let started = Instant::now();
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let target = destination.absolute_target(path);
    let capture = CaptureContext::new_with_target(&request, started, &target, events.log.clone());

    let response = match validate_inner_request(&request, &destination) {
        Ok(()) => forward_https(request, &events, &capture, &inspector, &destination).await,
        Err(message) => failed_response(
            &events,
            &capture,
            None,
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
    events: &EventSink,
    capture: &CaptureContext,
) -> Response<ProxyBody> {
    let target_parts = match upstream_target(&request) {
        Ok(parts) => parts,
        Err(message) => {
            return failed_response(
                events,
                capture,
                None,
                FailureStage::Request,
                StatusCode::BAD_REQUEST,
                message,
            );
        }
    };

    let stream = match TcpStream::connect(&target_parts.socket_target).await {
        Ok(stream) => stream,
        Err(error) => {
            return failed_response(
                events,
                capture,
                None,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
    };

    forward_over_stream(request, stream, events, capture, target_parts.path).await
}

async fn forward_https(
    request: Request<Incoming>,
    events: &EventSink,
    capture: &CaptureContext,
    inspector: &TlsInspector,
    destination: &Destination,
) -> Response<ProxyBody> {
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| Uri::from_static("/"), |value| value.clone().into());
    let stream = match timeout(TLS_TIMEOUT, inspector.connect(destination)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(UpstreamTlsError::Connect(error))) => {
            return failed_response(
                events,
                capture,
                None,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
        Ok(Err(UpstreamTlsError::Handshake(error))) => {
            return failed_response(
                events,
                capture,
                None,
                FailureStage::UpstreamTls,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
        Err(_) => {
            return failed_response(
                events,
                capture,
                None,
                FailureStage::UpstreamTls,
                StatusCode::GATEWAY_TIMEOUT,
                "upstream TLS connection timed out",
            );
        }
    };
    forward_over_stream(request, stream, events, capture, path).await
}

async fn forward_over_stream<IO>(
    mut request: Request<Incoming>,
    stream: IO,
    events: &EventSink,
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
                events,
                capture,
                None,
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
    let request = request
        .map(|body| ObservedBody::new(body, request_body.clone(), capture.body_log("Request")));

    match sender.send_request(request).await {
        Ok(mut response) => {
            if let Some(log) = capture.log.as_ref() {
                log.text(format!(
                    "[{}] Response headers\n{}",
                    capture.id.value(),
                    full_headers(response.headers())
                ));
            }
            let response_headers = capture_headers(response.headers());
            remove_hop_by_hop_headers(response.headers_mut());
            let status = response.status();
            let response_body = BodyPreview::new(BODY_PREVIEW_BYTES);
            events.send(capture.record(
                Some(request_body),
                CaptureOutcome::Response {
                    status: status.as_u16(),
                    headers: response_headers,
                    body: response_body.clone(),
                },
            ));
            response
                .map(|body| ObservedBody::new(body, response_body, capture.body_log("Response")))
                .map(BodyExt::boxed)
        }
        Err(error) => failed_response(
            events,
            capture,
            Some(request_body),
            FailureStage::Response,
            StatusCode::BAD_GATEWAY,
            &error.to_string(),
        ),
    }
}

fn send_connection_failure(
    events: &EventSink,
    capture: &CaptureContext,
    stage: FailureStage,
    message: &str,
) {
    capture.failure(stage, message);
    let message = bounded_text(message, ERROR_BYTES);
    events.send(capture.record(None, CaptureOutcome::Failed { stage, message }));
}

struct UpstreamTarget {
    socket_target: String,
    path: Uri,
}

fn upstream_target(request: &Request<Incoming>) -> Result<UpstreamTarget, &'static str> {
    if let Some(scheme) = request.uri().scheme_str()
        && scheme != "http"
    {
        return Err("only http:// requests are supported without CONNECT");
    }

    let authority = request
        .uri()
        .authority()
        .cloned()
        .or_else(|| {
            request
                .headers()
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<Authority>().ok())
        })
        .ok_or("the request has no upstream authority")?;
    let path = match request.uri().path_and_query() {
        Some(value) => value
            .as_str()
            .parse()
            .map_err(|_| "the request path is invalid")?,
        None => Uri::from_static("/"),
    };
    let host = authority.host();
    let port = authority.port_u16().unwrap_or(80);
    let socket_target = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    Ok(UpstreamTarget {
        socket_target,
        path,
    })
}

fn failed_response(
    events: &EventSink,
    capture: &CaptureContext,
    request_body: Option<BodyPreview>,
    stage: FailureStage,
    status: StatusCode,
    message: &str,
) -> Response<ProxyBody> {
    capture.failure(stage, message);
    let message = bounded_text(message, ERROR_BYTES);
    events.send(capture.record(
        request_body,
        CaptureOutcome::Failed {
            stage,
            message: message.clone(),
        },
    ));
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
