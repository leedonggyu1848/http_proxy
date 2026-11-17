use std::error::Error;
use std::time::Duration;

use http_proxy::{CaptureOutcome, ProxyConfig, ProxyHandle};
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
    assert_eq!(record.outcome(), &CaptureOutcome::Response { status: 200 });

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
    assert_eq!(record.outcome(), &CaptureOutcome::TunnelEstablished);

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
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
