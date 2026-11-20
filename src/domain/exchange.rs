//! One HTTP exchange, published when the request starts and settled once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use http::{Method, StatusCode, Version};
use tokio::sync::Notify;

use super::{BodyPreview, FailureStage, HeaderPreview, RecordId, TunnelPreview};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static IN_FLIGHT: Outcome = Outcome::InFlight;

/// Identity of one proxy run. Records from different runs never share it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(u64);

impl SessionId {
    pub(crate) fn next() -> Self {
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
    body: OnceLock<BodyPreview>,
}

impl CapturedRequest {
    pub(crate) const fn new(method: Method, target: String, headers: HeaderPreview) -> Self {
        Self {
            method,
            target,
            headers,
            body: OnceLock::new(),
        }
    }

    /// Returns the HTTP method.
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

    /// Returns the body preview once forwarding began, or `None` when the
    /// proxy never read the request body.
    #[must_use]
    pub fn body(&self) -> Option<&BodyPreview> {
        self.body.get()
    }
}

/// Response head received from the upstream server and its streamed body.
#[derive(Clone, Debug)]
pub struct CapturedResponse {
    status: StatusCode,
    version: Version,
    reason: String,
    headers: HeaderPreview,
    body: BodyPreview,
}

impl CapturedResponse {
    pub(crate) const fn new(
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
            headers,
            body,
        }
    }

    /// Returns the status code.
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
    /// Reports whether the exchange has not settled yet.
    #[must_use]
    pub const fn is_in_flight(&self) -> bool {
        matches!(self, Self::InFlight)
    }
}

/// Observed durations of an exchange. A phase the proxy did not observe, or
/// that has not finished yet, is `None`, never zero.
///
/// For a response the phases follow each other: `connect`, then `send`
/// until the client's request body was fully forwarded, then `wait` until
/// response headers arrived, then `receive` until the response body ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    connect: Option<Duration>,
    send: Option<Duration>,
    wait: Option<Duration>,
    receive: Option<Duration>,
    settled: Option<Duration>,
    complete: Option<Duration>,
}

impl Timing {
    /// Returns the time spent resolving and connecting to the upstream
    /// server, including the TLS handshake for inspected HTTPS.
    #[must_use]
    pub const fn connect(&self) -> Option<Duration> {
        self.connect
    }

    /// Returns the time from handing the request to the upstream connection
    /// until the request body ended.
    #[must_use]
    pub const fn send(&self) -> Option<Duration> {
        self.send
    }

    /// Returns the time from the end of the request body until response
    /// headers arrived (time to first byte). `None` when the server answered
    /// before the request body ended.
    #[must_use]
    pub const fn wait(&self) -> Option<Duration> {
        self.wait
    }

    /// Returns the time from response headers until the response body ended,
    /// whether it completed, was interrupted, or was cancelled.
    #[must_use]
    pub const fn receive(&self) -> Option<Duration> {
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
    send_started: OnceLock<Instant>,
    settlement: OnceLock<Settlement>,
    settled_signal: Notify,
}

/// One HTTP exchange observed by the proxy.
///
/// The engine publishes it when the request arrives and fills in the rest as
/// forwarding progresses, so a clone always shows the current state.
#[derive(Clone, Debug)]
pub struct Exchange {
    inner: Arc<ExchangeInner>,
}

impl Exchange {
    pub(crate) fn new(
        id: RecordId,
        session: SessionId,
        source: CaptureSource,
        request: CapturedRequest,
    ) -> Self {
        Self {
            inner: Arc::new(ExchangeInner {
                id,
                session,
                source,
                replay_of: OnceLock::new(),
                started_at: SystemTime::now(),
                started: Instant::now(),
                request,
                connect: OnceLock::new(),
                send_started: OnceLock::new(),
                settlement: OnceLock::new(),
                settled_signal: Notify::new(),
            }),
        }
    }

    /// Returns the original record when this exchange was manually replayed.
    #[must_use]
    pub fn replay_of(&self) -> Option<RecordId> {
        self.inner.replay_of.get().copied()
    }

    pub(crate) fn set_replay_of(&self, original: RecordId) {
        let _already_set = self.inner.replay_of.set(original).is_err();
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

    /// Returns the client's request.
    #[must_use]
    pub fn request(&self) -> &CapturedRequest {
        &self.inner.request
    }

    /// Returns the current outcome.
    #[must_use]
    pub fn outcome(&self) -> &Outcome {
        self.inner
            .settlement
            .get()
            .map_or(&IN_FLIGHT, |settlement| &settlement.outcome)
    }

    /// Returns the observed durations so far.
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
        Timing {
            connect: self.inner.connect.get().copied(),
            send: between(send_started, request_ended),
            wait: between(request_ended, headers),
            receive: between(headers, response_ended),
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
        loop {
            // Created before the check so a settlement in between still wakes it.
            let notified = self.inner.settled_signal.notified();
            if let Some(settlement) = self.inner.settlement.get() {
                return &settlement.outcome;
            }
            notified.await;
        }
    }

    /// Attaches the request-body preview as the request is handed to the
    /// upstream connection, which starts the `send` phase.
    pub(crate) fn attach_request_body(&self, body: BodyPreview) {
        let _already_started = self.inner.send_started.set(Instant::now()).is_err();
        let _already_attached = self.inner.request.body.set(body).is_err();
    }

    pub(crate) fn record_connect(&self, duration: Duration) {
        let _already_recorded = self.inner.connect.set(duration).is_err();
    }

    /// Moves the exchange out of [`Outcome::InFlight`]. Returns `false` and
    /// keeps the first outcome when the exchange had already settled.
    pub(crate) fn settle(&self, outcome: Outcome) -> bool {
        let settlement = Settlement {
            outcome,
            after: self.inner.started.elapsed(),
        };
        let settled = self.inner.settlement.set(settlement).is_ok();
        if settled {
            self.inner.settled_signal.notify_waiters();
        }
        settled
    }
}

/// Returns `end - start` when both were observed and `end` is not earlier.
fn between(start: Option<Duration>, end: Option<Duration>) -> Option<Duration> {
    end?.checked_sub(start?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange() -> Exchange {
        Exchange::new(
            RecordId::new(1),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(
                Method::GET,
                "http://example.invalid/".to_owned(),
                HeaderPreview::default(),
            ),
        )
    }

    #[test]
    fn starts_in_flight_with_unobserved_timing() {
        let exchange = exchange();
        assert!(exchange.outcome().is_in_flight());
        assert_eq!(exchange.timing(), Timing::default());
        assert!(exchange.request().body().is_none());
    }

    #[test]
    fn keeps_the_first_settlement() {
        let exchange = exchange();
        assert!(exchange.settle(Outcome::Cancelled));
        assert!(!exchange.settle(Outcome::Incomplete));
        assert!(matches!(exchange.outcome(), Outcome::Cancelled));
        assert!(exchange.timing().settled().is_some());
        assert_eq!(exchange.elapsed(), exchange.elapsed());
    }

    #[test]
    fn clones_share_progress() {
        let exchange = exchange();
        let view = exchange.clone();
        exchange.record_connect(Duration::from_millis(3));
        exchange.attach_request_body(BodyPreview::new(8));
        assert!(exchange.settle(Outcome::InspectionEstablished));
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
        let exchange = exchange();
        let request_body = BodyPreview::new(8);
        let response_body = BodyPreview::new(8);
        exchange.record_connect(Duration::from_millis(1));
        exchange.attach_request_body(request_body.clone());
        assert_eq!(exchange.timing().send(), None);
        request_body.complete();
        assert!(exchange.settle(response(&response_body)));

        let streaming = exchange.timing();
        assert!(streaming.send().is_some());
        assert!(streaming.wait().is_some());
        assert_eq!(streaming.receive(), None);
        assert_eq!(streaming.complete(), None);

        response_body.interrupt();
        let ended = exchange.timing();
        let receive = ended.receive().unwrap_or_default();
        let settled = ended.settled().unwrap_or_default();
        assert_eq!(ended.complete(), Some(settled + receive));
    }

    #[test]
    fn leaves_wait_unobserved_when_the_response_beats_the_request_body() {
        let exchange = exchange();
        let request_body = BodyPreview::new(8);
        exchange.attach_request_body(request_body.clone());
        assert!(exchange.settle(response(&BodyPreview::new(8))));
        request_body.cancel();
        let timing = exchange.timing();
        assert!(timing.send().is_some());
        assert_eq!(timing.wait(), None);
    }

    #[test]
    fn completes_other_outcomes_when_they_settle() {
        let exchange = exchange();
        assert!(exchange.settle(Outcome::Failed {
            stage: FailureStage::Connect,
            message: "refused".to_owned(),
        }));
        let timing = exchange.timing();
        assert_eq!(timing.complete(), timing.settled());
        assert_eq!(
            (timing.send(), timing.wait(), timing.receive()),
            (None, None, None)
        );
    }

    #[tokio::test]
    async fn wakes_a_waiter_when_settled() -> Result<(), tokio::time::error::Elapsed> {
        let exchange = exchange();
        let waiter = exchange.clone();
        let waiting =
            tokio::spawn(async move { matches!(waiter.settled().await, Outcome::Incomplete) });
        tokio::task::yield_now().await;
        assert!(exchange.settle(Outcome::Incomplete));
        let woke = tokio::time::timeout(Duration::from_secs(1), waiting).await?;
        assert!(matches!(woke, Ok(true)));
        Ok(())
    }
}
