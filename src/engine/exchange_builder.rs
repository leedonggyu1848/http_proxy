//! Captured exchanges for tests, built without running a proxy.
//!
//! Compiled only with the `test-support` feature, which release builds leave
//! off. The builder records each state through the same exchange, body, and
//! tunnel writers and the same header, target, and preview rules as the
//! capture path, so it cannot produce a record the proxy would never
//! produce: sensitive header values are redacted, long text is bounded, and
//! response bodies keep the same preview limits.

use std::sync::OnceLock;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, Version};
use hyper::ext::ReasonPhrase;

use crate::engine::capture::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, bounded_text, captured_request, captured_response,
    response_body_preview,
};
use crate::exchange::{
    BodyLifecycle, BodyPreviewWriter, CaptureSource, Exchange, ExchangeWriter, FailureStage,
    Outcome, RecordId, SessionId, TunnelState, TunnelWriter,
};

/// Request head and connection timing shared by every kind of fixture.
#[derive(Clone, Debug)]
struct RequestHead {
    source: CaptureSource,
    method: Method,
    target: String,
    headers: HeaderMap,
    session: Option<SessionId>,
    connect: Option<Duration>,
}

impl RequestHead {
    fn new(source: CaptureSource, method: Method, target: String) -> Self {
        Self {
            source,
            method,
            target,
            headers: HeaderMap::new(),
            session: None,
            connect: None,
        }
    }

    /// Publishes the exchange as the proxy does when the request arrives.
    fn start(&self, replay_of: Option<RecordId>) -> ExchangeWriter {
        let writer = ExchangeWriter::start(
            RecordId::next(),
            self.session.unwrap_or_else(fixture_session),
            self.source,
            captured_request(&self.method, &self.target, &self.headers),
        );
        if let Some(original) = replay_of {
            writer.set_replay_of(original);
        }
        writer
    }

    /// Records the upstream connection. The proxy does this only after the
    /// connection succeeded, right before it forwards a request or opens a
    /// tunnel, so no other state carries a connect time.
    fn connected(&self, writer: &ExchangeWriter) {
        writer.record_connect(self.connect.unwrap_or_default());
    }
}

/// Session shared by fixtures that do not choose one, as if one proxy run
/// captured them all.
fn fixture_session() -> SessionId {
    static SESSION: OnceLock<SessionId> = OnceLock::new();
    *SESSION.get_or_init(SessionId::next)
}

/// Ends a body preview the way the forwarded stream ended.
fn end_body(body: &BodyPreviewWriter, end: BodyLifecycle) {
    match end {
        BodyLifecycle::Streaming => {}
        BodyLifecycle::Complete => body.complete(),
        BodyLifecycle::Interrupted => body.interrupt(),
        BodyLifecycle::Cancelled => body.cancel(),
    }
}

/// A `CONNECT` request given to an HTTP fixture constructor.
///
/// The proxy never captures `CONNECT` as a plain or decrypted HTTP request:
/// it routes `CONNECT` to the tunnel path and rejects one inside an
/// inspected session. Build it with [`ExchangeBuilder::connect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("CONNECT requests are built with ExchangeBuilder::connect")]
pub struct ConnectMethodError;

/// Builds an HTTP exchange, from a plain proxy request or one decrypted
/// inside an inspected `CONNECT` session, in any state the proxy records.
///
/// Use [`ExchangeBuilder::connect`] for a `CONNECT` request itself; the
/// HTTP constructors refuse that method, as the proxy always routes it to
/// its own path.
///
/// ```
/// use http_proxy::{BodyLifecycle, ExchangeBuilder, ResponseFixture};
///
/// # fn main() -> Result<(), http_proxy::ConnectMethodError> {
/// let exchange = ExchangeBuilder::http(http::Method::GET, "http://example.test/")?
///     .respond(ResponseFixture::new(http::StatusCode::OK).body("hello", BodyLifecycle::Complete));
/// assert!(!exchange.is_live());
/// assert!(ExchangeBuilder::http(http::Method::CONNECT, "example.test:443").is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[must_use]
pub struct ExchangeBuilder {
    head: RequestHead,
    replay_of: Option<RecordId>,
    request_body: Option<(Vec<u8>, BodyLifecycle)>,
}

impl ExchangeBuilder {
    /// Starts a plain `http://` request forwarded by the proxy. `target` is
    /// the absolute request target the client sent.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectMethodError`] for `CONNECT`, which the proxy never
    /// forwards as a plain request; use [`ExchangeBuilder::connect`].
    pub fn http(method: Method, target: impl Into<String>) -> Result<Self, ConnectMethodError> {
        Self::with_source(CaptureSource::HttpProxy, method, target.into())
    }

    /// Starts a request decrypted inside an inspected `CONNECT` session.
    /// `target` is the absolute `https://` URL the proxy reconstructed.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectMethodError`] for `CONNECT`, which the proxy rejects
    /// inside an inspected session before capturing it.
    pub fn inspected_https(
        method: Method,
        target: impl Into<String>,
    ) -> Result<Self, ConnectMethodError> {
        Self::with_source(CaptureSource::InspectedHttps, method, target.into())
    }

    /// Starts a `CONNECT` request for `authority`, such as `example.test:443`.
    pub fn connect(authority: impl Into<String>) -> ConnectExchangeBuilder {
        ConnectExchangeBuilder {
            head: RequestHead::new(CaptureSource::Connect, Method::CONNECT, authority.into()),
        }
    }

    fn with_source(
        source: CaptureSource,
        method: Method,
        target: String,
    ) -> Result<Self, ConnectMethodError> {
        if method == Method::CONNECT {
            return Err(ConnectMethodError);
        }
        Ok(Self {
            head: RequestHead::new(source, method, target),
            replay_of: None,
            request_body: None,
        })
    }

    /// Assigns the exchange to `session` instead of the shared fixture session.
    pub const fn session(mut self, session: SessionId) -> Self {
        self.head.session = Some(session);
        self
    }

    /// Appends one request header as the client sent it.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.head.headers.append(name, value);
        self
    }

    /// Sets how long connecting to the upstream server took. As in the
    /// proxy, it is recorded only once the request is forwarded (a response,
    /// a `Response` or `ResponseTimeout` failure, or any state with a request
    /// body); forwarded fixtures without one record zero, and every other
    /// state, such as a `Connect` failure, has no connect time.
    pub const fn connect_time(mut self, duration: Duration) -> Self {
        self.head.connect = Some(duration);
        self
    }

    /// Sends `body` as the request body, ending as `end`. Setting a body
    /// means the proxy began forwarding the request, so the body is attached
    /// for every state except failures before forwarding (`Request`,
    /// `Connect`, `ConnectTimeout`, and `UpstreamTls`).
    pub fn request_body(mut self, body: impl Into<Vec<u8>>, end: BodyLifecycle) -> Self {
        self.request_body = Some((body.into(), end));
        self
    }

    /// Marks the exchange as a manual replay of `original`.
    pub const fn replay_of(mut self, original: RecordId) -> Self {
        self.replay_of = Some(original);
        self
    }

    /// Returns an exchange still waiting for response headers.
    #[must_use]
    pub fn in_flight(self) -> Exchange {
        self.start().exchange().clone()
    }

    /// Returns an exchange whose response headers arrived, with the response
    /// body observed as `response` describes.
    #[must_use]
    pub fn respond(self, response: ResponseFixture) -> Exchange {
        let writer = self.forward();
        let (head, bytes, end) = response.into_parts();
        let body = response_body_preview(head.headers());
        writer.settle(Outcome::Response(captured_response(&head, body.preview())));
        body.observe(&bytes);
        end_body(&body, end);
        writer.exchange().clone()
    }

    /// Returns an exchange that failed at `stage` before response headers
    /// arrived.
    #[must_use]
    pub fn fail(self, stage: FailureStage, message: &str) -> Exchange {
        let forwarded = matches!(
            stage,
            FailureStage::Response | FailureStage::ResponseTimeout
        );
        let writer = if forwarded {
            self.forward()
        } else {
            self.head.start(self.replay_of)
        };
        writer.settle(Outcome::Failed {
            stage,
            message: bounded_text(message, ERROR_BYTES),
        });
        writer.exchange().clone()
    }

    /// Returns an exchange whose client went away before it settled.
    #[must_use]
    pub fn cancel(self) -> Exchange {
        self.settle(Outcome::Cancelled)
    }

    /// Returns an exchange cut short because the proxy stopped.
    #[must_use]
    pub fn stop(self) -> Exchange {
        self.settle(Outcome::Incomplete)
    }

    fn settle(self, outcome: Outcome) -> Exchange {
        let writer = self.start();
        writer.settle(outcome);
        writer.exchange().clone()
    }

    /// Publishes the exchange and, when a request body was set, forwards it.
    fn start(self) -> ExchangeWriter {
        if self.request_body.is_some() {
            self.forward()
        } else {
            self.head.start(self.replay_of)
        }
    }

    /// Publishes the exchange and forwards the request as the proxy does:
    /// connect, then attach the request body and stream it. A request with
    /// no body set is forwarded with an empty one.
    fn forward(self) -> ExchangeWriter {
        let writer = self.head.start(self.replay_of);
        self.head.connected(&writer);
        let (bytes, end) = self
            .request_body
            .unwrap_or((Vec::new(), BodyLifecycle::Complete));
        let body = BodyPreviewWriter::new(BODY_PREVIEW_BYTES);
        writer.attach_request_body(body.preview());
        body.observe(&bytes);
        end_body(&body, end);
        writer
    }
}

/// Builds a `CONNECT` exchange in any state the proxy records.
#[derive(Clone, Debug)]
#[must_use]
pub struct ConnectExchangeBuilder {
    head: RequestHead,
}

impl ConnectExchangeBuilder {
    /// Assigns the exchange to `session` instead of the shared fixture session.
    pub const fn session(mut self, session: SessionId) -> Self {
        self.head.session = Some(session);
        self
    }

    /// Appends one request header as the client sent it.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.head.headers.append(name, value);
        self
    }

    /// Sets how long connecting to the upstream server took. Only a relayed
    /// tunnel records it, as the proxy does; without one it records zero.
    pub const fn connect_time(mut self, duration: Duration) -> Self {
        self.head.connect = Some(duration);
        self
    }

    /// Returns a `CONNECT` request the proxy has not answered yet.
    #[must_use]
    pub fn in_flight(self) -> Exchange {
        self.head.start(None).exchange().clone()
    }

    /// Returns a relayed tunnel that carried the given byte counts and is
    /// now in `state`; [`TunnelState::Open`] leaves it relaying.
    #[must_use]
    pub fn tunnel(
        self,
        client_to_upstream: usize,
        upstream_to_client: usize,
        state: TunnelState,
    ) -> Exchange {
        let writer = self.head.start(None);
        self.head.connected(&writer);
        let tunnel = TunnelWriter::new();
        writer.settle(Outcome::Tunnel(tunnel.preview()));
        tunnel.add_client_to_upstream(client_to_upstream);
        tunnel.add_upstream_to_client(upstream_to_client);
        match state {
            TunnelState::Open => {}
            TunnelState::Closed => tunnel.close(),
            TunnelState::Failed { stage, message } => {
                tunnel.fail(stage, bounded_text(&message, ERROR_BYTES));
            }
            TunnelState::Incomplete => tunnel.abandon(),
        }
        writer.exchange().clone()
    }

    /// Returns a `CONNECT` request whose TLS inspection session started.
    #[must_use]
    pub fn inspection_established(self) -> Exchange {
        self.settle(Outcome::InspectionEstablished)
    }

    /// Returns a `CONNECT` request that failed at `stage`.
    #[must_use]
    pub fn fail(self, stage: FailureStage, message: &str) -> Exchange {
        self.settle(Outcome::Failed {
            stage,
            message: bounded_text(message, ERROR_BYTES),
        })
    }

    /// Returns a `CONNECT` request whose client went away before it settled.
    #[must_use]
    pub fn cancel(self) -> Exchange {
        self.settle(Outcome::Cancelled)
    }

    /// Returns a `CONNECT` request cut short because the proxy stopped.
    #[must_use]
    pub fn stop(self) -> Exchange {
        self.settle(Outcome::Incomplete)
    }

    fn settle(self, outcome: Outcome) -> Exchange {
        let writer = self.head.start(None);
        writer.settle(outcome);
        writer.exchange().clone()
    }
}

/// Response head and body that an upstream server sends to a fixture.
#[derive(Clone, Debug)]
#[must_use]
pub struct ResponseFixture {
    status: StatusCode,
    version: Version,
    reason: Option<ReasonPhrase>,
    headers: HeaderMap,
    body: Vec<u8>,
    end: BodyLifecycle,
}

impl ResponseFixture {
    /// Starts an HTTP/1.1 response with the canonical reason phrase, no
    /// headers, and an empty body that already ended.
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            version: Version::HTTP_11,
            reason: None,
            headers: HeaderMap::new(),
            body: Vec::new(),
            end: BodyLifecycle::Complete,
        }
    }

    /// Sets the HTTP version the server answered with.
    pub const fn version(mut self, version: Version) -> Self {
        self.version = version;
        self
    }

    /// Sets a reason phrase other than the canonical one.
    pub fn reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(ReasonPhrase::from_static(reason.as_bytes()));
        self
    }

    /// Appends one response header as the server sent it.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Sends `body` as the response body, ending as `end`;
    /// [`BodyLifecycle::Streaming`] leaves it in progress.
    pub fn body(mut self, body: impl Into<Vec<u8>>, end: BodyLifecycle) -> Self {
        self.body = body.into();
        self.end = end;
        self
    }

    /// Splits the fixture into the response head as the upstream connection
    /// delivers it and the body bytes with how their stream ends.
    fn into_parts(self) -> (Response<()>, Vec<u8>, BodyLifecycle) {
        let mut head = Response::new(());
        *head.status_mut() = self.status;
        *head.version_mut() = self.version;
        *head.headers_mut() = self.headers;
        if let Some(reason) = self.reason {
            let _previous = head.extensions_mut().insert(reason);
        }
        (head, self.body, self.end)
    }
}

#[cfg(test)]
mod tests {
    use http::header::{AUTHORIZATION, CONTENT_TYPE};

    use super::*;
    use crate::exchange::{BodyPreview, BodyRetention, CapturedResponse, Timing};

    type TestResult = Result<(), ConnectMethodError>;

    fn response_of(exchange: &Exchange) -> Option<&CapturedResponse> {
        match exchange.outcome() {
            Outcome::Response(response) => Some(response),
            _ => None,
        }
    }

    #[test]
    fn redacts_and_bounds_request_heads_like_the_capture_path() -> TestResult {
        let long_target = format!("http://example.test/{}", "a".repeat(8 * 1024));
        let exchange = ExchangeBuilder::http(Method::GET, long_target)?
            .header(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer secret-token"),
            )
            .in_flight();
        let request = exchange.request();
        assert_eq!(
            request.headers().get("authorization").next(),
            Some("[REDACTED]")
        );
        assert_eq!(
            request.auth_schemes(),
            [crate::exchange::AuthScheme::Bearer]
        );
        assert!(request.target().ends_with('…'));
        assert!(exchange.outcome().is_in_flight());
        assert!(request.body().is_none());
        assert_eq!(exchange.timing(), Timing::default());
        Ok(())
    }

    #[test]
    fn forwards_an_empty_request_body_before_a_response() -> TestResult {
        let exchange = ExchangeBuilder::http(Method::GET, "http://example.test/")?
            .respond(ResponseFixture::new(StatusCode::NO_CONTENT));
        let body = exchange.request().body().and_then(BodyPreview::snapshot);
        assert_eq!(
            body.map(|body| body.lifecycle()),
            Some(BodyLifecycle::Complete)
        );
        assert_eq!(exchange.timing().connect(), Some(Duration::ZERO));
        assert_eq!(
            response_of(&exchange).map(CapturedResponse::reason),
            Some("No Content")
        );
        assert!(!exchange.is_live());
        Ok(())
    }

    #[test]
    fn keeps_response_bodies_streaming_or_truncated() -> TestResult {
        let streaming = ExchangeBuilder::http(Method::GET, "http://example.test/")?.respond(
            ResponseFixture::new(StatusCode::OK).body("partial", BodyLifecycle::Streaming),
        );
        assert!(streaming.is_live());
        assert_eq!(streaming.received_bytes(), Some(7));

        let large = vec![b'x'; BODY_PREVIEW_BYTES + 1];
        let truncated = ExchangeBuilder::http(Method::GET, "http://example.test/")?.respond(
            ResponseFixture::new(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
                .body(large, BodyLifecycle::Complete),
        );
        let snapshot = response_of(&truncated).and_then(|response| response.body().snapshot());
        assert_eq!(
            snapshot.map(|body| body.retention()),
            Some(BodyRetention::Truncated)
        );
        Ok(())
    }

    #[test]
    fn attaches_the_request_body_only_once_forwarding_began() -> TestResult {
        let before = ExchangeBuilder::http(Method::POST, "http://example.test/")?
            .request_body("a=1", BodyLifecycle::Complete)
            .fail(FailureStage::Connect, "refused");
        assert!(before.request().body().is_none());
        assert_eq!(before.timing().connect(), None);

        let after = ExchangeBuilder::http(Method::POST, "http://example.test/")?
            .request_body("a=1", BodyLifecycle::Complete)
            .fail(FailureStage::ResponseTimeout, "no response");
        assert!(after.request().body().is_some());
        assert!(after.outcome().is_failure());
        assert_eq!(after.timing().connect(), Some(Duration::ZERO));
        Ok(())
    }

    #[test]
    fn records_a_connect_time_only_after_connecting() -> TestResult {
        let connect = Duration::from_millis(7);
        for stage in [
            FailureStage::Request,
            FailureStage::Connect,
            FailureStage::ConnectTimeout,
            FailureStage::UpstreamTls,
        ] {
            let failed = ExchangeBuilder::http(Method::GET, "http://example.test/")?
                .connect_time(connect)
                .fail(stage, "failed");
            assert_eq!(failed.timing().connect(), None, "{stage}");
        }
        for unforwarded in [
            ExchangeBuilder::http(Method::GET, "http://example.test/")?
                .connect_time(connect)
                .in_flight(),
            ExchangeBuilder::connect("example.test:443")
                .connect_time(connect)
                .fail(FailureStage::Connect, "refused"),
            ExchangeBuilder::connect("example.test:443")
                .connect_time(connect)
                .inspection_established(),
        ] {
            assert_eq!(unforwarded.timing().connect(), None);
        }
        let responded = ExchangeBuilder::http(Method::GET, "http://example.test/")?
            .connect_time(connect)
            .respond(ResponseFixture::new(StatusCode::OK));
        assert_eq!(responded.timing().connect(), Some(connect));
        let tunnel = ExchangeBuilder::connect("example.test:443")
            .connect_time(connect)
            .tunnel(0, 0, TunnelState::Closed);
        assert_eq!(tunnel.timing().connect(), Some(connect));
        Ok(())
    }

    #[test]
    fn refuses_connect_on_the_http_paths() {
        assert_eq!(
            ExchangeBuilder::http(Method::CONNECT, "example.test:443").err(),
            Some(ConnectMethodError)
        );
        assert_eq!(
            ExchangeBuilder::inspected_https(Method::CONNECT, "example.test:443").err(),
            Some(ConnectMethodError)
        );
    }

    #[test]
    fn records_replays_cancellations_and_stops() -> TestResult {
        let original = ExchangeBuilder::http(Method::GET, "http://example.test/")?.cancel();
        assert!(matches!(original.outcome(), Outcome::Cancelled));
        let replay = ExchangeBuilder::http(Method::GET, "http://example.test/")?
            .replay_of(original.id())
            .stop();
        assert_eq!(replay.replay_of(), Some(original.id()));
        assert!(matches!(replay.outcome(), Outcome::Incomplete));
        assert_ne!(replay.id(), original.id());
        assert_eq!(replay.session(), original.session());
        Ok(())
    }

    #[test]
    fn builds_connect_tunnels_in_every_state() {
        let open = ExchangeBuilder::connect("example.test:443").tunnel(3, 5, TunnelState::Open);
        assert_eq!(open.source(), CaptureSource::Connect);
        assert!(open.is_live());
        assert_eq!(open.received_bytes(), Some(5));

        let failed = ExchangeBuilder::connect("example.test:443").tunnel(
            0,
            0,
            TunnelState::Failed {
                stage: FailureStage::Tunnel,
                message: "reset".to_owned(),
            },
        );
        assert!(failed.outcome().is_failure());
        assert!(!failed.is_live());

        let inspected = ExchangeBuilder::connect("example.test:443").inspection_established();
        assert!(matches!(
            inspected.outcome(),
            Outcome::InspectionEstablished
        ));
        assert_eq!(inspected.timing().connect(), None);
    }
}
