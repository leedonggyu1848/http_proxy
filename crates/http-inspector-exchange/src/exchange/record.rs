//! One HTTP exchange, published when the request starts and settled once.

use std::future::poll_fn;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::{Duration, Instant, SystemTime};

use http::{Method, StatusCode, Version};

use super::lock_recovering;
use super::wakers::WakerSet;
use super::{
    AuthScheme, BodyLifecycle, BodyPreview, FailureStage, HeaderPreview, MediaType, RecordId,
    TunnelPreview, TunnelState,
};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);
static IN_FLIGHT: Outcome = Outcome::InFlight;

/// Identity of one upstream connection used to forward HTTP exchanges.
///
/// Exchanges that reuse the same pooled connection share one `ConnectionId`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// Creates a connection identity with a specific numeric value for tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Allocates an identity no other upstream connection in this process has.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the numeric connection identity used in logs and exports.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Upstream connection metadata recorded for a forwarded exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpstreamConnection {
    id: ConnectionId,
    reused: bool,
}

impl UpstreamConnection {
    /// Records a newly established upstream connection.
    #[must_use]
    pub const fn fresh(id: ConnectionId) -> Self {
        Self { id, reused: false }
    }

    /// Records an existing pooled upstream connection reused for this exchange.
    #[must_use]
    pub const fn reused(id: ConnectionId) -> Self {
        Self { id, reused: true }
    }

    /// Returns the identity of the upstream connection.
    #[must_use]
    pub const fn id(self) -> ConnectionId {
        self.id
    }

    /// Reports whether the exchange reused an existing pooled connection.
    #[must_use]
    pub const fn is_reused(self) -> bool {
        self.reused
    }
}

/// Identity of one proxy run. Records from different runs never share it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(u64);

impl SessionId {
    /// Allocates an identity no other proxy session in this process has.
    #[must_use]
    pub fn next() -> Self {
        Self(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the numeric identity used in logs.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Path through the proxy by which an exchange was captured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureSource {
    /// A plain `http://` request forwarded by the proxy.
    HttpProxy,
    /// A `CONNECT` request that the proxy relays or inspects.
    Connect,
    /// A request decrypted inside an inspected `CONNECT` session.
    InspectedHttps,
}

/// Request line and headers as they arrived from the client.
#[derive(Debug)]
pub struct CapturedRequest {
    method: Method,
    target: String,
    headers: HeaderPreview,
    auth_schemes: Vec<AuthScheme>,
    media_type: Option<MediaType>,
    body: OnceLock<BodyPreview>,
}

impl CapturedRequest {
    /// Creates a captured request from bounded headers and extracted auth schemes.
    #[must_use]
    pub fn new(
        method: Method,
        target: String,
        headers: HeaderPreview,
        auth_schemes: Vec<AuthScheme>,
    ) -> Self {
        Self {
            method,
            target,
            media_type: content_media_type(&headers),
            headers,
            auth_schemes,
            body: OnceLock::new(),
        }
    }

    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.method
    }

    /// Returns the absolute request target or `CONNECT` authority.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns the original parsed request headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderPreview {
        &self.headers
    }

    /// Returns recognized authentication scheme names observed on the request,
    /// without retaining any credential or cookie values.
    #[must_use]
    pub fn auth_schemes(&self) -> &[AuthScheme] {
        &self.auth_schemes
    }

    /// Returns the media type of the first retained `Content-Type`, or
    /// `None` when it is missing or not a valid media type.
    #[must_use]
    pub const fn media_type(&self) -> Option<&MediaType> {
        self.media_type.as_ref()
    }

    /// Returns the body preview once forwarding began, or `None` when the
    /// proxy never read the request body.
    #[must_use]
    pub fn body(&self) -> Option<&BodyPreview> {
        self.body.get()
    }
}

/// Parses the first retained `Content-Type` once, where headers are captured.
fn content_media_type(headers: &HeaderPreview) -> Option<MediaType> {
    headers
        .get("content-type")
        .next()
        .and_then(MediaType::parse)
}

/// Response head received from the upstream server and its streamed body.
#[derive(Clone, Debug)]
pub struct CapturedResponse {
    status: StatusCode,
    version: Version,
    reason: String,
    headers: HeaderPreview,
    media_type: Option<MediaType>,
    body: BodyPreview,
}

impl CapturedResponse {
    /// Creates a captured response from bounded headers and its streamed body preview.
    #[must_use]
    pub fn new(
        status: StatusCode,
        version: Version,
        reason: String,
        headers: HeaderPreview,
        body: BodyPreview,
    ) -> Self {
        Self {
            status,
            version,
            reason,
            media_type: content_media_type(&headers),
            headers,
            body,
        }
    }

    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns the HTTP version the upstream server answered with.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// Returns the reason phrase as sent, or the canonical one when the
    /// server sent the standard phrase or none.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the original parsed response headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderPreview {
        &self.headers
    }

    /// Returns the media type of the first retained `Content-Type`, or
    /// `None` when it is missing or not a valid media type.
    #[must_use]
    pub const fn media_type(&self) -> Option<&MediaType> {
        self.media_type.as_ref()
    }

    /// Returns the live response-body preview.
    #[must_use]
    pub const fn body(&self) -> &BodyPreview {
        &self.body
    }
}

/// What became of an exchange.
///
/// An exchange starts [`InFlight`](Self::InFlight) and moves to exactly one
/// other state. Body streaming after the response head is tracked by the
/// response's [`BodyPreview`], not by this state.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// The proxy is still working on the request.
    InFlight,
    /// Response headers arrived from the upstream server.
    Response(CapturedResponse),
    /// The proxy relays an opaque `CONNECT` tunnel that keeps updating until it ends.
    Tunnel(TunnelPreview),
    /// The proxy established a TLS inspection session for a `CONNECT` request.
    InspectionEstablished,
    /// The proxy gave up before response headers arrived.
    Failed {
        stage: FailureStage,
        message: String,
    },
    /// The client went away before the proxy settled the request.
    Cancelled,
    /// The proxy stopped before the request settled.
    Incomplete,
}

impl Outcome {
    /// Reports failures, client cancellation, and observations cut short by shutdown.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        match self {
            Self::Failed { .. } | Self::Cancelled | Self::Incomplete => true,
            Self::Tunnel(tunnel) => matches!(
                tunnel.snapshot().state(),
                TunnelState::Failed { .. } | TunnelState::Incomplete
            ),
            Self::InFlight | Self::Response(_) | Self::InspectionEstablished => false,
        }
    }

    /// Reports whether the exchange has not settled yet.
    #[must_use]
    pub const fn is_in_flight(&self) -> bool {
        matches!(self, Self::InFlight)
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InFlight => f.write_str("In flight"),
            Self::Response(response) => write!(f, "Response {}", response.status().as_u16()),
            Self::Tunnel(_) => {
                f.write_str("CONNECT tunnel established (encrypted contents not inspected)")
            }
            Self::InspectionEstablished => f.write_str("HTTPS inspection established"),
            Self::Failed { stage, .. } => write!(f, "Failed {stage}"),
            Self::Cancelled => f.write_str("Cancelled by the client"),
            Self::Incomplete => f.write_str("Incomplete because the proxy stopped"),
        }
    }
}

/// Observed start offset and duration of one phase relative to request start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseSpan {
    start: Duration,
    duration: Duration,
}

impl PhaseSpan {
    /// Creates an observed phase span starting at `start` after request arrival.
    #[must_use]
    pub const fn new(start: Duration, duration: Duration) -> Self {
        Self { start, duration }
    }

    /// Returns when this phase started relative to request start.
    #[must_use]
    pub const fn start(&self) -> Duration {
        self.start
    }

    /// Returns how long this phase lasted.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns when this phase ended relative to request start.
    #[must_use]
    pub const fn end(&self) -> Duration {
        self.start.saturating_add(self.duration)
    }
}

/// Observed durations and phase spans of an exchange. A phase the proxy did
/// not observe, or that has not finished yet, is `None`, never zero.
///
/// For a response the phases follow each other: `connect`, then `send`
/// until the client's request body was fully forwarded, then `wait` until
/// response headers arrived, then `receive` until the response body ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    connect: Option<PhaseSpan>,
    send: Option<PhaseSpan>,
    wait: Option<PhaseSpan>,
    receive: Option<PhaseSpan>,
    settled: Option<Duration>,
    complete: Option<Duration>,
}

impl Timing {
    /// Returns the time spent resolving and connecting to the upstream
    /// server, including the TLS handshake for inspected HTTPS.
    #[must_use]
    pub const fn connect(&self) -> Option<Duration> {
        match self.connect {
            Some(span) => Some(span.duration),
            None => None,
        }
    }

    /// Returns the observed start offset and duration of the `connect` phase.
    #[must_use]
    pub const fn connect_span(&self) -> Option<PhaseSpan> {
        self.connect
    }

    /// Returns the time from handing the request to the upstream connection
    /// until the request body ended.
    #[must_use]
    pub const fn send(&self) -> Option<Duration> {
        match self.send {
            Some(span) => Some(span.duration),
            None => None,
        }
    }

    /// Returns the observed start offset and duration of the `send` phase.
    #[must_use]
    pub const fn send_span(&self) -> Option<PhaseSpan> {
        self.send
    }

    /// Returns the time from the end of the request body until response
    /// headers arrived (time to first byte). `None` when the server answered
    /// before the request body ended.
    #[must_use]
    pub const fn wait(&self) -> Option<Duration> {
        match self.wait {
            Some(span) => Some(span.duration),
            None => None,
        }
    }

    /// Returns the observed start offset and duration of the `wait` phase.
    #[must_use]
    pub const fn wait_span(&self) -> Option<PhaseSpan> {
        self.wait
    }

    /// Returns the time from response headers until the response body ended,
    /// whether it completed, was interrupted, or was cancelled.
    #[must_use]
    pub const fn receive(&self) -> Option<Duration> {
        match self.receive {
            Some(span) => Some(span.duration),
            None => None,
        }
    }

    /// Returns the observed start offset and duration of the `receive` phase.
    #[must_use]
    pub const fn receive_span(&self) -> Option<PhaseSpan> {
        self.receive
    }

    /// Returns the time from the request start until nothing more will be
    /// observed: the end of the response body for a response, or the
    /// settlement for every other outcome. `None` while a response body or
    /// the request is still in progress.
    #[must_use]
    pub const fn complete(&self) -> Option<Duration> {
        self.complete
    }

    /// Returns the time from the request start until the exchange left
    /// [`Outcome::InFlight`]; for a response, this is when its headers arrived.
    #[must_use]
    pub const fn settled(&self) -> Option<Duration> {
        self.settled
    }
}

#[derive(Debug)]
struct Settlement {
    outcome: Outcome,
    after: Duration,
}

#[derive(Debug)]
struct ExchangeInner {
    id: RecordId,
    session: SessionId,
    source: CaptureSource,
    replay_of: OnceLock<RecordId>,
    started_at: SystemTime,
    started: Instant,
    request: CapturedRequest,
    connect: OnceLock<Duration>,
    upstream_connection: Mutex<Option<UpstreamConnection>>,
    send_started: OnceLock<Instant>,
    settlement: OnceLock<Settlement>,
    settled_wakers: WakerSet,
}

/// One HTTP exchange observed by the proxy.
///
/// The engine publishes it when the request arrives and fills in the rest as
/// forwarding progresses, so a clone always shows the current state.
///
/// This is a read handle. Only the capture path that started the exchange
/// owns its writer, and a read handle cannot be turned into one, so code
/// that receives an `Exchange` can observe it but never change it:
///
/// ```
/// fn has_settled(exchange: &http_inspector_exchange::Exchange) -> bool {
///     !exchange.outcome().is_in_flight()
/// }
/// ```
///
/// ```compile_fail,E0599
/// fn cancel(exchange: &http_inspector_exchange::Exchange) {
///     exchange.settle(http_inspector_exchange::Outcome::Cancelled);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Exchange {
    inner: Arc<ExchangeInner>,
}

/// Write side of one [`Exchange`], owned by the capture path that started it.
///
/// It is the only way to record progress on an exchange. It is neither
/// `Clone` nor obtainable from an [`Exchange`], so whoever starts a capture
/// decides who may change it.
#[derive(Debug)]
pub struct ExchangeWriter {
    exchange: Exchange,
}

impl ExchangeWriter {
    /// Starts an in-flight exchange for a request that just arrived.
    #[must_use]
    pub fn start(
        id: RecordId,
        session: SessionId,
        source: CaptureSource,
        request: CapturedRequest,
    ) -> Self {
        Self {
            exchange: Exchange {
                inner: Arc::new(ExchangeInner {
                    id,
                    session,
                    source,
                    replay_of: OnceLock::new(),
                    started_at: SystemTime::now(),
                    started: Instant::now(),
                    request,
                    connect: OnceLock::new(),
                    upstream_connection: Mutex::new(None),
                    send_started: OnceLock::new(),
                    settlement: OnceLock::new(),
                    settled_wakers: WakerSet::default(),
                }),
            },
        }
    }

    /// Returns the read handle that this writer updates.
    #[must_use]
    pub const fn exchange(&self) -> &Exchange {
        &self.exchange
    }

    /// Links this exchange to the original record it replayed.
    pub fn set_replay_of(&self, original: RecordId) {
        let _already_set = self.exchange.inner.replay_of.set(original).is_err();
    }

    /// Attaches the request-body preview as the request is handed to the
    /// upstream connection, which starts the `send` phase.
    pub fn attach_request_body(&self, body: BodyPreview) {
        let inner = &self.exchange.inner;
        let _already_started = inner.send_started.set(Instant::now()).is_err();
        let _already_attached = inner.request.body.set(body).is_err();
    }

    /// Records how long the upstream connection took to establish.
    pub fn record_connect(&self, duration: Duration) {
        let _already_recorded = self.exchange.inner.connect.set(duration).is_err();
    }

    /// Records which upstream connection forwarded this exchange and whether
    /// it was reused from the connection pool.
    ///
    /// Calling this again before settlement updates the recorded connection,
    /// for example when an idempotent request retries on a fresh connection
    /// after a stale pooled connection closed.
    pub fn record_upstream_connection(&self, connection: UpstreamConnection) {
        *lock_recovering(&self.exchange.inner.upstream_connection) = Some(connection);
    }

    /// Moves the exchange out of [`Outcome::InFlight`]. Returns `false` and
    /// keeps the first outcome when the exchange had already settled.
    #[expect(
        clippy::must_use_candidate,
        reason = "most callers settle once and do not branch on whether an earlier outcome won"
    )]
    pub fn settle(&self, outcome: Outcome) -> bool {
        let inner = &self.exchange.inner;
        let settlement = Settlement {
            outcome,
            after: inner.started.elapsed(),
        };
        let settled = inner.settlement.set(settlement).is_ok();
        if settled {
            inner.settled_wakers.wake_all();
        }
        settled
    }
}

impl Exchange {
    /// Returns the original record when this exchange was manually replayed.
    #[must_use]
    pub fn replay_of(&self) -> Option<RecordId> {
        self.inner.replay_of.get().copied()
    }

    /// Returns the stable exchange identity.
    #[must_use]
    pub fn id(&self) -> RecordId {
        self.inner.id
    }

    /// Returns the proxy run that captured this exchange.
    #[must_use]
    pub fn session(&self) -> SessionId {
        self.inner.session
    }

    /// Returns the path through the proxy that captured this exchange.
    #[must_use]
    pub fn source(&self) -> CaptureSource {
        self.inner.source
    }

    /// Returns the wall-clock time at which the request arrived.
    #[must_use]
    pub fn started_at(&self) -> SystemTime {
        self.inner.started_at
    }

    /// Returns the monotonic timestamp at which the request arrived.
    #[must_use]
    pub fn started(&self) -> Instant {
        self.inner.started
    }

    #[must_use]
    pub fn request(&self) -> &CapturedRequest {
        &self.inner.request
    }

    #[must_use]
    pub fn outcome(&self) -> &Outcome {
        self.inner
            .settlement
            .get()
            .map_or(&IN_FLIGHT, |settlement| &settlement.outcome)
    }

    /// Reports whether the request, response body, or tunnel can still progress.
    #[must_use]
    pub fn is_live(&self) -> bool {
        match self.outcome() {
            Outcome::InFlight => true,
            Outcome::Response(response) => response.body().lifecycle() == BodyLifecycle::Streaming,
            Outcome::Tunnel(tunnel) => tunnel.snapshot().state() == &TunnelState::Open,
            Outcome::InspectionEstablished
            | Outcome::Failed { .. }
            | Outcome::Cancelled
            | Outcome::Incomplete => false,
        }
    }

    /// Returns response-body bytes or tunnel download bytes observed so far.
    #[must_use]
    pub fn received_bytes(&self) -> Option<u64> {
        match self.outcome() {
            Outcome::Response(response) => Some(response.body().observed_bytes()),
            Outcome::Tunnel(tunnel) => Some(tunnel.snapshot().upstream_to_client()),
            Outcome::InFlight
            | Outcome::InspectionEstablished
            | Outcome::Failed { .. }
            | Outcome::Cancelled
            | Outcome::Incomplete => None,
        }
    }

    /// Returns the upstream connection used to forward this exchange, or
    /// `None` when forwarding never reached an upstream connection.
    #[must_use]
    pub fn upstream_connection(&self) -> Option<UpstreamConnection> {
        *lock_recovering(&self.inner.upstream_connection)
    }

    /// Returns the observed durations and phase spans so far.
    #[must_use]
    pub fn timing(&self) -> Timing {
        let since_start = |instant: Instant| instant.saturating_duration_since(self.inner.started);
        let settlement = self.inner.settlement.get();
        let settled = settlement.map(|settlement| settlement.after);
        let send_started = self.inner.send_started.get().copied().map(since_start);
        let request_ended = self
            .request()
            .body()
            .and_then(BodyPreview::ended_at)
            .map(since_start);
        let response = settlement.and_then(|settlement| match &settlement.outcome {
            Outcome::Response(response) => Some(response),
            _ => None,
        });
        let headers = response.and(settled);
        let response_ended = response
            .and_then(|response| response.body().ended_at())
            .map(since_start);
        let reused = self
            .upstream_connection()
            .is_some_and(UpstreamConnection::is_reused);
        Timing {
            connect: if reused {
                None
            } else {
                self.inner
                    .connect
                    .get()
                    .copied()
                    .map(|duration| PhaseSpan::new(Duration::ZERO, duration))
            },
            send: span_between(send_started, request_ended),
            wait: span_between(request_ended, headers),
            receive: span_between(headers, response_ended),
            settled,
            complete: if response.is_some() {
                response_ended
            } else {
                settled
            },
        }
    }

    /// Returns the time until the exchange settled, or the time since it
    /// started while it is in flight.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.timing()
            .settled()
            .unwrap_or_else(|| self.inner.started.elapsed())
    }

    /// Waits until the exchange leaves [`Outcome::InFlight`] and returns the result.
    pub async fn settled(&self) -> &Outcome {
        poll_fn(|cx| {
            let outcome = || {
                self.inner
                    .settlement
                    .get()
                    .map(|settlement| &settlement.outcome)
            };
            if let Some(outcome) = outcome() {
                return Poll::Ready(outcome);
            }
            self.inner.settled_wakers.register(cx.waker());
            // Checked again so a settlement between the check and the
            // registration still completes this wait.
            outcome().map_or(Poll::Pending, Poll::Ready)
        })
        .await
    }
}

/// Returns the observed span `[start, end]` when both were observed and `end`
/// is not earlier than `start`.
fn span_between(start: Option<Duration>, end: Option<Duration>) -> Option<PhaseSpan> {
    let start = start?;
    let duration = end?.checked_sub(start)?;
    Some(PhaseSpan::new(start, duration))
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::atomic::AtomicBool;
    use std::task::{Context, Wake, Waker};

    use super::*;
    use crate::exchange::{BodyPreviewWriter, TunnelWriter};

    fn start() -> ExchangeWriter {
        ExchangeWriter::start(
            RecordId::new(1),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(
                Method::GET,
                "http://example.invalid/".to_owned(),
                HeaderPreview::default(),
                Vec::new(),
            ),
        )
    }

    #[test]
    fn response_media_type_uses_first_retained_value_without_parameters() {
        let response = |headers| {
            CapturedResponse::new(
                StatusCode::OK,
                Version::HTTP_11,
                "OK".into(),
                headers,
                BodyPreviewWriter::new(16).preview(),
            )
        };
        assert_eq!(response(HeaderPreview::default()).media_type(), None);
        let headers = HeaderPreview::new(
            vec![
                crate::exchange::CapturedHeader::new(
                    "Content-Type".into(),
                    " application/problem+json ; charset=utf-8".into(),
                ),
                crate::exchange::CapturedHeader::new("content-type".into(), "text/plain".into()),
            ],
            false,
        );
        assert_eq!(
            response(headers).media_type().map(MediaType::essence),
            Some("application/problem+json")
        );
    }

    #[test]
    fn request_media_type_is_parsed_from_captured_headers() {
        let request = |value: &str| {
            let headers = HeaderPreview::new(
                vec![crate::exchange::CapturedHeader::new(
                    "content-type".into(),
                    value.into(),
                )],
                false,
            );
            CapturedRequest::new(
                Method::POST,
                "http://example.invalid/".into(),
                headers,
                Vec::new(),
            )
        };
        assert_eq!(
            request("Application/X-WWW-Form-Urlencoded; charset=utf-8")
                .media_type()
                .map(MediaType::essence),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(request("not a media type").media_type(), None);
    }

    #[test]
    fn classifies_outcomes_and_live_transfer_bytes() {
        let pending = start();
        let pending = pending.exchange();
        assert!(pending.is_live());
        assert_eq!(pending.received_bytes(), None);
        assert!(!pending.outcome().is_failure());
        for outcome in [
            Outcome::Cancelled,
            Outcome::Incomplete,
            Outcome::Failed {
                stage: FailureStage::Connect,
                message: String::new(),
            },
            Outcome::InspectionEstablished,
        ] {
            let writer = start();
            let record = writer.exchange();
            let failure = !matches!(outcome, Outcome::InspectionEstablished);
            writer.settle(outcome);
            assert_eq!(record.outcome().is_failure(), failure);
            assert!(!record.is_live());
            assert_eq!(record.received_bytes(), None);
        }
        let body = BodyPreviewWriter::new(8);
        body.observe(b"abc");
        let writer = start();
        let record = writer.exchange();
        writer.settle(response(&body.preview()));
        assert!(record.is_live());
        assert!(!record.outcome().is_failure());
        assert_eq!(record.received_bytes(), Some(3));
        body.complete();
        assert!(!record.is_live());
        for state in 0..4 {
            let tunnel = TunnelWriter::new();
            tunnel.add_upstream_to_client(7);
            let writer = start();
            let record = writer.exchange();
            writer.settle(Outcome::Tunnel(tunnel.preview()));
            match state {
                1 => tunnel.close(),
                2 => tunnel.fail(FailureStage::Tunnel, String::new()),
                3 => tunnel.abandon(),
                _ => (),
            }
            assert_eq!(record.is_live(), state == 0);
            assert_eq!(record.outcome().is_failure(), state >= 2);
            assert_eq!(record.received_bytes(), Some(7));
        }
    }

    #[test]
    fn starts_in_flight_with_unobserved_timing() {
        let writer = start();
        let exchange = writer.exchange();
        assert!(exchange.outcome().is_in_flight());
        assert_eq!(exchange.timing(), Timing::default());
        assert!(exchange.request().body().is_none());
    }

    #[test]
    fn keeps_the_first_settlement() {
        let writer = start();
        let exchange = writer.exchange();
        assert!(writer.settle(Outcome::Cancelled));
        assert!(!writer.settle(Outcome::Incomplete));
        assert!(matches!(exchange.outcome(), Outcome::Cancelled));
        assert!(exchange.timing().settled().is_some());
        assert_eq!(exchange.elapsed(), exchange.elapsed());
    }

    #[test]
    fn clones_share_progress() {
        let writer = start();
        let view = writer.exchange().clone();
        writer.record_connect(Duration::from_millis(3));
        writer.attach_request_body(BodyPreviewWriter::new(8).preview());
        assert!(writer.settle(Outcome::InspectionEstablished));
        assert_eq!(view.timing().connect(), Some(Duration::from_millis(3)));
        assert!(view.request().body().is_some());
        assert!(matches!(view.outcome(), Outcome::InspectionEstablished));
    }

    fn response(body: &BodyPreview) -> Outcome {
        Outcome::Response(CapturedResponse::new(
            StatusCode::OK,
            Version::HTTP_11,
            "OK".to_owned(),
            HeaderPreview::default(),
            body.clone(),
        ))
    }

    #[test]
    fn splits_a_response_into_consecutive_phases() {
        let writer = start();
        let exchange = writer.exchange();
        let request_body = BodyPreviewWriter::new(8);
        let response_body = BodyPreviewWriter::new(8);
        writer.record_connect(Duration::from_millis(1));
        writer.attach_request_body(request_body.preview());
        assert_eq!(exchange.timing().send(), None);
        assert_eq!(exchange.timing().send_span(), None);
        request_body.complete();
        assert!(writer.settle(response(&response_body.preview())));

        let streaming = exchange.timing();
        assert_eq!(
            streaming.connect_span(),
            Some(PhaseSpan::new(Duration::ZERO, Duration::from_millis(1)))
        );
        assert!(streaming.send().is_some());
        assert!(streaming.wait().is_some());
        assert_eq!(streaming.receive(), None);
        assert_eq!(streaming.receive_span(), None);
        assert_eq!(streaming.complete(), None);

        response_body.interrupt();
        let ended = exchange.timing();
        let send = ended.send_span();
        let wait = ended.wait_span();
        let receive = ended.receive_span();
        assert_eq!(send.map(|span| span.duration()), ended.send());
        assert_eq!(wait.map(|span| span.duration()), ended.wait());
        assert_eq!(receive.map(|span| span.duration()), ended.receive());
        assert_eq!(send.map(|span| span.end()), wait.map(|span| span.start()));
        assert_eq!(wait.map(|span| span.end()), ended.settled());
        assert_eq!(receive.map(|span| span.start()), ended.settled());
        assert_eq!(receive.map(|span| span.end()), ended.complete());
        let receive = ended.receive().unwrap_or_default();
        let settled = ended.settled().unwrap_or_default();
        assert_eq!(ended.complete(), Some(settled + receive));
    }

    #[test]
    fn leaves_wait_unobserved_when_the_response_beats_the_request_body() {
        let writer = start();
        let exchange = writer.exchange();
        let request_body = BodyPreviewWriter::new(8);
        writer.attach_request_body(request_body.preview());
        assert!(writer.settle(response(&BodyPreviewWriter::new(8).preview())));
        request_body.cancel();
        let timing = exchange.timing();
        assert!(timing.send().is_some());
        assert!(timing.send_span().is_some());
        assert_eq!(timing.wait(), None);
        assert_eq!(timing.wait_span(), None);
    }

    #[test]
    fn completes_other_outcomes_when_they_settle() {
        let writer = start();
        let exchange = writer.exchange();
        assert!(writer.settle(Outcome::Failed {
            stage: FailureStage::Connect,
            message: "refused".to_owned(),
        }));
        let timing = exchange.timing();
        assert_eq!(timing.complete(), timing.settled());
        assert_eq!(
            (
                timing.connect_span(),
                timing.send_span(),
                timing.wait_span(),
                timing.receive_span()
            ),
            (None, None, None, None)
        );
    }

    struct WokenFlag(AtomicBool);

    impl Wake for WokenFlag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn wakes_a_waiter_when_settled() {
        let writer = start();
        let exchange = writer.exchange();
        let woken = Arc::new(WokenFlag(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&woken));
        let mut cx = Context::from_waker(&waker);
        let mut waiting = pin!(exchange.settled());
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        assert!(!woken.0.load(Ordering::SeqCst));
        assert!(writer.settle(Outcome::Incomplete));
        assert!(woken.0.load(Ordering::SeqCst));
        assert!(matches!(
            waiting.as_mut().poll(&mut cx),
            Poll::Ready(Outcome::Incomplete)
        ));
    }

    #[test]
    fn returns_at_once_when_already_settled() {
        let writer = start();
        let exchange = writer.exchange();
        assert!(writer.settle(Outcome::Cancelled));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            pin!(exchange.settled()).poll(&mut cx),
            Poll::Ready(Outcome::Cancelled)
        ));
    }

    #[test]
    fn records_upstream_connection_and_hides_connect_timing_when_reused() {
        let fresh_writer = start();
        let conn_id = ConnectionId::new(11);
        fresh_writer.record_connect(Duration::from_millis(4));
        fresh_writer.record_upstream_connection(UpstreamConnection::fresh(conn_id));
        assert_eq!(
            fresh_writer.exchange().upstream_connection(),
            Some(UpstreamConnection::fresh(conn_id))
        );
        assert_eq!(
            fresh_writer.exchange().timing().connect(),
            Some(Duration::from_millis(4))
        );

        let reused_writer = start();
        reused_writer.record_upstream_connection(UpstreamConnection::reused(conn_id));
        assert_eq!(
            reused_writer.exchange().upstream_connection(),
            Some(UpstreamConnection::reused(conn_id))
        );
        assert_eq!(reused_writer.exchange().timing().connect(), None);
        assert_eq!(reused_writer.exchange().timing().connect_span(), None);

        // Retrying a stale reused connection updates the connection to fresh.
        let retry_id = ConnectionId::new(12);
        reused_writer.record_connect(Duration::from_millis(6));
        reused_writer.record_upstream_connection(UpstreamConnection::fresh(retry_id));
        assert_eq!(
            reused_writer.exchange().upstream_connection(),
            Some(UpstreamConnection::fresh(retry_id))
        );
        assert_eq!(
            reused_writer.exchange().timing().connect(),
            Some(Duration::from_millis(6))
        );
    }
}
