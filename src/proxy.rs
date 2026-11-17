use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use bytes::Bytes;
use http::header::{
    CONNECTION, HOST, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
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
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::{mpsc, watch};

use crate::{CaptureOutcome, CaptureRecord, FailureStage};

type ProxyBody = BoxBody<Bytes, hyper::Error>;

/// Settings used to start the local proxy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProxyConfig {
    bind_addr: SocketAddr,
    event_capacity: usize,
}

impl ProxyConfig {
    /// Creates a loopback-only configuration. Port zero selects an available port.
    #[must_use]
    pub const fn loopback(port: u16) -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            event_capacity: 512,
        }
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
    shutdown: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl ProxyHandle {
    /// Starts a proxy on the configured loopback address.
    pub fn start(config: ProxyConfig) -> Result<Self, StartError> {
        let listener = StdTcpListener::bind(config.bind_addr).map_err(StartError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(StartError::ConfigureListener)?;
        let local_addr = listener.local_addr().map_err(StartError::LocalAddress)?;
        let runtime = RuntimeBuilder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(StartError::Runtime)?;
        let (event_sender, events) = mpsc::channel(config.event_capacity);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let thread = thread::Builder::new()
            .name("http-inspector-proxy".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let result = run(listener, event_sender, shutdown_receiver).await;
                    if let Err(error) = result {
                        tracing::error!(%error, "proxy server stopped");
                    }
                });
            })
            .map_err(StartError::Thread)?;

        Ok(Self {
            local_addr,
            events,
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
}

/// Error returned while the proxy stops.
#[derive(Debug, Error)]
pub enum StopError {
    #[error("the proxy thread panicked")]
    Thread,
}

async fn run(
    listener: StdTcpListener,
    events: mpsc::Sender<CaptureRecord>,
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
                tokio::spawn(async move {
                    serve_connection(stream, connection_events).await;
                });
            }
        }
    }
}

async fn serve_connection(stream: TcpStream, events: mpsc::Sender<CaptureRecord>) {
    let service = service_fn(move |request| proxy_request(request, events.clone()));
    let connection = server_http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(error) = connection.await {
        tracing::debug!(%error, "client connection ended");
    }
}

async fn proxy_request(
    mut request: Request<Incoming>,
    events: mpsc::Sender<CaptureRecord>,
) -> Result<Response<ProxyBody>, Infallible> {
    let started = Instant::now();
    let method = request.method().clone();
    let target = request.uri().to_string();

    let response = if method == Method::CONNECT {
        connect_tunnel(&mut request, &events, &method, &target, started).await
    } else {
        forward_http(request, &events, &method, &target, started).await
    };

    Ok(response)
}

async fn connect_tunnel(
    request: &mut Request<Incoming>,
    events: &mpsc::Sender<CaptureRecord>,
    method: &Method,
    target: &str,
    started: Instant,
) -> Response<ProxyBody> {
    let Some(authority) = request.uri().authority().cloned() else {
        return failed_response(
            events,
            method,
            target,
            started,
            FailureStage::Request,
            StatusCode::BAD_REQUEST,
            "CONNECT requires an authority",
        )
        .await;
    };

    let upstream = match TcpStream::connect(authority.as_str()).await {
        Ok(stream) => stream,
        Err(error) => {
            return failed_response(
                events,
                method,
                target,
                started,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            )
            .await;
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

    send_record(
        events,
        CaptureRecord::new(
            method.clone(),
            target.to_owned(),
            CaptureOutcome::TunnelEstablished,
            started.elapsed(),
        ),
    )
    .await;

    empty_response(StatusCode::OK)
}

async fn forward_http(
    mut request: Request<Incoming>,
    events: &mpsc::Sender<CaptureRecord>,
    method: &Method,
    target: &str,
    started: Instant,
) -> Response<ProxyBody> {
    let target_parts = match upstream_target(&request) {
        Ok(parts) => parts,
        Err(message) => {
            return failed_response(
                events,
                method,
                target,
                started,
                FailureStage::Request,
                StatusCode::BAD_REQUEST,
                message,
            )
            .await;
        }
    };

    let stream = match TcpStream::connect(&target_parts.socket_target).await {
        Ok(stream) => stream,
        Err(error) => {
            return failed_response(
                events,
                method,
                target,
                started,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            )
            .await;
        }
    };

    let handshake = client_http1::handshake(TokioIo::new(stream)).await;
    let (mut sender, connection) = match handshake {
        Ok(parts) => parts,
        Err(error) => {
            return failed_response(
                events,
                method,
                target,
                started,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            )
            .await;
        }
    };

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "upstream HTTP connection ended");
        }
    });

    *request.uri_mut() = target_parts.path;
    remove_hop_by_hop_headers(request.headers_mut());

    match sender.send_request(request).await {
        Ok(mut response) => {
            remove_hop_by_hop_headers(response.headers_mut());
            let status = response.status();
            send_record(
                events,
                CaptureRecord::new(
                    method.clone(),
                    target.to_owned(),
                    CaptureOutcome::Response {
                        status: status.as_u16(),
                    },
                    started.elapsed(),
                ),
            )
            .await;
            response.map(BodyExt::boxed)
        }
        Err(error) => {
            failed_response(
                events,
                method,
                target,
                started,
                FailureStage::Response,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            )
            .await
        }
    }
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

fn remove_hop_by_hop_headers(headers: &mut http::HeaderMap) {
    headers.remove(CONNECTION);
    headers.remove(PROXY_AUTHORIZATION);
    headers.remove("proxy-connection");
    headers.remove("keep-alive");
    headers.remove(TE);
    headers.remove(TRAILER);
    headers.remove(TRANSFER_ENCODING);
    headers.remove(UPGRADE);
}

async fn failed_response(
    events: &mpsc::Sender<CaptureRecord>,
    method: &Method,
    target: &str,
    started: Instant,
    stage: FailureStage,
    status: StatusCode,
    message: &str,
) -> Response<ProxyBody> {
    send_record(
        events,
        CaptureRecord::new(
            method.clone(),
            target.to_owned(),
            CaptureOutcome::Failed {
                stage,
                message: message.to_owned(),
            },
            started.elapsed(),
        ),
    )
    .await;
    text_response(status, message)
}

async fn send_record(events: &mpsc::Sender<CaptureRecord>, record: CaptureRecord) {
    if events.send(record).await.is_err() {
        tracing::debug!("capture receiver closed");
    }
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
