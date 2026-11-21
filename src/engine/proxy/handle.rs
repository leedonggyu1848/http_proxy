use super::capture_context::{CaptureContext, EventNotifier, EventSink};
use super::forward::forward_http;
use super::inspect::connect_tunnel;
use super::status::{AcceptAction, EngineStatus, accept_action};
use super::{EngineState, ProxyBody, ProxyConfig};
use crate::engine::domain::{CaptureSource, Exchange, RecordId, SessionId};
use crate::engine::journal::FullLog;
use crate::engine::tls::{CaInfo, HttpsSetupError, TlsInspector};
use http::{Method, Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1 as server_http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Keeps one client connection, and any tunnel it upgrades into, counted
/// against the connection limit.
pub(super) type ConnectionPermit = Arc<OwnedSemaphorePermit>;

/// Shared state that every connection handled by one proxy run uses.
pub(super) struct Engine {
    pub(super) events: EventSink,
    pub(super) session: SessionId,
    pub(super) stopped: Arc<AtomicBool>,
    pub(super) inspector: Option<Arc<TlsInspector>>,
    pub(super) local_addr: SocketAddr,
    pub(super) connect_timeout: Duration,
    pub(super) response_timeout: Duration,
    pub(super) connections: Arc<Semaphore>,
}

/// A running proxy and its capture event stream.
pub struct ProxyHandle {
    pub(super) local_addr: SocketAddr,
    events: mpsc::Receiver<Exchange>,
    dropped: Arc<AtomicU64>,
    notifier: Arc<OnceLock<EventNotifier>>,
    ca_info: Option<CaInfo>,
    shutdown: watch::Sender<bool>,
    status: EngineStatus,
    thread: Option<JoinHandle<()>>,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) engine: Arc<Engine>,
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
        let runtime_handle = runtime.handle().clone();
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
            connections: Arc::new(Semaphore::new(max_connections)),
        });
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let status = EngineStatus::default();
        let engine_status = status.clone();
        let handle_engine = Arc::clone(&engine);
        let thread = thread::Builder::new()
            .name("http-inspector-proxy".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let result = run(listener, engine, shutdown_receiver).await;
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
            runtime: runtime_handle,
            engine: handle_engine,
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
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let connections = Arc::clone(&engine.connections);
    let mut failures = AcceptFailures::default();
    loop {
        tokio::select! {
            () = wait_for_shutdown(&mut shutdown) => return Ok(()),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let permit = tokio::select! {
                        () = wait_for_shutdown(&mut shutdown) => return Ok(()),
                        permit = Arc::clone(&connections).acquire_owned() => match permit {
                            Ok(permit) => Arc::new(permit),
                            Err(_) => return Ok(()),
                        },
                    };
                    failures.reset();
                    let connection_engine = Arc::clone(&engine);
                    tokio::spawn(async move {
                        serve_connection(stream, connection_engine, permit).await;
                    });
                }
                Err(error) => {
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

pub(super) fn server_builder() -> server_http1::Builder {
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
        let capture = CaptureContext::start(
            request.method(),
            request.headers(),
            request.extensions().get::<RecordId>().copied(),
            CaptureSource::Connect,
            &target,
            &engine,
        );
        connect_tunnel(&mut request, &engine, capture, permit).await
    } else {
        let capture = CaptureContext::start(
            request.method(),
            request.headers(),
            request.extensions().get::<RecordId>().copied(),
            CaptureSource::HttpProxy,
            &target,
            &engine,
        );
        forward_http(request.map(BodyExt::boxed), &engine, &capture).await
    };

    Ok(response)
}
