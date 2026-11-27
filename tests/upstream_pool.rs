//! Integration tests for upstream HTTP/1 connection pooling across plain HTTP
//! and inspected HTTPS paths.

use std::fs;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http_inspector_core::https::HttpsInspection;
use http_inspector_core::interception::{ProxyConfig, ProxyHandle};
use http_inspector_exchange::{FailureStage, Outcome};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Barrier, Notify};
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub mod common;

use common::{TEST_TIMEOUT, TestError, next_record, read_headers, settled, wait_for_body_end};

async fn read_one_http_response<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let headers = read_headers(stream).await?;
    let header_text = String::from_utf8_lossy(&headers);
    let mut content_length = 0_usize;
    for line in header_text.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0_u8; content_length];
    stream.read_exact(&mut body).await?;
    Ok((headers, body))
}

async fn read_optional_headers<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<Option<Vec<u8>>> {
    let mut data = Vec::new();
    while !data.ends_with(b"\r\n\r\n") {
        let mut byte = [0_u8; 1];
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Ok(None);
        }
        data.extend_from_slice(&byte);
    }
    Ok(Some(data))
}

async fn read_optional_request_with_body<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let Some(headers) = read_optional_headers(stream).await? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&headers);
    let mut content_length = 0_usize;
    for line in text.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0_u8; content_length];
    stream.read_exact(&mut body).await?;
    Ok(Some((headers, body)))
}

fn test_tls_server(sans: Vec<String>) -> Result<(TlsAcceptor, String), TestError> {
    let root_key = KeyPair::generate()?;
    let mut root_params = CertificateParams::default();
    let now = time::OffsetDateTime::now_utc();
    root_params.not_before = now - time::Duration::days(1);
    root_params.not_after = now + time::Duration::days(30);
    root_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let mut root_name = DistinguishedName::new();
    root_name.push(DnType::CommonName, "HTTP Inspector Pool Test Root");
    root_params.distinguished_name = root_name;
    let root_certificate = root_params.self_signed(&root_key)?;
    let root_pem = root_certificate.pem();
    let issuer = Issuer::new(root_params, root_key);

    let server_key = KeyPair::generate()?;
    let mut server_params = CertificateParams::new(sans)?;
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

async fn connect_inspected(
    proxy: &ProxyHandle,
    host: &str,
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
    let connect =
        format!("CONNECT {host}:{upstream_port} HTTP/1.1\r\nHost: {host}:{upstream_port}\r\n\r\n");
    client.write_all(connect.as_bytes()).await?;
    let response = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    if !response.starts_with(b"HTTP/1.1 200 OK") {
        return Err("CONNECT did not succeed".into());
    }
    let server_name = ServerName::try_from(host.to_owned())?;
    Ok(timeout(TEST_TIMEOUT, connector.connect(server_name, client)).await??)
}

#[tokio::test]
async fn reuses_upstream_connection_for_plain_http_requests() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_counter = Arc::clone(&accepted);
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = upstream.accept().await else {
                break;
            };
            accepted_counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut count = 0_usize;
                while read_optional_headers(&mut stream)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    count += 1;
                    let body = format!("reply-{count}");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if stream.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;

    // Send two sequential requests over a keep-alive client connection.
    let first_req =
        format!("GET http://{upstream_addr}/one HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n");
    client.write_all(first_req.as_bytes()).await?;
    let (h1, b1) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert!(h1.starts_with(b"HTTP/1.1 200 OK"));
    assert_eq!(b1, b"reply-1");

    let second_req = format!(
        "GET http://{upstream_addr}/two HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(second_req.as_bytes()).await?;
    let (h2, b2) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert!(h2.starts_with(b"HTTP/1.1 200 OK"));
    assert_eq!(b2, b"reply-2");

    let r1 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    assert!(matches!(settled(&r1).await?, Outcome::Response(_)));
    assert!(matches!(settled(&r2).await?, Outcome::Response(_)));

    let c1 = r1.upstream_connection().ok_or("missing connection on r1")?;
    let c2 = r2.upstream_connection().ok_or("missing connection on r2")?;
    assert!(!c1.is_reused());
    assert!(c2.is_reused());
    assert_eq!(c1.id(), c2.id());
    assert!(r1.timing().connect().is_some());
    assert_eq!(r2.timing().connect(), None);
    assert_eq!(r2.timing().connect_span(), None);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);

    server.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn reuses_upstream_connection_for_inspected_https_requests() -> Result<(), TestError> {
    let temporary = tempfile::tempdir()?;
    let upstream_root_path = temporary.path().join("upstream-root.pem");
    let (upstream_acceptor, upstream_root) = test_tls_server(vec!["localhost".to_owned()])?;
    fs::write(&upstream_root_path, upstream_root)?;

    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let tls_handshakes = Arc::new(AtomicUsize::new(0));
    let handshake_counter = Arc::clone(&tls_handshakes);
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = upstream.accept().await else {
                break;
            };
            let acceptor = upstream_acceptor.clone();
            let counter = Arc::clone(&handshake_counter);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut count = 0_usize;
                while read_optional_headers(&mut tls)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    count += 1;
                    let body = format!("tls-{count}");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if tls.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let inspection = HttpsInspection::new(temporary.path().join("proxy-ca"))
        .with_additional_upstream_root(upstream_root_path);
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_https_inspection(inspection))?;

    let mut tls = connect_inspected(&proxy, "localhost", upstream_addr.port()).await?;
    let req1 = format!(
        "GET /first HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
        upstream_addr.port()
    );
    tls.write_all(req1.as_bytes()).await?;
    let (_, b1) = timeout(TEST_TIMEOUT, read_one_http_response(&mut tls)).await??;
    assert_eq!(b1, b"tls-1");

    let req2 = format!(
        "GET /second HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
        upstream_addr.port()
    );
    tls.write_all(req2.as_bytes()).await?;
    let (_, b2) = timeout(TEST_TIMEOUT, read_one_http_response(&mut tls)).await??;
    assert_eq!(b2, b"tls-2");

    let setup = next_record(&mut proxy).await?;
    assert!(matches!(
        settled(&setup).await?,
        Outcome::InspectionEstablished
    ));
    let r1 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    assert!(matches!(settled(&r1).await?, Outcome::Response(_)));
    assert!(matches!(settled(&r2).await?, Outcome::Response(_)));

    let c1 = r1.upstream_connection().ok_or("missing connection on r1")?;
    let c2 = r2.upstream_connection().ok_or("missing connection on r2")?;
    assert!(!c1.is_reused());
    assert!(c2.is_reused());
    assert_eq!(c1.id(), c2.id());
    assert!(r1.timing().connect().is_some());
    assert_eq!(r2.timing().connect(), None);
    assert_eq!(r2.timing().connect_span(), None);
    assert_eq!(tls_handshakes.load(Ordering::SeqCst), 1);

    server.abort();
    proxy.stop()?;
    Ok(())
}

fn spawn_flaky_idempotent_server(
    upstream: TcpListener,
    accepted: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<io::Result<()>> {
    tokio::spawn(async move {
        // Connection #1: serves warmup GET, then closes without replying as soon as
        // a second request starts arriving (simulating an idle keep-alive close race).
        let (mut first, _) = upstream.accept().await?;
        accepted.fetch_add(1, Ordering::SeqCst);
        let _warmup = read_optional_request_with_body(&mut first).await?;
        first
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nwarm")
            .await?;
        let _dropped = read_optional_request_with_body(&mut first).await?;
        drop(first);

        // Connection #2: serves retried GET, then drops on the next request (PUT).
        let (mut second, _) = upstream.accept().await?;
        accepted.fetch_add(1, Ordering::SeqCst);
        let _retried_get = read_optional_request_with_body(&mut second).await?;
        second
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\nretried")
            .await?;
        let _dropped_put = read_optional_request_with_body(&mut second).await?;
        drop(second);

        // Connection #3: serves retried PUT and echoes its body back.
        let (mut third, _) = upstream.accept().await?;
        accepted.fetch_add(1, Ordering::SeqCst);
        let Some((_, put_body)) = read_optional_request_with_body(&mut third).await? else {
            return Err(io::Error::other("missing retried PUT"));
        };
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            put_body.len()
        );
        third.write_all(reply.as_bytes()).await?;
        third.write_all(&put_body).await?;
        Ok(())
    })
}

#[tokio::test]
async fn retries_idempotent_requests_when_upstream_closes_idle_connection() -> Result<(), TestError>
{
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let accepted = Arc::new(AtomicUsize::new(0));
    let server = spawn_flaky_idempotent_server(upstream, Arc::clone(&accepted));

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;

    // 1. Warmup GET puts Connection #1 into the pool.
    client
        .write_all(
            format!("GET http://{upstream_addr}/warm HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let (_, b1) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert_eq!(b1, b"warm");

    // 2. GET hits Connection #1 which closes without answering -> retried on Connection #2.
    client
        .write_all(
            format!("GET http://{upstream_addr}/retry HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let (h2, b2) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert!(h2.starts_with(b"HTTP/1.1 200 OK"));
    assert_eq!(b2, b"retried");

    // 3. PUT with a small body hits Connection #2 which closes -> retried on Connection #3.
    client
        .write_all(
            format!(
                "PUT http://{upstream_addr}/item HTTP/1.1\r\nHost: {upstream_addr}\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload"
            )
            .as_bytes(),
        )
        .await?;
    let (h3, b3) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert!(h3.starts_with(b"HTTP/1.1 200 OK"));
    assert_eq!(b3, b"payload");

    let r1 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    let r3 = next_record(&mut proxy).await?;
    assert!(matches!(settled(&r1).await?, Outcome::Response(_)));
    assert!(matches!(settled(&r2).await?, Outcome::Response(_)));
    assert!(matches!(settled(&r3).await?, Outcome::Response(_)));

    let c1 = r1.upstream_connection().ok_or("missing c1")?;
    let c2 = r2.upstream_connection().ok_or("missing c2")?;
    let c3 = r3.upstream_connection().ok_or("missing c3")?;
    assert!(!c1.is_reused());
    assert!(
        !c2.is_reused(),
        "retried GET completed on a fresh connection"
    );
    assert_ne!(c1.id(), c2.id());
    assert!(r2.timing().connect().is_some());
    assert!(
        !c3.is_reused(),
        "retried PUT completed on a fresh connection"
    );
    assert_ne!(c2.id(), c3.id());
    assert!(r3.timing().connect().is_some());

    // The retried PUT body must be observed exactly once (7 bytes, not 14).
    let put_body = wait_for_body_end(r3.request().body().ok_or("missing PUT body")?).await?;
    assert_eq!(put_body.observed_bytes(), 7);
    assert_eq!(put_body.bytes(), b"payload");

    timeout(TEST_TIMEOUT, server).await???;
    assert_eq!(accepted.load(Ordering::SeqCst), 3);
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn does_not_retry_non_idempotent_post_when_idle_connection_closes() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let accepted = Arc::new(AtomicUsize::new(0));
    let post_attempts = Arc::new(AtomicUsize::new(0));
    let accepted_counter = Arc::clone(&accepted);
    let post_counter = Arc::clone(&post_attempts);

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = upstream.accept().await else {
                break;
            };
            let conn_num = accepted_counter.fetch_add(1, Ordering::SeqCst) + 1;
            let posts = Arc::clone(&post_counter);
            tokio::spawn(async move {
                if conn_num == 1 {
                    let _warmup = read_optional_request_with_body(&mut stream).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await;
                    // When the next request (POST) arrives on the pooled socket,
                    // close without replying. A retry would open connection #2.
                    if let Ok(Some((head, _))) = read_optional_request_with_body(&mut stream).await
                        && head.starts_with(b"POST ")
                    {
                        posts.fetch_add(1, Ordering::SeqCst);
                    }
                    drop(stream);
                } else {
                    if let Ok(Some((head, _))) = read_optional_request_with_body(&mut stream).await
                        && head.starts_with(b"POST ")
                    {
                        posts.fetch_add(1, Ordering::SeqCst);
                    }
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nduplicate")
                        .await;
                }
            });
        }
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;

    // 1. Warmup GET establishes Connection #1 in the pool.
    client
        .write_all(
            format!("GET http://{upstream_addr}/warm HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let (_, b1) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert_eq!(b1, b"ok");

    // 2. Non-idempotent POST over the pooled connection must NOT be retried when it closes.
    client
        .write_all(
            format!(
                "POST http://{upstream_addr}/charge HTTP/1.1\r\nHost: {upstream_addr}\r\nContent-Length: 10\r\nConnection: close\r\n\r\namount=100"
            )
            .as_bytes(),
        )
        .await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    assert!(
        response.starts_with(b"HTTP/1.1 502 Bad Gateway"),
        "{}",
        String::from_utf8_lossy(&response)
    );

    let r1 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    assert!(matches!(settled(&r1).await?, Outcome::Response(_)));
    assert!(matches!(
        settled(&r2).await?,
        Outcome::Failed {
            stage: FailureStage::Response,
            ..
        }
    ));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(post_attempts.load(Ordering::SeqCst), 1);

    server.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn does_not_share_connections_across_different_hosts() -> Result<(), TestError> {
    let upstream_a = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_b = TcpListener::bind("127.0.0.1:0").await?;
    let addr_a = upstream_a.local_addr()?;
    let addr_b = upstream_b.local_addr()?;

    let spawn_keep_alive = |listener: TcpListener| {
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    while read_optional_headers(&mut stream)
                        .await
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        if stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        })
    };
    let server_a = spawn_keep_alive(upstream_a);
    let server_b = spawn_keep_alive(upstream_b);

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;

    for target in [
        format!("{addr_a}"),
        format!("{addr_b}"),
        format!("localhost:{}", addr_a.port()),
    ] {
        let req = format!("GET http://{target}/ HTTP/1.1\r\nHost: {target}\r\n\r\n");
        client.write_all(req.as_bytes()).await?;
        let (head, body) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
        assert!(head.starts_with(b"HTTP/1.1 200 OK"));
        assert_eq!(body, b"ok");
    }

    let r_a = next_record(&mut proxy).await?;
    let r_b = next_record(&mut proxy).await?;
    let r_localhost = next_record(&mut proxy).await?;
    let c_a = r_a.upstream_connection().ok_or("missing c_a")?;
    let c_b = r_b.upstream_connection().ok_or("missing c_b")?;
    let c_localhost = r_localhost
        .upstream_connection()
        .ok_or("missing c_localhost")?;
    assert!(!c_a.is_reused());
    assert!(!c_b.is_reused());
    assert!(!c_localhost.is_reused());
    assert_ne!(c_a.id(), c_b.id());
    assert_ne!(c_a.id(), c_localhost.id());
    assert_ne!(c_b.id(), c_localhost.id());

    server_a.abort();
    server_b.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn enforces_max_idle_per_host_and_closes_on_stop() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let both_arrived = Arc::new(Barrier::new(2));
    let closed_count = Arc::new(AtomicUsize::new(0));
    let closed_notify = Arc::new(Notify::new());
    let server_closed = Arc::clone(&closed_count);
    let server_notify = Arc::clone(&closed_notify);
    let server_barrier = Arc::clone(&both_arrived);

    let server = tokio::spawn(async move {
        let mut index = 0_usize;
        loop {
            let Ok((mut stream, _)) = upstream.accept().await else {
                break;
            };
            index += 1;
            let current = index;
            let barrier = Arc::clone(&server_barrier);
            let closed = Arc::clone(&server_closed);
            let notify = Arc::clone(&server_notify);
            tokio::spawn(async move {
                while read_optional_headers(&mut stream)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    if current <= 2 {
                        barrier.wait().await;
                    }
                    if stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                closed.fetch_add(1, Ordering::SeqCst);
                notify.notify_waiters();
            });
        }
    });

    let mut proxy = ProxyHandle::start(
        ProxyConfig::loopback(0)
            .with_max_idle_per_host(1)
            .with_idle_timeout(Duration::from_secs(30)),
    )?;
    let proxy_addr = proxy.local_addr();
    let send_one = |path: &'static str| async move {
        let mut client = TcpStream::connect(proxy_addr).await?;
        let req = format!(
            "GET http://{upstream_addr}{path} HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(req.as_bytes()).await?;
        let (head, body) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
        assert!(head.starts_with(b"HTTP/1.1 200 OK"));
        assert_eq!(body, b"ok");
        Ok::<_, TestError>(())
    };
    tokio::try_join!(send_one("/a"), send_one("/b"))?;
    let _r1 = next_record(&mut proxy).await?;
    let _r2 = next_record(&mut proxy).await?;

    // One of the two connections must be evicted immediately because max_idle_per_host == 1.
    timeout(Duration::from_secs(2), async {
        while closed_count.load(Ordering::SeqCst) < 1 {
            closed_notify.notified().await;
        }
    })
    .await?;
    assert_eq!(closed_count.load(Ordering::SeqCst), 1);

    // Stopping the proxy must close the remaining pooled idle connection.
    proxy.stop()?;
    timeout(Duration::from_secs(2), async {
        while closed_count.load(Ordering::SeqCst) < 2 {
            closed_notify.notified().await;
        }
    })
    .await?;
    assert_eq!(closed_count.load(Ordering::SeqCst), 2);
    server.abort();
    Ok(())
}

#[tokio::test]
async fn evicts_idle_connection_after_idle_timeout() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let expired_closed = Arc::new(AtomicUsize::new(0));
    let expired_notify = Arc::new(Notify::new());
    let expired_closed_ref = Arc::clone(&expired_closed);
    let expired_notify_ref = Arc::clone(&expired_notify);
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = upstream.accept().await else {
                break;
            };
            let closed = Arc::clone(&expired_closed_ref);
            let notify = Arc::clone(&expired_notify_ref);
            tokio::spawn(async move {
                while read_optional_headers(&mut stream)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    if stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                closed.fetch_add(1, Ordering::SeqCst);
                notify.notify_waiters();
            });
        }
    });

    let mut proxy =
        ProxyHandle::start(ProxyConfig::loopback(0).with_idle_timeout(Duration::from_millis(80)))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client
        .write_all(
            format!("GET http://{upstream_addr}/1 HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let _ = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    let first = next_record(&mut proxy).await?;
    let first_conn = first.upstream_connection().ok_or("missing first conn")?;

    // Wait for the idle reaper to close the expired connection without sending another request.
    timeout(Duration::from_secs(2), async {
        while expired_closed.load(Ordering::SeqCst) < 1 {
            expired_notify.notified().await;
        }
    })
    .await?;

    // Sending another request opens a fresh connection.
    client
        .write_all(
            format!(
                "GET http://{upstream_addr}/2 HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let _ = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    let second = next_record(&mut proxy).await?;
    let second_conn = second.upstream_connection().ok_or("missing second conn")?;
    assert!(!second_conn.is_reused());
    assert_ne!(first_conn.id(), second_conn.id());
    assert!(second.timing().connect().is_some());

    server.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn does_not_share_inspected_https_across_different_connect_destinations()
-> Result<(), TestError> {
    let temporary = tempfile::tempdir()?;
    let upstream_root_path = temporary.path().join("upstream-root.pem");
    let (upstream_acceptor, upstream_root) =
        test_tls_server(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])?;
    fs::write(&upstream_root_path, upstream_root)?;

    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let handshake_counter = Arc::clone(&handshakes);
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = upstream.accept().await else {
                break;
            };
            let acceptor = upstream_acceptor.clone();
            let counter = Arc::clone(&handshake_counter);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                while read_optional_headers(&mut tls)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
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

    let inspection = HttpsInspection::new(temporary.path().join("proxy-ca"))
        .with_additional_upstream_root(upstream_root_path);
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_https_inspection(inspection))?;

    for host in ["localhost", "127.0.0.1"] {
        let mut tls = connect_inspected(&proxy, host, port).await?;
        let req = format!("GET / HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
        tls.write_all(req.as_bytes()).await?;
        let (head, body) = timeout(TEST_TIMEOUT, read_one_http_response(&mut tls)).await??;
        assert!(head.starts_with(b"HTTP/1.1 200 OK"));
        assert_eq!(body, b"ok");
    }

    let _setup1 = next_record(&mut proxy).await?;
    let r1 = next_record(&mut proxy).await?;
    let _setup2 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    let c1 = r1.upstream_connection().ok_or("missing c1")?;
    let c2 = r2.upstream_connection().ok_or("missing c2")?;
    assert!(!c1.is_reused());
    assert!(!c2.is_reused());
    assert_ne!(c1.id(), c2.id());
    assert_eq!(handshakes.load(Ordering::SeqCst), 2);

    server.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn discards_already_closed_idle_connection_before_sending_post() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let post_count = Arc::new(AtomicUsize::new(0));
    let post_counter = Arc::clone(&post_count);

    let server = tokio::spawn(async move {
        // Connection #1 serves warmup GET, then closes immediately while idle.
        let (mut first, _) = upstream.accept().await?;
        let _warmup = read_optional_request_with_body(&mut first).await?;
        first
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        first.shutdown().await?;
        drop(first);

        // Connection #2 serves the POST once.
        let (mut second, _) = upstream.accept().await?;
        let Some((head, body)) = read_optional_request_with_body(&mut second).await? else {
            return Err(io::Error::other("missing POST"));
        };
        assert!(head.starts_with(b"POST /once "));
        assert_eq!(body, b"data");
        post_counter.fetch_add(1, Ordering::SeqCst);
        second
            .write_all(
                b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\nConnection: close\r\n\r\ncreated",
            )
            .await?;
        Ok::<_, io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client
        .write_all(
            format!("GET http://{upstream_addr}/warm HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let _ = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    // Allow TCP FIN from Connection #1 to be processed while idle.
    tokio::time::sleep(Duration::from_millis(30)).await;

    client
        .write_all(
            format!(
                "POST http://{upstream_addr}/once HTTP/1.1\r\nHost: {upstream_addr}\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata"
            )
            .as_bytes(),
        )
        .await?;
    let (head, body) = timeout(TEST_TIMEOUT, read_one_http_response(&mut client)).await??;
    assert!(head.starts_with(b"HTTP/1.1 201 Created"));
    assert_eq!(body, b"created");

    let r1 = next_record(&mut proxy).await?;
    let r2 = next_record(&mut proxy).await?;
    assert!(matches!(settled(&r1).await?, Outcome::Response(_)));
    assert!(matches!(settled(&r2).await?, Outcome::Response(_)));
    let c1 = r1.upstream_connection().ok_or("missing c1")?;
    let c2 = r2.upstream_connection().ok_or("missing c2")?;
    assert!(!c2.is_reused());
    assert_ne!(c1.id(), c2.id());
    assert_eq!(post_count.load(Ordering::SeqCst), 1);

    timeout(TEST_TIMEOUT, server).await???;
    proxy.stop()?;
    Ok(())
}
