use std::fs;
use std::sync::Arc;
use std::time::Duration;

use http_inspector_core::https::HttpsInspection;
use http_inspector_core::interception::{EngineState, ProxyConfig, ProxyHandle};
use http_inspector_core::replay::{REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField};
use http_inspector_exchange::{
    BodyLifecycle, BodyRetention, CapturedHeader, FailureStage, HeaderPreview, Outcome,
    TunnelPreview, TunnelSnapshot, TunnelState,
};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub mod common;

use common::{TestError, read_headers, settled_within};

/// Shorter than [`common::TEST_TIMEOUT`]; these cases expect a prompt answer.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn forwards_http_request_and_records_response() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let request = read_headers(&mut stream).await?;
        let _send_result = request_sender.send(request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    assert_eq!(proxy.state(), EngineState::Running);
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/hello?name=rust HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    let response = String::from_utf8(response)?;
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.ends_with("hello"));

    let upstream_request = timeout(TEST_TIMEOUT, request_receiver).await??;
    let upstream_request = String::from_utf8(upstream_request)?;
    assert!(upstream_request.starts_with("GET /hello?name=rust HTTP/1.1\r\n"));

    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    assert_eq!(record.request().method(), http::Method::GET);
    assert!(matches!(
        settled_within(&record, TEST_TIMEOUT).await?,
        Outcome::Response(response) if response.status() == 200
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    assert_eq!(proxy.state(), EngineState::Running);
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn strips_fields_named_by_connection_in_both_directions() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let request = read_headers(&mut stream).await?;
        let _send_result = request_sender.send(request);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nConnection: close, X-Upstream-Only\r\nX-Upstream-Only: 1\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n",
            )
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/ HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: X-Internal\r\nConnection: close, x-other\r\nx-internal: a\r\nX-Other: b\r\nX-Kept: c\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    let response = String::from_utf8(response)?.to_ascii_lowercase();
    assert!(response.starts_with("http/1.1 200 ok"));
    assert!(!response.contains("x-upstream-only"));
    assert!(!response.contains("proxy-authenticate"));

    let upstream_request = timeout(TEST_TIMEOUT, request_receiver).await??;
    let upstream_request = String::from_utf8(upstream_request)?.to_ascii_lowercase();
    assert!(!upstream_request.contains("x-internal"));
    assert!(!upstream_request.contains("x-other"));
    assert!(upstream_request.contains("x-kept: c"));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn rejects_requests_that_target_the_proxy_itself() -> Result<(), TestError> {
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let proxy_addr = proxy.local_addr();
    let port = proxy_addr.port();
    let requests = [
        format!("GET http://{proxy_addr}/ HTTP/1.1\r\nHost: {proxy_addr}\r\n\r\n"),
        format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"),
        format!("CONNECT 0.0.0.0:{port} HTTP/1.1\r\nHost: 0.0.0.0:{port}\r\n\r\n"),
    ];
    for request in requests {
        let mut client = TcpStream::connect(proxy_addr).await?;
        client.write_all(request.as_bytes()).await?;
        let response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
        let response = String::from_utf8(response)?;
        assert!(
            response.starts_with("HTTP/1.1 508 Loop Detected"),
            "{request:?} -> {response:?}"
        );

        let record = timeout(TEST_TIMEOUT, proxy.receive())
            .await?
            .ok_or("capture channel closed")?;
        assert!(matches!(
            settled_within(&record, TEST_TIMEOUT).await?,
            Outcome::Failed {
                stage: FailureStage::Request,
                ..
            }
        ));
    }
    assert!(
        timeout(Duration::from_millis(200), proxy.receive())
            .await
            .is_err(),
        "a rejected request must not be forwarded again"
    );
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_a_timeout_when_upstream_never_responds() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        tokio::time::sleep(TEST_TIMEOUT).await;
        Ok::<_, std::io::Error>(())
    });

    let config = ProxyConfig::loopback(0).with_response_timeout(Duration::from_millis(200));
    let mut proxy = ProxyHandle::start(config)?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/slow HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;

    let response = timeout(Duration::from_secs(2), read_headers(&mut client)).await??;
    assert!(String::from_utf8(response)?.starts_with("HTTP/1.1 504 Gateway Timeout"));
    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    assert!(matches!(
        settled_within(&record, TEST_TIMEOUT).await?,
        Outcome::Failed {
            stage: FailureStage::ResponseTimeout,
            ..
        }
    ));
    assert!(record.elapsed() < Duration::from_secs(2));

    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn holds_new_clients_while_the_connection_limit_is_reached() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_max_connections(1))?;
    let first = TcpStream::connect(proxy.local_addr()).await?;
    let mut second = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/ HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    second.write_all(request.as_bytes()).await?;
    let mut byte = [0_u8; 1];
    assert!(
        timeout(Duration::from_millis(300), second.read(&mut byte))
            .await
            .is_err(),
        "the second client must wait while the first holds the only slot"
    );

    drop(first);
    let response = timeout(TEST_TIMEOUT, read_headers(&mut second)).await??;
    assert!(String::from_utf8(response)?.starts_with("HTTP/1.1 204 No Content"));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn connect_creates_a_bidirectional_tunnel() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await?;
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!("CONNECT {upstream_addr} HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n");
    client.write_all(request.as_bytes()).await?;

    let response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    let response = String::from_utf8(response)?;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    client.write_all(b"ping").await?;
    let mut reply = [0_u8; 4];
    timeout(TEST_TIMEOUT, client.read_exact(&mut reply)).await??;
    assert_eq!(&reply, b"pong");

    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    assert_eq!(record.request().method(), http::Method::CONNECT);
    let Outcome::Tunnel(tunnel) = settled_within(&record, TEST_TIMEOUT).await? else {
        return Err("expected a tunnel record".into());
    };

    timeout(TEST_TIMEOUT, upstream_task).await???;
    drop(client);
    let snapshot = wait_for_tunnel_end(tunnel).await?;
    assert_eq!(snapshot.state(), &TunnelState::Closed);
    assert_eq!(snapshot.client_to_upstream(), 4);
    assert_eq!(snapshot.upstream_to_client(), 4);
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_a_tunnel_reset_by_upstream() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await?;
        stream.set_zero_linger()?;
        drop(stream);
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!("CONNECT {upstream_addr} HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n");
    client.write_all(request.as_bytes()).await?;
    let _response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    client.write_all(b"ping").await?;

    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    let Outcome::Tunnel(tunnel) = settled_within(&record, TEST_TIMEOUT).await? else {
        return Err("expected a tunnel record".into());
    };
    timeout(TEST_TIMEOUT, upstream_task).await???;
    let snapshot = wait_for_tunnel_end(tunnel).await?;
    assert!(
        matches!(
            snapshot.state(),
            TunnelState::Failed {
                stage: FailureStage::Tunnel,
                ..
            }
        ),
        "{snapshot:?}"
    );
    assert_eq!(snapshot.client_to_upstream(), 4);
    proxy.stop()?;
    Ok(())
}

async fn wait_for_tunnel_end(tunnel: &TunnelPreview) -> Result<TunnelSnapshot, TestError> {
    let poll = async {
        loop {
            let snapshot = tunnel.snapshot();
            if snapshot.state() != &TunnelState::Open {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    Ok(timeout(TEST_TIMEOUT, poll).await?)
}

#[tokio::test]
async fn captures_bounded_headers_and_body_previews() -> Result<(), TestError> {
    const BODY_SIZE: usize = 9_000;

    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _headers = read_headers(&mut stream).await?;
        let mut body = vec![0_u8; BODY_SIZE];
        stream.read_exact(&mut body).await?;
        assert!(body.iter().all(|byte| *byte == b'q'));

        let response_headers = format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {BODY_SIZE}\r\nX-Reply: yes\r\nSet-Cookie: session=secret\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response_headers.as_bytes()).await?;
        stream.write_all(&vec![b'r'; BODY_SIZE]).await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request_headers = format!(
        "POST http://{upstream_addr}/upload HTTP/1.1\r\nHost: {upstream_addr}\r\nContent-Length: {BODY_SIZE}\r\nX-Request: yes\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request_headers.as_bytes()).await?;
    client.write_all(&vec![b'q'; BODY_SIZE]).await?;

    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 201 Created"));
    assert!(response.ends_with(&vec![b'r'; BODY_SIZE]));

    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    assert_eq!(
        header_value(record.request().headers(), "x-request"),
        Some("yes")
    );
    assert_eq!(
        header_value(record.request().headers(), "authorization"),
        Some("[REDACTED]")
    );

    let request_body = record
        .request()
        .body()
        .ok_or("request body preview missing")?
        .snapshot()
        .ok_or("request body preview busy")?;
    assert_eq!(request_body.observed_bytes(), BODY_SIZE as u64);
    assert_eq!(request_body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(request_body.retention(), BodyRetention::Truncated);
    assert!(request_body.bytes().iter().all(|byte| *byte == b'q'));

    let Outcome::Response(response) = settled_within(&record, TEST_TIMEOUT).await? else {
        return Err("expected an HTTP response capture".into());
    };
    let (status, headers, body) = (response.status(), response.headers(), response.body());
    assert_eq!(status, 201);
    assert_eq!(header_value(headers, "x-reply"), Some("yes"));
    assert_eq!(header_value(headers, "set-cookie"), Some("[REDACTED]"));
    let response_body = body.snapshot().ok_or("response body preview busy")?;
    assert_eq!(response_body.observed_bytes(), BODY_SIZE as u64);
    assert_eq!(response_body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(response_body.retention(), BodyRetention::Truncated);
    assert!(response_body.bytes().iter().all(|byte| *byte == b'r'));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn decrypts_https_with_a_trusted_local_ca() -> Result<(), TestError> {
    let temporary = tempfile::tempdir()?;
    let upstream_root_path = temporary.path().join("upstream-root.pem");
    let (upstream_acceptor, upstream_root) = test_tls_server()?;
    fs::write(&upstream_root_path, upstream_root)?;

    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await?;
        let mut stream = upstream_acceptor
            .accept(stream)
            .await
            .map_err(std::io::Error::other)?;
        let request = read_headers(&mut stream).await?;
        assert!(request.starts_with(b"GET /secure HTTP/1.1\r\n"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nX-Secure: yes\r\nConnection: close\r\n\r\nsecret",
            )
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let inspection = HttpsInspection::new(temporary.path().join("proxy-ca"))
        .with_additional_upstream_root(upstream_root_path);
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_https_inspection(inspection))?;
    let mut tls = connect_to_inspection(&proxy, upstream_addr.port()).await?;
    let request = format!(
        "GET /secure HTTP/1.1\r\nHost: localhost:{}\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n",
        upstream_addr.port()
    );
    tls.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, tls.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 200 OK"));
    assert!(response.ends_with(b"secret"));

    let setup = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("inspection setup capture missing")?;
    assert!(matches!(
        settled_within(&setup, TEST_TIMEOUT).await?,
        Outcome::InspectionEstablished
    ));
    let exchange = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("decrypted exchange capture missing")?;
    assert_eq!(
        exchange.request().target(),
        format!("https://localhost:{}/secure", upstream_addr.port())
    );
    assert_eq!(
        header_value(exchange.request().headers(), "authorization"),
        Some("[REDACTED]")
    );
    let Outcome::Response(response) = settled_within(&exchange, TEST_TIMEOUT).await? else {
        return Err("expected a decrypted HTTP response capture".into());
    };
    let (status, headers, body) = (response.status(), response.headers(), response.body());
    assert_eq!(status, 200);
    assert_eq!(header_value(headers, "x-secure"), Some("yes"));
    let body = body.snapshot().ok_or("response preview busy")?;
    assert_eq!(body.bytes(), b"secret");
    assert_eq!(body.lifecycle(), BodyLifecycle::Complete);

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn rejects_an_untrusted_upstream_certificate() -> Result<(), TestError> {
    let temporary = tempfile::tempdir()?;
    let (upstream_acceptor, _) = test_tls_server()?;
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await?;
        let handshake = upstream_acceptor.accept(stream).await;
        assert!(handshake.is_err());
        Ok::<_, std::io::Error>(())
    });

    let inspection = HttpsInspection::new(temporary.path().join("proxy-ca"));
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_https_inspection(inspection))?;
    let mut tls = connect_to_inspection(&proxy, upstream_addr.port()).await?;
    let request = format!(
        "GET /untrusted HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
        upstream_addr.port()
    );
    tls.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, tls.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 502 Bad Gateway"));

    let setup = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("inspection setup capture missing")?;
    assert!(matches!(
        settled_within(&setup, TEST_TIMEOUT).await?,
        Outcome::InspectionEstablished
    ));
    let exchange = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("upstream TLS failure capture missing")?;
    assert!(matches!(
        settled_within(&exchange, TEST_TIMEOUT).await?,
        Outcome::Failed {
            stage: FailureStage::UpstreamTls,
            ..
        }
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn rejects_a_host_that_differs_from_connect() -> Result<(), TestError> {
    let temporary = tempfile::tempdir()?;
    let unused_listener = TcpListener::bind("127.0.0.1:0").await?;
    let unused_port = unused_listener.local_addr()?.port();
    drop(unused_listener);

    let inspection = HttpsInspection::new(temporary.path().join("proxy-ca"));
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_https_inspection(inspection))?;
    let mut tls = connect_to_inspection(&proxy, unused_port).await?;
    tls.write_all(b"GET /blocked HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, tls.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 400 Bad Request"));

    let setup = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("inspection setup capture missing")?;
    assert!(matches!(
        settled_within(&setup, TEST_TIMEOUT).await?,
        Outcome::InspectionEstablished
    ));
    let rejected = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("rejected request capture missing")?;
    assert!(matches!(
        settled_within(&rejected, TEST_TIMEOUT).await?,
        Outcome::Failed {
            stage: FailureStage::Request,
            ..
        }
    ));

    proxy.stop()?;
    Ok(())
}

async fn connect_to_inspection(
    proxy: &ProxyHandle,
    upstream_port: u16,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TestError> {
    let ca_path = proxy
        .ca_info()
        .ok_or("inspection CA metadata missing")?
        .certificate_path();
    let mut roots = RootCertStore::empty();
    let certificates = CertificateDer::pem_file_iter(ca_path)?.collect::<Result<Vec<_>, _>>()?;
    for certificate in certificates {
        roots.add(certificate)?;
    }
    let mut client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(Arc::new(client_config));

    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let connect = format!(
        "CONNECT localhost:{upstream_port} HTTP/1.1\r\nHost: localhost:{upstream_port}\r\n\r\n"
    );
    client.write_all(connect.as_bytes()).await?;
    let response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    if !response.starts_with(b"HTTP/1.1 200 OK") {
        return Err("CONNECT did not succeed".into());
    }
    let server_name = ServerName::try_from("localhost".to_owned())?;
    Ok(timeout(TEST_TIMEOUT, connector.connect(server_name, client)).await??)
}

fn test_tls_server() -> Result<(TlsAcceptor, String), TestError> {
    let root_key = KeyPair::generate()?;
    let mut root_params = CertificateParams::default();
    let now = time::OffsetDateTime::now_utc();
    root_params.not_before = now - time::Duration::days(1);
    root_params.not_after = now + time::Duration::days(30);
    root_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let mut root_name = DistinguishedName::new();
    root_name.push(DnType::CommonName, "HTTP Inspector Test Root");
    root_params.distinguished_name = root_name;
    let root_certificate = root_params.self_signed(&root_key)?;
    let root_pem = root_certificate.pem();
    let issuer = Issuer::new(root_params, root_key);

    let server_key = KeyPair::generate()?;
    let mut server_params = CertificateParams::new(vec!["localhost".to_owned()])?;
    server_params.not_before = now - time::Duration::days(1);
    server_params.not_after = now + time::Duration::days(7);
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let server_certificate = server_params.signed_by(&server_key, &issuer)?;
    let private_key = PrivatePkcs8KeyDer::from(server_key.serialize_der()).into();
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![server_certificate.der().clone()], private_key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok((TlsAcceptor::from(Arc::new(config)), root_pem))
}

fn header_value<'a>(headers: &'a HeaderPreview, name: &str) -> Option<&'a str> {
    headers
        .entries()
        .iter()
        .find(|header| header.name() == name)
        .map(CapturedHeader::value)
}

#[tokio::test]
async fn replays_edited_headers_and_cookie_with_link_and_redaction() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let request = read_headers(&mut stream).await?;
        let _send_result = request_sender.send(request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_max_connections(1))?;
    assert_eq!(proxy.state(), EngineState::Running);
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let mut request = format!(
        "GET http://{upstream_addr}/hello?name=rust HTTP/1.1\r\nHost: {upstream_addr}\r\nX-File-Name: 한글.txt\r\nX-Raw: "
    )
    .into_bytes();
    request.extend_from_slice(b"\xff\r\nConnection: close\r\n\r\n");
    client.write_all(&request).await?;

    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    let response = String::from_utf8(response)?;
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.ends_with("hello"));

    let upstream_request = timeout(TEST_TIMEOUT, request_receiver).await??;
    let upstream_request = String::from_utf8_lossy(&upstream_request);
    assert!(upstream_request.starts_with("GET /hello?name=rust HTTP/1.1\r\n"));

    let record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?;
    assert_eq!(record.request().method(), http::Method::GET);
    assert!(matches!(
        settled_within(&record, TEST_TIMEOUT).await?,
        Outcome::Response(response) if response.status() == 200
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    assert_eq!(proxy.state(), EngineState::Running);

    let mut draft = ReplayDraft::new(record.clone())?;
    disable_unreproducible_header(&mut draft, proxy.local_addr())?;
    add_edited_headers(&mut draft);
    let replay_server = TcpListener::bind("127.0.0.1:0").await?;
    draft.url = format!("http://{}/edited", replay_server.local_addr()?);
    let task = proxy.replay(&draft)?;
    let (mut replay_stream, _) = timeout(TEST_TIMEOUT, replay_server.accept()).await??;
    let replay_headers = String::from_utf8(read_headers(&mut replay_stream).await?)?;
    assert!(replay_headers.contains("x-edited: changed"));
    assert!(replay_headers.contains("x-file-name: 한글.txt"));
    assert!(!replay_headers.contains("x-raw"));
    assert!(replay_headers.contains("cookie: session=changed"));
    assert!(replay_headers.contains("content-length: 0"));
    replay_stream
        .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n")
        .await?;
    let replay_record = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("missing replay")?;
    assert_eq!(replay_record.replay_of(), Some(record.id()));
    assert_ne!(replay_record.id(), record.id());
    assert!(
        replay_record
            .request()
            .headers()
            .entries()
            .iter()
            .any(|field| field.name() == "cookie" && field.value() == "[REDACTED]")
    );
    assert!(
        matches!(settled_within(&replay_record, TEST_TIMEOUT).await?, Outcome::Response(response) if response.status() == 201)
    );
    assert!(record.replay_of().is_none());
    assert_replay_validation(&mut draft, proxy.local_addr());
    assert_body_file_loading(&mut draft, proxy.local_addr())?;
    task.cancel();
    proxy.stop()?;
    Ok(())
}

fn add_edited_headers(draft: &mut ReplayDraft) {
    draft.headers.push(ReplayField {
        enabled: true,
        name: "x-edited".to_owned(),
        value: "changed".to_owned(),
    });
    draft.headers.push(ReplayField {
        enabled: true,
        name: "cookie".to_owned(),
        value: "session=changed".to_owned(),
    });
}

/// UTF-8 values survive as captured; lossy non-UTF-8 values block sending until disabled.
fn disable_unreproducible_header(
    draft: &mut ReplayDraft,
    address: std::net::SocketAddr,
) -> Result<(), TestError> {
    assert!(draft.headers.iter().any(|field| {
        field.name.eq_ignore_ascii_case("x-file-name") && field.value == "한글.txt"
    }));
    let raw_error = draft
        .request(address)
        .err()
        .ok_or("non-UTF-8 header must block replay")?;
    assert!(raw_error.to_string().contains("x-raw"), "{raw_error}");
    for field in &mut draft.headers {
        if field.name.eq_ignore_ascii_case("x-raw") {
            field.enabled = false;
        }
    }
    Ok(())
}

/// Complete files replace the body up to the limit; larger files and truncated URLs are explained.
fn assert_body_file_loading(
    draft: &mut ReplayDraft,
    address: std::net::SocketAddr,
) -> Result<(), TestError> {
    let directory = tempfile::tempdir()?;
    let body_path = directory.path().join("body.json");
    std::fs::write(&body_path, br#"{"edited":true}"#)?;
    draft.body_missing = true;
    draft.load_body_file(&body_path)?;
    assert_eq!(draft.body.as_ref(), br#"{"edited":true}"#);
    assert!(!draft.body_missing);

    let large_path = directory.path().join("large.bin");
    std::fs::File::create(&large_path)?.set_len(REPLAY_BODY_FILE_LIMIT_BYTES + 1)?;
    let error = draft
        .load_body_file(&large_path)
        .err()
        .ok_or("oversized body file must be refused")?;
    assert!(error.to_string().contains("limited"), "{error}");
    assert_eq!(draft.body.as_ref(), br#"{"edited":true}"#);

    let url = std::mem::replace(&mut draft.url, "http://example.test/a b…".to_owned());
    let error = draft
        .request(address)
        .err()
        .ok_or("truncated URL must be refused")?;
    assert!(error.to_string().contains("truncated"), "{error}");
    draft.url = url;
    Ok(())
}

fn assert_replay_validation(draft: &mut ReplayDraft, address: std::net::SocketAddr) {
    draft.headers.push(ReplayField {
        enabled: true,
        name: "bad name".to_owned(),
        value: "x".to_owned(),
    });
    assert!(draft.request(address).is_err());
    draft.headers.pop();
    draft.url = format!("http://{address}/");
    assert!(draft.request(address).is_err());
    draft.url = "http://example.test/".to_owned();
    draft.body = "한글".as_bytes().to_vec().into();
    let framed = draft.request(address);
    assert!(
        matches!(framed, Ok(request) if request.headers().get("content-length").is_some_and(|value| value == "6"))
    );
    assert!(!draft.needs_confirmation());
    draft.method = "POST".to_owned();
    assert!(draft.needs_confirmation());
    draft.body_missing = true;
    assert!(draft.request(address).is_err());
    draft.body_missing = false;
    draft.headers.push(ReplayField {
        enabled: true,
        name: "authorization".to_owned(),
        value: String::new(),
    });
    assert!(draft.request(address).is_err());
}
