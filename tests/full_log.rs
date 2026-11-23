use std::fmt::Write as _;
use std::io;
use std::path::Path;

use http_proxy::{FullLog, ProxyConfig, ProxyHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{TEST_TIMEOUT, TestError, read_headers_within};

/// Larger than [`common::HEADER_LIMIT`], as this test always allowed.
const LARGE_HEADER_LIMIT: usize = 32_768;
const BODY_SIZE: usize = 20_000;

/// Saves the log like the UI does: a busy queue is retried, never treated as
/// a lost record.
fn export(log: &FullLog, path: &Path) -> Result<String, TestError> {
    let saved = loop {
        match log.export(path.to_path_buf()) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => std::thread::yield_now(),
            result => break result?,
        }
    };
    saved.recv_timeout(TEST_TIMEOUT)??;
    Ok(std::fs::read_to_string(path)?)
}

fn escaped_body(text: &str, side: &str) -> String {
    let prefix = format!("] {side} body bytes: ");
    text.lines()
        .filter_map(|line| line.split_once(&prefix).map(|(_, bytes)| bytes))
        .collect()
}

async fn serve_large_exchange(
    upstream: TcpListener,
    expected_request: Vec<u8>,
    response_body: Vec<u8>,
    release: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    let (mut stream, _) = upstream.accept().await?;
    let headers = read_headers_within(&mut stream, LARGE_HEADER_LIMIT).await?;
    assert!(headers.starts_with(b"POST /large HTTP/1.1\r\n"));
    let mut body = vec![0; BODY_SIZE];
    stream.read_exact(&mut body).await?;
    assert_eq!(body, expected_request);
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {BODY_SIZE}\r\nSet-Cookie: response-secret\r\nConnection: close\r\n"
    );
    for index in 0..80 {
        writeln!(response, "X-Response-{index}: value-{index}\r").map_err(io::Error::other)?;
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await?;
    let (prefix, suffix) = response_body
        .split_at_checked(BODY_SIZE / 2)
        .ok_or_else(|| io::Error::other("Test response body is shorter than expected"))?;
    stream.write_all(prefix).await?;
    release.await.map_err(io::Error::other)?;
    stream.write_all(suffix).await?;
    Ok(())
}

#[tokio::test]
async fn exports_full_bodies_headers_and_a_streaming_snapshot() -> Result<(), TestError> {
    let directory = tempfile::tempdir()?;
    let log = FullLog::new()?;
    let request_body: Vec<u8> = (0..=255).cycle().take(BODY_SIZE).collect();
    let response_body: Vec<u8> = (0..=255).rev().cycle().take(BODY_SIZE).collect();
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(serve_large_exchange(
        upstream,
        request_body.clone(),
        response_body.clone(),
        release_receiver,
    ));
    let (expected_prefix, expected_suffix) = response_body
        .split_at_checked(BODY_SIZE / 2)
        .ok_or("Test response body is shorter than expected")?;
    let proxy = ProxyHandle::start_recording(ProxyConfig::loopback(0), log.clone())?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let mut request = format!(
        "POST http://{address}/large HTTP/1.1\r\nHost: {address}\r\nContent-Length: {BODY_SIZE}\r\nAuthorization: request-secret\r\nConnection: close\r\n"
    );
    for index in 0..80 {
        writeln!(request, "X-Request-{index}: value-{index}\r")?;
    }
    request.push_str("\r\n");
    client.write_all(request.as_bytes()).await?;
    client.write_all(&request_body).await?;
    let response_headers = timeout(
        TEST_TIMEOUT,
        read_headers_within(&mut client, LARGE_HEADER_LIMIT),
    )
    .await??;
    assert!(response_headers.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let mut prefix = vec![0; BODY_SIZE / 2];
    timeout(TEST_TIMEOUT, client.read_exact(&mut prefix)).await??;
    assert_eq!(prefix, expected_prefix);

    let snapshot = export(&log, &directory.path().join("streaming.log"))?;
    assert_eq!(
        escaped_body(&snapshot, "Request"),
        request_body.escape_ascii().to_string()
    );
    assert_eq!(
        escaped_body(&snapshot, "Response"),
        expected_prefix.escape_ascii().to_string()
    );
    assert!(!snapshot.contains("] Response body: complete"));

    release_sender.send(()).map_err(|()| "upstream closed")?;
    let mut suffix = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut suffix)).await??;
    assert_eq!(suffix, expected_suffix);
    timeout(TEST_TIMEOUT, upstream_task).await???;

    let complete = export(&log, &directory.path().join("complete.log"))?;
    assert_eq!(
        escaped_body(&complete, "Request"),
        request_body.escape_ascii().to_string()
    );
    assert_eq!(
        escaped_body(&complete, "Response"),
        response_body.escape_ascii().to_string()
    );
    assert!(complete.contains("] Request body: complete"));
    assert!(complete.contains("] Response body: complete"));
    for index in 0..80 {
        assert!(complete.contains(&format!("x-request-{index}: value-{index}\n")));
        assert!(complete.contains(&format!("x-response-{index}: value-{index}\n")));
    }
    assert!(complete.contains("authorization: [REDACTED]\n"));
    assert!(complete.contains("set-cookie: [REDACTED]\n"));
    assert!(!complete.contains("request-secret"));
    assert!(!complete.contains("response-secret"));
    assert!(log.error().is_none());
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn exports_more_than_ui_retention_without_draining_ui_events() -> Result<(), TestError> {
    const REQUESTS: usize = 2_050;

    let directory = tempfile::tempdir()?;
    let log = FullLog::new()?;
    let proxy = ProxyHandle::start_recording(ProxyConfig::loopback(0), log.clone())?;
    for index in 0..REQUESTS {
        let mut client = timeout(TEST_TIMEOUT, TcpStream::connect(proxy.local_addr())).await??;
        let request = format!(
            "GET ftp://example.invalid/request-{index} HTTP/1.1\r\nHost: example.invalid\r\nConnection: close\r\n\r\n"
        );
        timeout(TEST_TIMEOUT, client.write_all(request.as_bytes())).await??;
        let mut response = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
        assert!(response.starts_with(b"HTTP/1.1 400 Bad Request\r\n"));
    }
    assert!(proxy.dropped_records() > 0);
    let complete = export(&log, &directory.path().join("all-records.log"))?;
    let requests: Vec<_> = complete
        .lines()
        .filter(|line| line.contains("] Request GET "))
        .collect();
    assert_eq!(requests.len(), REQUESTS);
    assert!(
        requests
            .first()
            .is_some_and(|line| line.ends_with("ftp://example.invalid/request-0"))
    );
    assert!(
        requests
            .last()
            .is_some_and(|line| line.ends_with("ftp://example.invalid/request-2049"))
    );
    assert!(log.error().is_none());
    proxy.stop()?;
    Ok(())
}
