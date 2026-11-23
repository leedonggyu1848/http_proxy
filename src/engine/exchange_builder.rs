//! Captured exchanges for tests, built without running a proxy.
//!
//! Compiled only with the `test-support` feature, which release builds leave
//! off. The builder records each state through the same exchange, body, and
//! tunnel writers and the same header, target, and preview rules as the
//! capture path, so it cannot produce a record the proxy would never
//! produce: sensitive header values are redacted, long text is bounded, and
//! response bodies keep the same preview limits. Inputs that no proxy path
//! could have recorded, such as a failure stage another path reports or an
//! HTTP/2 upstream response, are refused with a [`FixtureError`].

use std::sync::OnceLock;
use std::time::Duration;

use http::uri::Authority;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, Uri, Version};
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

/// Fixture input that no proxy path could have recorded.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FixtureError {
    /// `CONNECT` given to an HTTP constructor. The proxy routes `CONNECT` to
    /// the tunnel path and rejects one inside an inspected session; build it
    /// with [`ExchangeBuilder::connect`].
    #[error("CONNECT requests are built with ExchangeBuilder::connect")]
    ConnectMethod,
    /// A failure stage that the proxy reports only on another path.
    #[error("the proxy never records a {stage} failure for {route}")]
    FailureStage {
        /// The refused stage.
        stage: FailureStage,
        /// The kind of record it was given for.
        route: &'static str,
    },
    /// A target the proxy could not have forwarded, or reconstructed.
    #[error("{route} need {expected}, not {target:?}")]
    Target {
        /// The kind of record.
        route: &'static str,
        /// What a forwarded target looks like on that path.
        expected: &'static str,
        /// The refused target.
        target: String,
    },
    /// Byte counts on a tunnel whose client upgrade failed; the relay starts
    /// counting only after the upgrade.
    #[error("a tunnel whose upgrade failed relayed no bytes")]
    BytesBeforeUpgrade,
    /// An upstream response version other than HTTP/1.0 or HTTP/1.1; the
    /// proxy talks HTTP/1 to upstream servers.
    #[error("the proxy talks HTTP/1 upstream, so a response cannot be {0:?}")]
    Version(Version),
    /// A 1xx status other than `101`, which the HTTP client consumes before
    /// the final response.
    #[error("{0} is never a final response")]
    InformationalStatus(StatusCode),
    /// A body on a response that cannot have one: to `HEAD`, or with status
    /// `101`, `204`, or `304`.
    #[error("a {status} response to {method} has no body")]
    UnexpectedBody {
        /// The request method.
        method: Method,
        /// The response status.
        status: StatusCode,
    },
}

/// The path through the proxy that records an HTTP fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpRoute {
    Plain,
    Inspected,
}

impl HttpRoute {
    const fn source(self) -> CaptureSource {
        match self {
            Self::Plain => CaptureSource::HttpProxy,
            Self::Inspected => CaptureSource::InspectedHttps,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain HTTP requests",
            Self::Inspected => "inspected HTTPS requests",
        }
    }

    /// Reports whether the path reports a failure at `stage`. Only the
    /// inspected path makes an upstream TLS handshake; `CONNECT`-only and
    /// tunnel stages never end an HTTP exchange.
    const fn reports(self, stage: FailureStage) -> bool {
        match stage {
            FailureStage::Request
            | FailureStage::Connect
            | FailureStage::ConnectTimeout
            | FailureStage::Response
            | FailureStage::ResponseTimeout => true,
            FailureStage::UpstreamTls => matches!(self, Self::Inspected),
            FailureStage::Upgrade | FailureStage::ClientTls | FailureStage::Tunnel => false,
        }
    }

    /// Checks that `target` is one the path forwards: an absolute URL with
    /// the path's scheme and a host.
    fn check_target(self, target: &str) -> Result<(), FixtureError> {
        let scheme = match self {
            Self::Plain => "http",
            Self::Inspected => "https",
        };
        let forwardable = target
            .parse::<Uri>()
            .is_ok_and(|uri| uri.scheme_str() == Some(scheme) && uri.host().is_some());
        if forwardable {
            Ok(())
        } else {
            Err(FixtureError::Target {
                route: self.name(),
                expected: match self {
                    Self::Plain => "an absolute http:// URL",
                    Self::Inspected => "an absolute https:// URL",
                },
                target: target.to_owned(),
            })
        }
    }
}

/// Reports whether a `CONNECT` request that the proxy settled can have
/// failed at `stage`: before connecting (`Request`, `Connect`,
/// `ConnectTimeout`), or while starting an inspection session (`Upgrade`,
/// `ClientTls`).
const fn connect_reports(stage: FailureStage) -> bool {
    matches!(
        stage,
        FailureStage::Request
            | FailureStage::Connect
            | FailureStage::ConnectTimeout
            | FailureStage::Upgrade
            | FailureStage::ClientTls
    )
}

const CONNECT_ROUTE: &str = "CONNECT requests";
const TUNNEL_ROUTE: &str = "relayed CONNECT tunnels";

/// Checks that a `CONNECT` target is an authority, as every `CONNECT`
/// outcome past the `Request` stage requires.
fn check_authority(target: &str) -> Result<(), FixtureError> {
    target
        .parse::<Authority>()
        .map(drop)
        .map_err(|_| FixtureError::Target {
            route: CONNECT_ROUTE,
            expected: "a host:port authority",
            target: target.to_owned(),
        })
}

/// Builds an HTTP exchange, from a plain proxy request or one decrypted
/// inside an inspected `CONNECT` session, in any state the proxy records.
///
/// Use [`ExchangeBuilder::connect`] for a `CONNECT` request itself; the
/// HTTP constructors refuse that method, as the proxy always routes it to
/// its own path.
///
/// Each final method returns [`FixtureError`] for input that the chosen
/// path could not have recorded.
///
/// ```
/// use http_proxy::{BodyLifecycle, ExchangeBuilder, FailureStage, ResponseFixture};
///
/// # fn main() -> Result<(), http_proxy::FixtureError> {
/// let exchange = ExchangeBuilder::http(http::Method::GET, "http://example.test/")?
///     .respond(ResponseFixture::new(http::StatusCode::OK).body("hello", BodyLifecycle::Complete))?;
/// assert!(!exchange.is_live());
/// assert!(ExchangeBuilder::http(http::Method::CONNECT, "example.test:443").is_err());
/// let tunnel_stage = ExchangeBuilder::http(http::Method::GET, "http://example.test/")?
///     .fail(FailureStage::Tunnel, "reset");
/// assert!(tunnel_stage.is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[must_use]
pub struct ExchangeBuilder {
    route: HttpRoute,
    head: RequestHead,
    replay_of: Option<RecordId>,
    request_body: Option<(Vec<u8>, BodyLifecycle)>,
}

impl ExchangeBuilder {
    /// Starts a plain request forwarded by the proxy. `target` is the
    /// request target the client sent; any target may arrive, but only an
    /// absolute `http://` URL gets past the `Request` stage.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::ConnectMethod`] for `CONNECT`, which the proxy
    /// never forwards as a plain request; use [`ExchangeBuilder::connect`].
    pub fn http(method: Method, target: impl Into<String>) -> Result<Self, FixtureError> {
        Self::with_route(HttpRoute::Plain, method, target.into())
    }

    /// Starts a request decrypted inside an inspected `CONNECT` session.
    /// `target` is the absolute `https://` URL the proxy reconstructed.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::ConnectMethod`] for `CONNECT`, which the proxy
    /// rejects inside an inspected session before capturing it, and
    /// [`FixtureError::Target`] for a target that is not an absolute
    /// `https://` URL, which the proxy never reconstructs.
    pub fn inspected_https(
        method: Method,
        target: impl Into<String>,
    ) -> Result<Self, FixtureError> {
        let target = target.into();
        HttpRoute::Inspected.check_target(&target)?;
        Self::with_route(HttpRoute::Inspected, method, target)
    }

    /// Starts a `CONNECT` request for `authority`, such as `example.test:443`.
    pub fn connect(authority: impl Into<String>) -> ConnectExchangeBuilder {
        ConnectExchangeBuilder {
            head: RequestHead::new(CaptureSource::Connect, Method::CONNECT, authority.into()),
        }
    }

    fn with_route(route: HttpRoute, method: Method, target: String) -> Result<Self, FixtureError> {
        if method == Method::CONNECT {
            return Err(FixtureError::ConnectMethod);
        }
        Ok(Self {
            route,
            head: RequestHead::new(route.source(), method, target),
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
    /// `Connect`, `ConnectTimeout`, and `UpstreamTls`), and the target must
    /// then be one the proxy forwards.
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
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::Target`] when a request body was set for a
    /// target the proxy does not forward.
    pub fn in_flight(self) -> Result<Exchange, FixtureError> {
        Ok(self.start()?.exchange().clone())
    }

    /// Returns an exchange whose response headers arrived, with the response
    /// body observed as `response` describes.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError`] for a target the proxy does not forward or a
    /// response it cannot receive (see [`ResponseFixture`]).
    pub fn respond(self, response: ResponseFixture) -> Result<Exchange, FixtureError> {
        response.check(&self.head.method)?;
        let writer = self.forward()?;
        let (head, bytes, end) = response.into_parts();
        let body = response_body_preview(head.headers());
        writer.settle(Outcome::Response(captured_response(&head, body.preview())));
        body.observe(&bytes);
        end_body(&body, end);
        Ok(writer.exchange().clone())
    }

    /// Returns an exchange that failed at `stage` before response headers
    /// arrived.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::FailureStage`] for a stage this path never
    /// reports (`Upgrade`, `ClientTls`, `Tunnel`, and `UpstreamTls` on plain
    /// HTTP), and [`FixtureError::Target`] for a stage past `Request` with a
    /// target the proxy does not forward.
    pub fn fail(self, stage: FailureStage, message: &str) -> Result<Exchange, FixtureError> {
        if !self.route.reports(stage) {
            return Err(FixtureError::FailureStage {
                stage,
                route: self.route.name(),
            });
        }
        let writer = match stage {
            FailureStage::Response | FailureStage::ResponseTimeout => self.forward()?,
            FailureStage::Request => self.head.start(self.replay_of),
            _ => {
                self.route.check_target(&self.head.target)?;
                self.head.start(self.replay_of)
            }
        };
        writer.settle(Outcome::Failed {
            stage,
            message: bounded_text(message, ERROR_BYTES),
        });
        Ok(writer.exchange().clone())
    }

    /// Returns an exchange whose client went away before it settled.
    ///
    /// # Errors
    ///
    /// As [`ExchangeBuilder::in_flight`].
    pub fn cancel(self) -> Result<Exchange, FixtureError> {
        self.settle(Outcome::Cancelled)
    }

    /// Returns an exchange cut short because the proxy stopped.
    ///
    /// # Errors
    ///
    /// As [`ExchangeBuilder::in_flight`].
    pub fn stop(self) -> Result<Exchange, FixtureError> {
        self.settle(Outcome::Incomplete)
    }

    fn settle(self, outcome: Outcome) -> Result<Exchange, FixtureError> {
        let writer = self.start()?;
        writer.settle(outcome);
        Ok(writer.exchange().clone())
    }

    /// Publishes the exchange and, when a request body was set, forwards it.
    fn start(self) -> Result<ExchangeWriter, FixtureError> {
        if self.request_body.is_some() {
            self.forward()
        } else {
            Ok(self.head.start(self.replay_of))
        }
    }

    /// Publishes the exchange and forwards the request as the proxy does:
    /// connect, then attach the request body and stream it. A request with
    /// no body set is forwarded with an empty one.
    fn forward(self) -> Result<ExchangeWriter, FixtureError> {
        self.route.check_target(&self.head.target)?;
        let writer = self.head.start(self.replay_of);
        self.head.connected(&writer);
        let (bytes, end) = self
            .request_body
            .unwrap_or((Vec::new(), BodyLifecycle::Complete));
        let body = BodyPreviewWriter::new(BODY_PREVIEW_BYTES);
        writer.attach_request_body(body.preview());
        body.observe(&bytes);
        end_body(&body, end);
        Ok(writer)
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
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::Target`] for a target that is not an
    /// authority, [`FixtureError::FailureStage`] for a failure other than
    /// `Upgrade` or `Tunnel`, which are the only ones the relay records, and
    /// [`FixtureError::BytesBeforeUpgrade`] for an `Upgrade` failure with
    /// byte counts.
    pub fn tunnel(
        self,
        client_to_upstream: usize,
        upstream_to_client: usize,
        state: TunnelState,
    ) -> Result<Exchange, FixtureError> {
        check_authority(&self.head.target)?;
        if let TunnelState::Failed { stage, .. } = &state {
            match stage {
                FailureStage::Upgrade if client_to_upstream > 0 || upstream_to_client > 0 => {
                    return Err(FixtureError::BytesBeforeUpgrade);
                }
                FailureStage::Upgrade | FailureStage::Tunnel => {}
                _ => {
                    return Err(FixtureError::FailureStage {
                        stage: *stage,
                        route: TUNNEL_ROUTE,
                    });
                }
            }
        }
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
        Ok(writer.exchange().clone())
    }

    /// Returns a `CONNECT` request whose TLS inspection session started.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::Target`] for a target that is not an authority.
    pub fn inspection_established(self) -> Result<Exchange, FixtureError> {
        check_authority(&self.head.target)?;
        Ok(self.settle(Outcome::InspectionEstablished))
    }

    /// Returns a `CONNECT` request that failed at `stage`.
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::FailureStage`] for a stage that no `CONNECT`
    /// path reports (`Response`, `ResponseTimeout`, `UpstreamTls`), or
    /// `Tunnel`, which the relay records inside [`ConnectExchangeBuilder::tunnel`],
    /// and [`FixtureError::Target`] for a stage past `Request` with a target
    /// that is not an authority.
    pub fn fail(self, stage: FailureStage, message: &str) -> Result<Exchange, FixtureError> {
        if !connect_reports(stage) {
            return Err(FixtureError::FailureStage {
                stage,
                route: CONNECT_ROUTE,
            });
        }
        if stage != FailureStage::Request {
            check_authority(&self.head.target)?;
        }
        Ok(self.settle(Outcome::Failed {
            stage,
            message: bounded_text(message, ERROR_BYTES),
        }))
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
///
/// [`ExchangeBuilder::respond`] refuses a response the proxy cannot receive:
/// a version other than HTTP/1.0 or HTTP/1.1, a 1xx status other than `101`,
/// or a body on a response to `HEAD` or with status `101`, `204`, or `304`.
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

    /// Checks that the proxy can receive this response to `method`.
    fn check(&self, method: &Method) -> Result<(), FixtureError> {
        if !matches!(self.version, Version::HTTP_10 | Version::HTTP_11) {
            return Err(FixtureError::Version(self.version));
        }
        if self.status.is_informational() && self.status != StatusCode::SWITCHING_PROTOCOLS {
            return Err(FixtureError::InformationalStatus(self.status));
        }
        let bodiless = *method == Method::HEAD
            || matches!(
                self.status,
                StatusCode::SWITCHING_PROTOCOLS | StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
            );
        if bodiless && !self.body.is_empty() {
            return Err(FixtureError::UnexpectedBody {
                method: method.clone(),
                status: self.status,
            });
        }
        Ok(())
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

    type TestResult = Result<(), FixtureError>;

    const ALL_STAGES: [FailureStage; 9] = [
        FailureStage::Request,
        FailureStage::Connect,
        FailureStage::ConnectTimeout,
        FailureStage::Response,
        FailureStage::ResponseTimeout,
        FailureStage::Upgrade,
        FailureStage::ClientTls,
        FailureStage::UpstreamTls,
        FailureStage::Tunnel,
    ];

    fn get() -> Result<ExchangeBuilder, FixtureError> {
        ExchangeBuilder::http(Method::GET, "http://example.test/")
    }

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
            .in_flight()?;
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
        let exchange = get()?.respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
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
        let streaming = get()?.respond(
            ResponseFixture::new(StatusCode::OK).body("partial", BodyLifecycle::Streaming),
        )?;
        assert!(streaming.is_live());
        assert_eq!(streaming.received_bytes(), Some(7));

        let large = vec![b'x'; BODY_PREVIEW_BYTES + 1];
        let truncated = get()?.respond(
            ResponseFixture::new(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
                .body(large, BodyLifecycle::Complete),
        )?;
        let snapshot = response_of(&truncated).and_then(|response| response.body().snapshot());
        assert_eq!(
            snapshot.map(|body| body.retention()),
            Some(BodyRetention::Truncated)
        );
        Ok(())
    }

    #[test]
    fn attaches_the_request_body_only_once_forwarding_began() -> TestResult {
        let post = || ExchangeBuilder::http(Method::POST, "http://example.test/");
        let before = post()?
            .request_body("a=1", BodyLifecycle::Complete)
            .fail(FailureStage::Connect, "refused")?;
        assert!(before.request().body().is_none());
        assert_eq!(before.timing().connect(), None);

        let after = post()?
            .request_body("a=1", BodyLifecycle::Complete)
            .fail(FailureStage::ResponseTimeout, "no response")?;
        assert!(after.request().body().is_some());
        assert!(after.outcome().is_failure());
        assert_eq!(after.timing().connect(), Some(Duration::ZERO));
        Ok(())
    }

    #[test]
    fn records_a_connect_time_only_after_connecting() -> TestResult {
        let connect = Duration::from_millis(7);
        let https = || ExchangeBuilder::inspected_https(Method::GET, "https://example.test/");
        for stage in [
            FailureStage::Request,
            FailureStage::Connect,
            FailureStage::ConnectTimeout,
        ] {
            let failed = get()?.connect_time(connect).fail(stage, "failed")?;
            assert_eq!(failed.timing().connect(), None, "{stage}");
        }
        let tls = https()?
            .connect_time(connect)
            .fail(FailureStage::UpstreamTls, "bad certificate")?;
        assert_eq!(tls.timing().connect(), None);
        for unforwarded in [
            get()?.connect_time(connect).in_flight()?,
            ExchangeBuilder::connect("example.test:443")
                .connect_time(connect)
                .fail(FailureStage::Connect, "refused")?,
            ExchangeBuilder::connect("example.test:443")
                .connect_time(connect)
                .inspection_established()?,
        ] {
            assert_eq!(unforwarded.timing().connect(), None);
        }
        let responded = https()?
            .connect_time(connect)
            .respond(ResponseFixture::new(StatusCode::OK))?;
        assert_eq!(responded.timing().connect(), Some(connect));
        let tunnel = ExchangeBuilder::connect("example.test:443")
            .connect_time(connect)
            .tunnel(0, 0, TunnelState::Closed)?;
        assert_eq!(tunnel.timing().connect(), Some(connect));
        Ok(())
    }

    #[test]
    fn refuses_connect_on_the_http_paths() {
        assert_eq!(
            ExchangeBuilder::http(Method::CONNECT, "example.test:443").err(),
            Some(FixtureError::ConnectMethod)
        );
        assert_eq!(
            ExchangeBuilder::inspected_https(Method::CONNECT, "https://example.test/").err(),
            Some(FixtureError::ConnectMethod)
        );
    }

    #[test]
    fn accepts_only_the_failure_stages_each_path_reports() -> TestResult {
        let accepted = |result: Result<Exchange, FixtureError>| match result {
            Ok(exchange) => Ok(exchange.outcome().is_failure()),
            Err(FixtureError::FailureStage { .. }) => Ok(false),
            Err(error) => Err(error),
        };
        for stage in ALL_STAGES {
            let plain = accepted(get()?.fail(stage, "failed"))?;
            let inspected = accepted(
                ExchangeBuilder::inspected_https(Method::GET, "https://example.test/")?
                    .fail(stage, "failed"),
            )?;
            let connect =
                accepted(ExchangeBuilder::connect("example.test:443").fail(stage, "failed"))?;
            let tunnel = accepted(ExchangeBuilder::connect("example.test:443").tunnel(
                0,
                0,
                TunnelState::Failed {
                    stage,
                    message: "failed".to_owned(),
                },
            ))?;
            let http_stage = matches!(
                stage,
                FailureStage::Request
                    | FailureStage::Connect
                    | FailureStage::ConnectTimeout
                    | FailureStage::Response
                    | FailureStage::ResponseTimeout
            );
            assert_eq!(plain, http_stage, "plain {stage}");
            assert_eq!(
                inspected,
                http_stage || stage == FailureStage::UpstreamTls,
                "inspected {stage}"
            );
            assert_eq!(
                connect,
                matches!(
                    stage,
                    FailureStage::Request
                        | FailureStage::Connect
                        | FailureStage::ConnectTimeout
                        | FailureStage::Upgrade
                        | FailureStage::ClientTls
                ),
                "CONNECT {stage}"
            );
            assert_eq!(
                tunnel,
                matches!(stage, FailureStage::Upgrade | FailureStage::Tunnel),
                "tunnel {stage}"
            );
        }
        Ok(())
    }

    #[test]
    fn refuses_targets_the_proxy_would_not_forward() -> TestResult {
        let origin_form = || ExchangeBuilder::http(Method::GET, "/path");
        assert!(
            origin_form()?
                .fail(FailureStage::Request, "bad target")
                .is_ok()
        );
        assert!(origin_form()?.in_flight().is_ok());
        assert!(matches!(
            origin_form()?.respond(ResponseFixture::new(StatusCode::OK)),
            Err(FixtureError::Target { .. })
        ));
        assert!(matches!(
            origin_form()?.fail(FailureStage::Connect, "refused"),
            Err(FixtureError::Target { .. })
        ));
        assert!(matches!(
            origin_form()?
                .request_body("", BodyLifecycle::Complete)
                .cancel(),
            Err(FixtureError::Target { .. })
        ));
        assert!(matches!(
            ExchangeBuilder::inspected_https(Method::GET, "http://example.test/"),
            Err(FixtureError::Target { .. })
        ));

        let no_authority = || ExchangeBuilder::connect("/");
        assert!(
            no_authority()
                .fail(FailureStage::Request, "no authority")
                .is_ok()
        );
        assert!(matches!(
            no_authority().tunnel(0, 0, TunnelState::Open),
            Err(FixtureError::Target { .. })
        ));
        assert!(matches!(
            no_authority().inspection_established(),
            Err(FixtureError::Target { .. })
        ));
        Ok(())
    }

    #[test]
    fn refuses_responses_the_proxy_cannot_receive() -> TestResult {
        assert_eq!(
            get()?
                .respond(ResponseFixture::new(StatusCode::OK).version(Version::HTTP_2))
                .err(),
            Some(FixtureError::Version(Version::HTTP_2))
        );
        assert_eq!(
            get()?
                .respond(ResponseFixture::new(StatusCode::CONTINUE))
                .err(),
            Some(FixtureError::InformationalStatus(StatusCode::CONTINUE))
        );
        assert!(
            get()?
                .respond(ResponseFixture::new(StatusCode::SWITCHING_PROTOCOLS))
                .is_ok()
        );
        let with_body = |status| ResponseFixture::new(status).body("x", BodyLifecycle::Complete);
        for status in [StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED] {
            assert!(matches!(
                get()?.respond(with_body(status)),
                Err(FixtureError::UnexpectedBody { .. })
            ));
        }
        assert!(matches!(
            ExchangeBuilder::http(Method::HEAD, "http://example.test/")?
                .respond(with_body(StatusCode::OK)),
            Err(FixtureError::UnexpectedBody { .. })
        ));
        assert!(get()?.respond(with_body(StatusCode::OK)).is_ok());
        Ok(())
    }

    #[test]
    fn records_replays_cancellations_and_stops() -> TestResult {
        let original = get()?.cancel()?;
        assert!(matches!(original.outcome(), Outcome::Cancelled));
        let replay = get()?.replay_of(original.id()).stop()?;
        assert_eq!(replay.replay_of(), Some(original.id()));
        assert!(matches!(replay.outcome(), Outcome::Incomplete));
        assert_ne!(replay.id(), original.id());
        assert_eq!(replay.session(), original.session());
        Ok(())
    }

    #[test]
    fn builds_connect_tunnels_in_every_state() -> TestResult {
        let connect = || ExchangeBuilder::connect("example.test:443");
        let open = connect().tunnel(3, 5, TunnelState::Open)?;
        assert_eq!(open.source(), CaptureSource::Connect);
        assert!(open.is_live());
        assert_eq!(open.received_bytes(), Some(5));

        let failed = connect().tunnel(
            0,
            0,
            TunnelState::Failed {
                stage: FailureStage::Tunnel,
                message: "reset".to_owned(),
            },
        )?;
        assert!(failed.outcome().is_failure());
        assert!(!failed.is_live());
        assert_eq!(
            connect()
                .tunnel(
                    4,
                    0,
                    TunnelState::Failed {
                        stage: FailureStage::Upgrade,
                        message: "closed".to_owned(),
                    },
                )
                .err(),
            Some(FixtureError::BytesBeforeUpgrade)
        );

        let inspected = connect().inspection_established()?;
        assert!(matches!(
            inspected.outcome(),
            Outcome::InspectionEstablished
        ));
        assert_eq!(inspected.timing().connect(), None);
        Ok(())
    }
}
