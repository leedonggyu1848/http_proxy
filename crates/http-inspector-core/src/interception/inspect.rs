use super::capture_context::{CaptureContext, TunnelCapture};
use super::forward::forward_over_stream;
use super::handle::{ConnectionPermit, Engine, server_builder};
use super::upstream::{UpstreamFailure, connect_deadline, connect_within, resolve_within};
use super::{ProxyBody, empty_response, failed_response, tunnel};
use crate::https::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
use http::header::{HOST, UPGRADE};
use http::uri::Authority;
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use http_inspector_exchange::{CaptureSource, FailureStage, Outcome, RecordId};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;

const TLS_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn connect_tunnel(
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
    capture.writer.record_connect(connect_started.elapsed());

    let tunnel = TunnelCapture::start(capture);
    let on_upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        let _permit = permit;
        let client = async { on_upgrade.await.map(TokioIo::new) };
        tunnel::relay(client, upstream, tunnel).await;
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
            if !capture.is_stopped() {
                capture.fail(FailureStage::Upgrade, &error.to_string());
            }
            return;
        }
        Err(_) => {
            if !capture.is_stopped() {
                capture.fail(FailureStage::Upgrade, "CONNECT upgrade timed out");
            }
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
    let capture = CaptureContext::start(
        request.method(),
        request.headers(),
        request.extensions().get::<RecordId>().copied(),
        CaptureSource::InspectedHttps,
        &target,
        &engine,
    );

    let response = match validate_inner_request(&request, &destination) {
        Ok(()) => {
            forward_https(
                request.map(BodyExt::boxed),
                &engine,
                &capture,
                &inspector,
                &destination,
            )
            .await
        }
        Err(message) => failed_response(
            &capture,
            FailureStage::Request,
            StatusCode::BAD_REQUEST,
            &message.to_string(),
        ),
    };
    Ok(response)
}

/// Reason an inspected request cannot be forwarded inside its CONNECT tunnel.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
enum InnerRequestError {
    #[error("nested CONNECT is not supported")]
    NestedConnect,
    #[error("HTTPS requests inside CONNECT must use origin-form targets")]
    NonOriginTarget,
    #[error("protocol upgrades inside inspected HTTPS are not supported")]
    ProtocolUpgrade,
    #[error("the inspected request has no valid Host header")]
    InvalidHost,
    #[error("the inspected Host header does not match the CONNECT authority")]
    HostMismatch,
}

fn validate_inner_request<B>(
    request: &Request<B>,
    destination: &Destination,
) -> Result<(), InnerRequestError> {
    if request.method() == Method::CONNECT {
        return Err(InnerRequestError::NestedConnect);
    }
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return Err(InnerRequestError::NonOriginTarget);
    }
    if request.headers().contains_key(UPGRADE) {
        return Err(InnerRequestError::ProtocolUpgrade);
    }
    let authority = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
        .ok_or(InnerRequestError::InvalidHost)?;
    if !destination.matches_authority(&authority) {
        return Err(InnerRequestError::HostMismatch);
    }
    Ok(())
}

pub(super) async fn forward_https(
    request: Request<ProxyBody>,
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
    let stream = match timeout(
        engine.connect_timeout,
        TcpStream::connect(destination.socket_target()),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return failed_response(
                capture,
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                &error.to_string(),
            );
        }
        Err(_) => return UpstreamFailure::connect_timeout(engine).respond(capture),
    };
    let stream = match timeout(TLS_TIMEOUT, inspector.handshake(destination, stream)).await {
        Ok(Ok(stream)) => stream,
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
                "upstream TLS handshake timed out",
            );
        }
    };
    capture.writer.record_connect(connect_started.elapsed());
    forward_over_stream(request, stream, engine, capture, path).await
}

#[cfg(test)]
mod inner_request_tests {
    use super::{Destination, InnerRequestError, validate_inner_request};
    use http::{
        Method, Request,
        header::{HOST, UPGRADE},
        uri::Authority,
    };

    #[test]
    fn rejects_invalid_tunnel_requests_by_kind() -> Result<(), Box<dyn std::error::Error>> {
        let destination = Destination::from_authority(&Authority::from_static("example.com:443"))?;
        for (method, uri, host, upgrade, expected) in [
            (
                Method::CONNECT,
                "/",
                Some("example.com"),
                false,
                Some(InnerRequestError::NestedConnect),
            ),
            (
                Method::GET,
                "https://example.com/",
                Some("example.com"),
                false,
                Some(InnerRequestError::NonOriginTarget),
            ),
            (
                Method::GET,
                "/",
                Some("example.com"),
                true,
                Some(InnerRequestError::ProtocolUpgrade),
            ),
            (
                Method::GET,
                "/",
                None,
                false,
                Some(InnerRequestError::InvalidHost),
            ),
            (
                Method::GET,
                "/",
                Some("other.example"),
                false,
                Some(InnerRequestError::HostMismatch),
            ),
            (Method::GET, "/", Some("example.com"), false, None),
        ] {
            let mut builder = Request::builder().method(method).uri(uri);
            if let Some(host) = host {
                builder = builder.header(HOST, host);
            }
            if upgrade {
                builder = builder.header(UPGRADE, "websocket");
            }
            assert_eq!(
                validate_inner_request(&builder.body(())?, &destination),
                expected.map_or(Ok(()), Err)
            );
        }
        Ok(())
    }
}
