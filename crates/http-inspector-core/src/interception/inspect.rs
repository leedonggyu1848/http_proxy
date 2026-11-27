use super::capture_context::{CaptureContext, TunnelCapture};
use super::forward::{forward_pooled, handshake_http1};
use super::handle::{ConnectionPermit, Engine, server_builder};
use super::pool::{PoolKey, PooledConnection};
use super::upstream::{UpstreamFailure, connect_deadline, connect_within, resolve_within};
use super::{ProxyBody, empty_response, failed_response, tunnel};
use crate::https::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
use http::header::{HOST, UPGRADE};
use http::uri::Authority;
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use http_inspector_exchange::{CaptureSource, FailureStage, Outcome};
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
    let attribution = capture.writer.exchange().connection().cloned();
    let service = service_fn(move |mut request: Request<Incoming>| {
        if let Some(attribution) = attribution.as_ref() {
            request.extensions_mut().insert(attribution.clone());
        }
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
    let capture = CaptureContext::start(&request, CaptureSource::InspectedHttps, &target, &engine);

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
    let key = PoolKey::https(destination, inspector.upstream_tls_key().clone());
    forward_pooled(request, path, key, engine, capture, || {
        dial_https(destination, inspector, engine)
    })
    .await
}

async fn dial_https(
    destination: &Destination,
    inspector: &TlsInspector,
    engine: &Engine,
) -> Result<(PooledConnection, Duration), UpstreamFailure> {
    let connect_started = Instant::now();
    let stream = match timeout(
        engine.connect_timeout,
        TcpStream::connect(destination.socket_target()),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return Err(UpstreamFailure::new(
                FailureStage::Connect,
                StatusCode::BAD_GATEWAY,
                error.to_string(),
            ));
        }
        Err(_) => return Err(UpstreamFailure::connect_timeout(engine)),
    };
    let stream = match timeout(TLS_TIMEOUT, inspector.handshake(destination, stream)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(UpstreamTlsError::Handshake(error))) => {
            return Err(UpstreamFailure::new(
                FailureStage::UpstreamTls,
                StatusCode::BAD_GATEWAY,
                error.to_string(),
            ));
        }
        Err(_) => {
            return Err(UpstreamFailure::new(
                FailureStage::UpstreamTls,
                StatusCode::GATEWAY_TIMEOUT,
                "upstream TLS handshake timed out".to_owned(),
            ));
        }
    };
    let connect_duration = connect_started.elapsed();
    let connection = handshake_http1(stream).await?;
    Ok((connection, connect_duration))
}

#[cfg(test)]
mod inner_request_tests {
    use super::super::capture_context::{CaptureContext, EventSink};
    use super::super::handle::Engine;
    use super::super::infallible_to_hyper;
    use super::super::pool::UpstreamPool;
    use super::{Destination, InnerRequestError, forward_https, validate_inner_request};
    use crate::https::{HttpsInspection, TlsInspector};
    use bytes::Bytes;
    use http::{
        Method, Request,
        header::{HOST, UPGRADE},
        uri::Authority,
    };
    use http_body_util::{BodyExt, Empty};
    use http_inspector_exchange::{CaptureSource, Outcome, SessionId};
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    use rustls::ServerConfig;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{Semaphore, mpsc};
    use tokio_rustls::TlsAcceptor;

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

    fn self_signed_root(
        common_name: &str,
    ) -> Result<(String, Issuer<'static, KeyPair>), Box<dyn std::error::Error>> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(30);
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, common_name);
        params.distinguished_name = name;
        let cert = params.self_signed(&key)?;
        Ok((cert.pem(), Issuer::new(params, key)))
    }

    fn tls_acceptor(
        issuer: &Issuer<'_, KeyPair>,
    ) -> Result<TlsAcceptor, Box<dyn std::error::Error>> {
        let server_key = KeyPair::generate()?;
        let mut server_params = CertificateParams::new(vec!["localhost".to_owned()])?;
        let now = time::OffsetDateTime::now_utc();
        server_params.not_before = now - time::Duration::days(1);
        server_params.not_after = now + time::Duration::days(7);
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        let cert = server_params.signed_by(&server_key, issuer)?;
        let private_key = PrivatePkcs8KeyDer::from(server_key.serialize_der()).into();
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private_key)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    async fn read_http_head<S: tokio::io::AsyncRead + Unpin>(
        stream: &mut S,
    ) -> std::io::Result<bool> {
        let mut data = Vec::new();
        while !data.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                return Ok(false);
            }
            data.extend_from_slice(&byte);
        }
        Ok(true)
    }

    fn test_engine() -> (Engine, mpsc::Receiver<http_inspector_exchange::Exchange>) {
        let (sender, receiver) = mpsc::channel(16);
        let engine = Engine {
            events: EventSink {
                sender,
                dropped: Arc::new(AtomicU64::new(0)),
                notifier: Arc::new(OnceLock::new()),
                log: None,
                body_store: None,
            },
            session: SessionId::next(),
            stopped: Arc::new(AtomicBool::new(false)),
            inspector: None,
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(5),
            connections: Arc::new(Semaphore::new(16)),
            upstream_pool: Arc::new(UpstreamPool::new(4, Duration::from_secs(30))),
        };
        (engine, receiver)
    }

    async fn send_https_request(
        engine: &Engine,
        destination: &Destination,
        inspector: &TlsInspector,
        port: u16,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let req = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(HOST, format!("localhost:{port}"))
            .body(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())?;
        let capture = CaptureContext::start(
            &req,
            CaptureSource::InspectedHttps,
            &destination.absolute_target(path),
            engine,
        );
        let resp = forward_https(req, engine, &capture, inspector, destination).await;
        let _ = resp.into_body().collect().await?;
        Ok(())
    }

    #[tokio::test]
    async fn does_not_share_pooled_https_connections_across_tls_configs()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let (primary_pem, primary_issuer) = self_signed_root("Upstream Root A")?;
        let (extra_pem, _extra_issuer) = self_signed_root("Upstream Root B")?;
        let primary_bundle = temp.path().join("root-a.pem");
        let combined_bundle = temp.path().join("root-ab.pem");
        std::fs::write(&primary_bundle, &primary_pem)?;
        std::fs::write(&combined_bundle, format!("{primary_pem}\n{extra_pem}"))?;

        let (first_inspector, _) = TlsInspector::open(
            &HttpsInspection::new(temp.path().join("ca-a"))
                .with_additional_upstream_root(primary_bundle),
        )?;
        let (second_inspector, _) = TlsInspector::open(
            &HttpsInspection::new(temp.path().join("ca-b"))
                .with_additional_upstream_root(combined_bundle),
        )?;
        assert_ne!(
            first_inspector.upstream_tls_key(),
            second_inspector.upstream_tls_key()
        );

        let acceptor = tls_acceptor(&primary_issuer)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let handshakes = Arc::new(AtomicUsize::new(0));
        let server_handshakes = Arc::clone(&handshakes);
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                let count = Arc::clone(&server_handshakes);
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    count.fetch_add(1, Ordering::SeqCst);
                    while read_http_head(&mut tls).await.unwrap_or(false) {
                        if tls
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });

        let (engine, mut receiver) = test_engine();
        let destination =
            Destination::from_authority(&format!("localhost:{port}").parse::<Authority>()?)?;

        send_https_request(&engine, &destination, &first_inspector, port, "/a1").await?;
        let r1 = receiver.recv().await.ok_or("missing r1")?;
        assert!(matches!(r1.outcome(), Outcome::Response(_)));
        let c1 = r1.upstream_connection().ok_or("missing c1")?;
        assert!(!c1.is_reused());

        // Different TLS verification roots must NOT reuse c1.
        send_https_request(&engine, &destination, &second_inspector, port, "/b1").await?;
        let r2 = receiver.recv().await.ok_or("missing r2")?;
        assert!(matches!(r2.outcome(), Outcome::Response(_)));
        let c2 = r2.upstream_connection().ok_or("missing c2")?;
        assert!(!c2.is_reused());
        assert_ne!(c1.id(), c2.id());
        assert_eq!(handshakes.load(Ordering::SeqCst), 2);

        // Second request with the first inspector reuses c1 without another TLS handshake.
        send_https_request(&engine, &destination, &first_inspector, port, "/a2").await?;
        let r3 = receiver.recv().await.ok_or("missing r3")?;
        assert!(matches!(r3.outcome(), Outcome::Response(_)));
        let c3 = r3.upstream_connection().ok_or("missing c3")?;
        assert!(c3.is_reused());
        assert_eq!(c3.id(), c1.id());
        assert_eq!(r3.timing().connect(), None);
        assert_eq!(handshakes.load(Ordering::SeqCst), 2);

        engine.upstream_pool.close_all();
        server.abort();
        Ok(())
    }
}
