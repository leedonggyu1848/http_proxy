use super::capture_context::CaptureContext;
use super::handle::Engine;
use super::headers::remove_hop_by_hop_headers;
use super::target::upstream_target;
use super::upstream::connect_upstream;
use super::{ProxyBody, failed_response};
use crate::journal::BodySide;
use crate::{
    BODY_PREVIEW_BYTES, BodyPreview, CapturedResponse, ERROR_BYTES, FailureStage, ObservedBody,
    Outcome, bounded_text, capture_headers, response_body_preview,
};
use http::{Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::client::conn::http1 as client_http1;
use hyper::ext::ReasonPhrase;
use hyper_util::rt::TokioIo;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

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

    let connect_started = Instant::now();
    let stream = match connect_upstream(&target_parts.authority, 80, engine).await {
        Ok(stream) => stream,
        Err(failure) => return failure.respond(capture),
    };
    capture.exchange.record_connect(connect_started.elapsed());

    forward_over_stream(request, stream, engine, capture, target_parts.path).await
}

pub(super) async fn forward_over_stream<IO>(
    mut request: Request<ProxyBody>,
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
    let request = request
        .map(|body| ObservedBody::new(body, request_body, capture.body_log(BodySide::Request)));

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
                log.response_head(capture.id(), response.headers());
            }
            let response_body = response_body_preview(response.headers());
            capture.settle(Outcome::Response(captured_response(
                &response,
                response_body.clone(),
            )));
            remove_hop_by_hop_headers(response.headers_mut());
            response
                .map(|body| {
                    ObservedBody::new(body, response_body, capture.body_log(BodySide::Response))
                })
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
