use std::error::Error;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use http_proxy::{
    BodyLifecycle, BodyRetention, CaptureOutcome, HeaderPreview, HttpsInspection, ProxyConfig,
    ProxyHandle,
};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn forwards_http_request_and_records_response() -> Result<(), Box<dyn Error>> {
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
    assert_eq!(record.method(), http::Method::GET);
    assert!(matches!(
        record.outcome(),
        CaptureOutcome::Response { status: 200, .. }
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn connect_creates_a_bidirectional_tunnel() -> Result<(), Box<dyn Error>> {
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
    assert_eq!(record.method(), http::Method::CONNECT);
    assert!(matches!(
        record.outcome(),
        CaptureOutcome::TunnelEstablished
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn captures_bounded_headers_and_body_previews() -> Result<(), Box<dyn Error>> {
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
        header_value(record.request_headers(), "x-request"),
        Some("yes")
    );
    assert_eq!(
        header_value(record.request_headers(), "authorization"),
        Some("[REDACTED]")
    );

    let request_body = record
        .request_body()
        .ok_or("request body preview missing")?
        .snapshot()
        .ok_or("request body preview busy")?;
    assert_eq!(request_body.observed_bytes(), BODY_SIZE as u64);
    assert_eq!(request_body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(request_body.retention(), BodyRetention::Truncated);
    assert!(request_body.bytes().iter().all(|byte| *byte == b'q'));

    let CaptureOutcome::Response {
        status,
        headers,
        body,
    } = record.outcome()
    else {
        return Err("expected an HTTP response capture".into());
    };
    assert_eq!(*status, 201);
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
async fn decrypts_https_with_a_trusted_local_ca() -> Result<(), Box<dyn Error>> {
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
        "CONNECT localhost:{} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
        upstream_addr.port(),
        upstream_addr.port()
    );
    client.write_all(connect.as_bytes()).await?;
    let connect_response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    assert!(connect_response.starts_with(b"HTTP/1.1 200 OK"));

    let server_name = ServerName::try_from("localhost".to_owned())?;
    let mut tls = timeout(TEST_TIMEOUT, connector.connect(server_name, client)).await??;
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
        setup.outcome(),
        CaptureOutcome::InspectionEstablished
    ));
    let exchange = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("decrypted exchange capture missing")?;
    assert_eq!(
        exchange.target(),
        format!("https://localhost:{}/secure", upstream_addr.port())
    );
    assert_eq!(
        header_value(exchange.request_headers(), "authorization"),
        Some("[REDACTED]")
    );
    let CaptureOutcome::Response {
        status,
        headers,
        body,
    } = exchange.outcome()
    else {
        return Err("expected a decrypted HTTP response capture".into());
    };
    assert_eq!(*status, 200);
    assert_eq!(header_value(headers, "x-secure"), Some("yes"));
    let body = body.snapshot().ok_or("response preview busy")?;
    assert_eq!(body.bytes(), b"secret");
    assert_eq!(body.lifecycle(), BodyLifecycle::Complete);

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn rejects_an_untrusted_upstream_certificate() -> Result<(), Box<dyn Error>> {
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
        setup.outcome(),
        CaptureOutcome::InspectionEstablished
    ));
    let exchange = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("upstream TLS failure capture missing")?;
    assert!(matches!(
        exchange.outcome(),
        CaptureOutcome::Failed {
            stage: http_proxy::FailureStage::UpstreamTls,
            ..
        }
    ));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn rejects_a_host_that_differs_from_connect() -> Result<(), Box<dyn Error>> {
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
        setup.outcome(),
        CaptureOutcome::InspectionEstablished
    ));
    let rejected = timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("rejected request capture missing")?;
    assert!(matches!(
        rejected.outcome(),
        CaptureOutcome::Failed {
            stage: http_proxy::FailureStage::Request,
            ..
        }
    ));

    proxy.stop()?;
    Ok(())
}

async fn connect_to_inspection(
    proxy: &ProxyHandle,
    upstream_port: u16,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Box<dyn Error>> {
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

fn test_tls_server() -> Result<(TlsAcceptor, String), Box<dyn Error>> {
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
        .map(http_proxy::CapturedHeader::value)
}

async fn read_headers<S>(stream: &mut S) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    let mut byte = [0_u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        data.push(byte[0]);
        if data.len() > 16 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "headers exceeded the test limit",
            ));
        }
    }
    Ok(data)
}
