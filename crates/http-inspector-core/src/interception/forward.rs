use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::request::Parts;
use http::{HeaderMap, Method, Request, Response, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use http_inspector_exchange::http_semantics::remove_hop_by_hop_headers;
use http_inspector_exchange::{
    BodyPreview, BodyPreviewWriter, BodySide, ConnectionId, FailureStage, Outcome,
};
use hyper::body::Incoming;
use hyper::client::conn::http1 as client_http1;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use super::capture::{BODY_PREVIEW_BYTES, ObservedBody, captured_response, response_body_preview};
use super::capture_context::CaptureContext;
use super::handle::Engine;
use super::pool::{
    MAX_RETRY_BODY_BYTES, PoolKey, PoolReturnBody, PooledConnection, RetryBodyHandle,
    is_idempotent_method, is_stale_connection_error, replayable_body, response_allows_keep_alive,
};
use super::target::upstream_target;
use super::upstream::{UpstreamFailure, connect_upstream};
use super::{ProxyBody, failed_response};

#[derive(Clone)]
struct RequestHead {
    method: Method,
    uri: Uri,
    version: Version,
    headers: HeaderMap,
}

impl RequestHead {
    fn from_parts(parts: &Parts) -> Self {
        Self {
            method: parts.method.clone(),
            uri: parts.uri.clone(),
            version: parts.version,
            headers: parts.headers.clone(),
        }
    }

    fn into_request(self, body: ProxyBody) -> Request<ProxyBody> {
        let mut request = Request::new(body);
        *request.method_mut() = self.method;
        *request.uri_mut() = self.uri;
        *request.version_mut() = self.version;
        *request.headers_mut() = self.headers;
        request
    }
}

struct ForwardContext<'a> {
    key: PoolKey,
    request_method: Method,
    request_version: Version,
    engine: &'a Engine,
    capture: &'a CaptureContext,
    request_body_preview: BodyPreview,
}

enum SendOutcome {
    Response(Response<Incoming>),
    Stale(hyper::Error),
    Failed(Response<ProxyBody>),
}

pub(super) async fn forward_http(
    request: Request<ProxyBody>,
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
                &message.to_string(),
            );
        }
    };

    let authority = target_parts.authority;
    let key = PoolKey::plain(&authority, 80);
    forward_pooled(request, target_parts.path, key, engine, capture, || {
        dial_plain(&authority, engine)
    })
    .await
}

async fn dial_plain(
    authority: &http::uri::Authority,
    engine: &Engine,
) -> Result<(PooledConnection, Duration), UpstreamFailure> {
    let connect_started = Instant::now();
    let stream = connect_upstream(authority, 80, engine).await?;
    let connect_duration = connect_started.elapsed();
    let connection = handshake_http1(stream).await?;
    Ok((connection, connect_duration))
}

/// Runs the HTTP/1.1 client handshake over `stream` and spawns its connection
/// driver task, returning a fresh [`PooledConnection`].
pub(super) async fn handshake_http1<IO>(stream: IO) -> Result<PooledConnection, UpstreamFailure>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sender, connection) = client_http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| {
            UpstreamFailure::new(
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                error.to_string(),
            )
        })?;
    let driver = tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "upstream HTTP connection ended");
        }
    })
    .abort_handle();
    Ok(PooledConnection::fresh(
        ConnectionId::next(),
        sender,
        driver,
    ))
}

/// Forwards `request` using an idle pooled connection for `key` when available,
/// or dials a new one via `dial`. When a reused idle connection was closed by
/// the upstream server before answering, only idempotent requests are retried
/// once on a fresh connection.
pub(super) async fn forward_pooled<F, Fut>(
    mut request: Request<ProxyBody>,
    path: Uri,
    key: PoolKey,
    engine: &Engine,
    capture: &CaptureContext,
    mut dial: F,
) -> Response<ProxyBody>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(PooledConnection, Duration), UpstreamFailure>>,
{
    let mut conn = match engine.upstream_pool.checkout(&key).await {
        Some(reused) => reused,
        None => match dial().await {
            Ok((fresh, connect_duration)) => {
                capture.writer.record_connect(connect_duration);
                fresh
            }
            Err(failure) => return failure.respond(capture),
        },
    };
    capture.writer.record_upstream_connection(conn.metadata());

    *request.uri_mut() = path;
    remove_hop_by_hop_headers(request.headers_mut());

    let request_body_writer = BodyPreviewWriter::new(BODY_PREVIEW_BYTES);
    let request_body_preview = request_body_writer.preview();
    capture
        .writer
        .attach_request_body(request_body_preview.clone());

    let (parts, body) = request.into_parts();
    let request_method = parts.method.clone();
    let request_version = parts.version;
    let observed_body = ObservedBody::new(
        body,
        request_body_writer,
        capture.body_sinks(BodySide::Request),
    );
    let can_retry = conn.is_reused() && is_idempotent_method(&parts.method);
    let (first_request, retry_state) = if can_retry {
        let head = RequestHead::from_parts(&parts);
        let (attempt_body, retry_handle) = replayable_body(observed_body, MAX_RETRY_BODY_BYTES);
        (
            Request::from_parts(parts, attempt_body.boxed()),
            Some((head, retry_handle)),
        )
    } else {
        (Request::from_parts(parts, observed_body.boxed()), None)
    };

    let context = ForwardContext {
        key,
        request_method,
        request_version,
        engine,
        capture,
        request_body_preview,
    };

    match send_once(&mut conn, first_request, can_retry, engine, capture).await {
        SendOutcome::Response(response) => finalize_response(response, conn, context),
        SendOutcome::Failed(response) => {
            conn.close();
            response
        }
        SendOutcome::Stale(error) => {
            conn.close();
            retry_on_fresh_connection(retry_state, error, context, dial).await
        }
    }
}

async fn retry_on_fresh_connection<F, Fut>(
    retry_state: Option<(RequestHead, RetryBodyHandle<ObservedBody<ProxyBody>>)>,
    original_error: hyper::Error,
    context: ForwardContext<'_>,
    mut dial: F,
) -> Response<ProxyBody>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(PooledConnection, Duration), UpstreamFailure>>,
{
    let Some((head, handle)) = retry_state else {
        return failed_response(
            context.capture,
            FailureStage::Response,
            StatusCode::BAD_GATEWAY,
            &original_error.to_string(),
        );
    };
    let Some(retry_body) = handle.take_retry_body() else {
        return failed_response(
            context.capture,
            FailureStage::Response,
            StatusCode::BAD_GATEWAY,
            &original_error.to_string(),
        );
    };

    let connect_started = Instant::now();
    let (mut fresh, connect_duration) = match dial().await {
        Ok(pair) => pair,
        Err(failure) => return failure.respond(context.capture),
    };
    context
        .capture
        .writer
        .record_connect_at(connect_started, connect_duration);
    context
        .capture
        .writer
        .record_upstream_connection(fresh.metadata());

    let send_complete = context.capture.writer.restart_send();
    let retry_body = retry_body.with_on_end(send_complete);
    let retry_request = head.into_request(retry_body.boxed());
    match send_once(
        &mut fresh,
        retry_request,
        false,
        context.engine,
        context.capture,
    )
    .await
    {
        SendOutcome::Response(response) => finalize_response(response, fresh, context),
        SendOutcome::Failed(response) => {
            fresh.close();
            response
        }
        SendOutcome::Stale(error) => {
            fresh.close();
            failed_response(
                context.capture,
                FailureStage::Response,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            )
        }
    }
}

async fn send_once(
    conn: &mut PooledConnection,
    request: Request<ProxyBody>,
    allow_stale_retry: bool,
    engine: &Engine,
    capture: &CaptureContext,
) -> SendOutcome {
    let Ok(response) = timeout(
        engine.response_timeout,
        conn.sender_mut().send_request(request),
    )
    .await
    else {
        if capture.is_stopped() {
            return std::future::pending().await;
        }
        return SendOutcome::Failed(failed_response(
            capture,
            FailureStage::ResponseTimeout,
            StatusCode::GATEWAY_TIMEOUT,
            &format!(
                "no response headers within {} ms",
                engine.response_timeout.as_millis()
            ),
        ));
    };
    match response {
        Ok(response) => SendOutcome::Response(response),
        Err(_) if capture.is_stopped() => {
            // `drop(runtime)` cancels the spawned upstream connection driver
            // independently of this task; wait to be cancelled so
            // `CaptureContext::drop` records `Outcome::Incomplete` without
            // sending a synthetic error response to the client.
            std::future::pending().await
        }
        Err(error) if allow_stale_retry && is_stale_connection_error(&error) => {
            SendOutcome::Stale(error)
        }
        Err(error) => SendOutcome::Failed(failed_response(
            capture,
            FailureStage::Response,
            StatusCode::BAD_GATEWAY,
            &error.to_string(),
        )),
    }
}

fn finalize_response(
    mut response: Response<Incoming>,
    conn: PooledConnection,
    context: ForwardContext<'_>,
) -> Response<ProxyBody> {
    let keep_alive = response_allows_keep_alive(
        &context.request_method,
        context.request_version,
        response.status(),
        response.version(),
        response.headers(),
    );
    if let Some(log) = context.capture.log.as_ref() {
        log.response_head(context.capture.id(), response.headers());
    }
    let response_body = response_body_preview(response.headers());
    context.capture.settle(Outcome::Response(captured_response(
        &response,
        response_body.preview(),
    )));
    remove_hop_by_hop_headers(response.headers_mut());
    let (parts, body) = response.into_parts();
    let observed = ObservedBody::new(
        body,
        response_body,
        context.capture.body_sinks(BodySide::Response),
    );
    let pooled = PoolReturnBody::new(
        observed,
        conn.into_return(
            Arc::clone(&context.engine.upstream_pool),
            context.key,
            keep_alive,
            context.request_body_preview,
        ),
    );
    Response::from_parts(parts, pooled.boxed())
}
