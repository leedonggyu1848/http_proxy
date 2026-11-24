//! Helpers shared by the integration tests in `tests/`.
//!
//! Each integration test is its own crate and declares this module as
//! `pub mod common;`. Because the module and its items are `pub`, a helper
//! that one test crate does not call is not reported as dead code there, and
//! no lint has to be silenced.

use std::net::SocketAddr;
use std::time::Duration;

use http_proxy::exchange::{BodyLifecycle, BodyPreview, BodySnapshot, Exchange, Outcome};
use http_proxy::interception::ProxyHandle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Error type for every integration test and helper.
///
/// `Send + Sync` lets the same type cross `tokio::spawn` boundaries, which
/// several tests rely on when an upstream task returns its own result.
pub type TestError = Box<dyn std::error::Error + Send + Sync>;

/// Result type for every integration test and helper.
pub type TestResult<T = ()> = Result<T, TestError>;

/// How long the shared helpers wait for the proxy before failing a test.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest message head [`read_headers`] accepts.
pub const HEADER_LIMIT: usize = 16 * 1024;

/// The largest request head [`read_request`] accepts.
const REQUEST_HEAD_LIMIT: usize = 64 * 1024;

/// Reads an HTTP/1.1 message head up to and including the blank line.
pub async fn read_headers<S>(stream: &mut S) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    read_headers_within(stream, HEADER_LIMIT).await
}

/// Reads an HTTP/1.1 message head of at most `limit` bytes.
pub async fn read_headers_within<S>(stream: &mut S, limit: usize) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    while !data.ends_with(b"\r\n\r\n") {
        data.push(stream.read_u8().await?);
        if data.len() > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "headers exceeded the test limit",
            ));
        }
    }
    Ok(data)
}

/// What a test upstream received for one request.
#[derive(Debug, Eq, PartialEq)]
pub struct ReceivedRequest {
    pub method: String,
    pub target: String,
    /// Lowercase names with values, sorted, so header order does not matter.
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

/// Reads one HTTP/1.1 request with an optional `Content-Length` body.
pub async fn read_request(stream: &mut TcpStream) -> std::io::Result<ReceivedRequest> {
    let invalid = |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let head = read_headers_within(stream, REQUEST_HEAD_LIMIT).await?;
    let mut lines = head
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty());
    let request_line = String::from_utf8_lossy(lines.next().ok_or_else(|| invalid("empty"))?);
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut length = 0;
    for line in lines {
        let colon = line
            .iter()
            .position(|&byte| byte == b':')
            .ok_or_else(|| invalid("header without colon"))?;
        let (name, value) = line.split_at(colon);
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        let value = value.get(1..).unwrap_or_default().trim_ascii().to_vec();
        if name == "content-length" {
            length = String::from_utf8_lossy(&value)
                .parse()
                .map_err(|_| invalid("bad length"))?;
        }
        headers.push((name, value));
    }
    headers.sort();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(ReceivedRequest {
        method,
        target,
        headers,
        body,
    })
}

/// Sends a `GET` for `path` on `upstream` through the proxy, reads the reply
/// to the end, and returns the exchange the proxy published for it.
pub async fn exchange_through(
    proxy: &mut ProxyHandle,
    upstream: SocketAddr,
    path: &str,
) -> TestResult<Exchange> {
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream}{path} HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    // A cut body ends with a reset; only the end of the exchange matters here.
    let _read = timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await?;
    next_record(proxy).await
}

/// Waits for the next exchange the proxy publishes.
pub async fn next_record(proxy: &mut ProxyHandle) -> TestResult<Exchange> {
    Ok(timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?)
}

/// Waits up to [`TEST_TIMEOUT`] for the exchange's final outcome.
pub async fn settled(exchange: &Exchange) -> TestResult<&Outcome> {
    settled_within(exchange, TEST_TIMEOUT).await
}

/// Waits up to `limit` for the exchange's final outcome.
pub async fn settled_within(exchange: &Exchange, limit: Duration) -> TestResult<&Outcome> {
    Ok(timeout(limit, exchange.settled()).await?)
}

/// Polls `body` until it stops streaming and returns its final snapshot.
pub async fn wait_for_body_end(body: &BodyPreview) -> TestResult<BodySnapshot> {
    let poll = async {
        loop {
            if let Some(snapshot) = body.snapshot()
                && snapshot.lifecycle() != BodyLifecycle::Streaming
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    Ok(timeout(TEST_TIMEOUT, poll).await?)
}
