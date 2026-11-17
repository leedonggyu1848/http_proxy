use std::error::Error;
use std::time::Duration;

use http_proxy::{
    BodyLifecycle, BodyRetention, CaptureOutcome, HeaderPreview, ProxyConfig, ProxyHandle,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

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

fn header_value<'a>(headers: &'a HeaderPreview, name: &str) -> Option<&'a str> {
    headers
        .entries()
        .iter()
        .find(|header| header.name() == name)
        .map(http_proxy::CapturedHeader::value)
}

async fn read_headers(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
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
