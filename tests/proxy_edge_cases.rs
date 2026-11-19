//! Forwarding edge cases from the design document's verification strategy.
//!
//! Every case drives the proxy over real loopback TCP connections.

use std::collections::HashSet;
use std::convert::Infallible;
use std::error::Error;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureOutcome, CaptureRecord,
    FailureStage, HeaderPreview, ProxyConfig, ProxyHandle,
};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_BYTES: usize = 64 * 1024;
const CHUNK_COUNT: usize = 64;
const LARGE_BODY_BYTES: u64 = (CHUNK_BYTES * CHUNK_COUNT) as u64;

type TestBody = BoxBody<Bytes, Infallible>;
type TestError = Box<dyn Error + Send + Sync>;

#[tokio::test]
async fn preserves_the_order_of_repeated_header_fields() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let request = read_headers(&mut stream).await?;
        let _send_result = request_sender.send(request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nX-Multi: one\r\nX-Multi: two\r\nX-Multi: three\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let response = exchange_raw(
        proxy.local_addr(),
        &format!(
            "GET http://{upstream_addr}/ HTTP/1.1\r\nHost: {upstream_addr}\r\nX-Dup: first\r\nX-Other: o\r\nX-Dup: second\r\nConnection: close\r\n\r\n"
        ),
    )
    .await?;
    // `HeaderMap` groups values by name, so only the order among values of
    // one name is guaranteed to survive forwarding.
    assert_eq!(
        field_values(&response, "x-multi"),
        vec!["one", "two", "three"]
    );
    let upstream_request = String::from_utf8(timeout(TEST_TIMEOUT, request_receiver).await??)?;
    assert_eq!(
        field_values(&upstream_request, "x-dup"),
        vec!["first", "second"]
    );

    let record = next_record(&mut proxy).await?;
    assert_eq!(
        captured_values(record.request_headers(), "x-dup"),
        vec!["first", "second"]
    );
    let CaptureOutcome::Response { headers, .. } = record.outcome() else {
        return Err("expected a response record".into());
    };
    assert_eq!(
        captured_values(headers, "x-multi"),
        vec!["one", "two", "three"]
    );

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn streams_large_chunked_bodies_in_both_directions() -> Result<(), TestError> {
    let (upstream_addr, upstream_task) = serve_upstream(|request: Request<Incoming>| async move {
        let received = request
            .into_body()
            .collect()
            .await
            .map_or(0, |body| body.to_bytes().len());
        Response::builder()
            .header("x-received", received.to_string())
            .body(RepeatBody::new(b'r', CHUNK_BYTES, CHUNK_COUNT).boxed())
    })
    .await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;

    let stream = TcpStream::connect(proxy.local_addr()).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    let connection_task = tokio::spawn(connection);
    let request = Request::post(format!("http://{upstream_addr}/upload"))
        .header("host", upstream_addr.to_string())
        .body(RepeatBody::new(b'q', CHUNK_BYTES, CHUNK_COUNT))?;
    let response = timeout(TEST_TIMEOUT, sender.send_request(request)).await??;
    assert_eq!(
        response.headers().get("x-received"),
        Some(&http::HeaderValue::from(LARGE_BODY_BYTES))
    );
    let body = timeout(TEST_TIMEOUT, response.into_body().collect())
        .await??
        .to_bytes();
    assert_eq!(body.len() as u64, LARGE_BODY_BYTES);
    assert!(body.iter().all(|byte| *byte == b'r'));

    let record = next_record(&mut proxy).await?;
    let request_body = wait_for_body_end(
        record
            .request_body()
            .ok_or("request body preview missing")?,
    )
    .await?;
    assert_eq!(request_body.observed_bytes(), LARGE_BODY_BYTES);
    assert_eq!(request_body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(request_body.retention(), BodyRetention::Truncated);
    let CaptureOutcome::Response { body, .. } = record.outcome() else {
        return Err("expected a response record".into());
    };
    let response_body = wait_for_body_end(body).await?;
    assert_eq!(response_body.observed_bytes(), LARGE_BODY_BYTES);
    assert_eq!(response_body.lifecycle(), BodyLifecycle::Complete);

    drop(sender);
    timeout(TEST_TIMEOUT, connection_task).await???;
    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_an_interrupted_body_when_upstream_closes_mid_response() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial...")
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/cut HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let headers = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    assert!(headers.starts_with(b"HTTP/1.1 200 OK"));
    let mut received = Vec::new();
    // The proxy aborts the client connection, which may surface as a reset.
    let _read = timeout(TEST_TIMEOUT, client.read_to_end(&mut received)).await?;
    assert!(received.len() < 100, "the client must not see a full body");

    let record = next_record(&mut proxy).await?;
    let CaptureOutcome::Response { status, body, .. } = record.outcome() else {
        return Err("expected a response record".into());
    };
    assert_eq!(*status, 200);
    let body = wait_for_body_end(body).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Interrupted);
    assert_eq!(body.bytes(), b"partial...");

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn forwards_many_concurrent_requests_independently() -> Result<(), TestError> {
    const CLIENTS: usize = 32;
    let (upstream_addr, upstream_task) = serve_upstream(|request: Request<Incoming>| async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let path = request.uri().path().to_owned();
        Response::builder().body(Full::new(Bytes::from(path)).boxed())
    })
    .await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let proxy_addr = proxy.local_addr();

    let mut clients = Vec::with_capacity(CLIENTS);
    for index in 0..CLIENTS {
        clients.push(tokio::spawn(async move {
            let request = format!(
                "GET http://{upstream_addr}/client-{index} HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
            );
            let response = exchange_raw(proxy_addr, &request).await?;
            Ok::<_, TestError>((index, response))
        }));
    }
    for client in clients {
        let (index, response) = timeout(TEST_TIMEOUT, client).await???;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with(&format!("/client-{index}")));
    }

    let mut ids = HashSet::new();
    let mut targets = HashSet::new();
    for _ in 0..CLIENTS {
        let record = next_record(&mut proxy).await?;
        assert!(matches!(
            record.outcome(),
            CaptureOutcome::Response { status: 200, .. }
        ));
        ids.insert(record.id());
        targets.insert(record.target().to_owned());
    }
    assert_eq!(ids.len(), CLIENTS);
    assert_eq!(targets.len(), CLIENTS);

    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_the_stage_at_which_a_request_failed() -> Result<(), TestError> {
    let refused_addr = unused_addr().await?;
    let malformed = TcpListener::bind("127.0.0.1:0").await?;
    let malformed_addr = malformed.local_addr()?;
    let malformed_task = tokio::spawn(async move {
        let (mut stream, _) = malformed.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream.write_all(b"this is not HTTP\r\n\r\n").await?;
        Ok::<_, std::io::Error>(())
    });

    let cases = [
        (
            "GET /no-host HTTP/1.0\r\n\r\n".to_owned(),
            "HTTP/1.0 400 Bad Request",
            FailureStage::Request,
        ),
        (
            "GET https://example.invalid/ HTTP/1.1\r\nHost: example.invalid\r\nConnection: close\r\n\r\n".to_owned(),
            "HTTP/1.1 400 Bad Request",
            FailureStage::Request,
        ),
        (
            format!("GET http://{refused_addr}/ HTTP/1.1\r\nHost: {refused_addr}\r\nConnection: close\r\n\r\n"),
            "HTTP/1.1 502 Bad Gateway",
            FailureStage::Connect,
        ),
        (
            format!("GET http://{malformed_addr}/ HTTP/1.1\r\nHost: {malformed_addr}\r\nConnection: close\r\n\r\n"),
            "HTTP/1.1 502 Bad Gateway",
            FailureStage::Response,
        ),
    ];

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    for (request, expected_status, expected_stage) in cases {
        let response = exchange_raw(proxy.local_addr(), &request).await?;
        assert!(
            response.starts_with(expected_status),
            "{request:?} -> {response:?}"
        );
        let record = next_record(&mut proxy).await?;
        let CaptureOutcome::Failed { stage, message } = record.outcome() else {
            return Err(format!("{request:?} was not recorded as a failure").into());
        };
        assert_eq!(*stage, expected_stage, "{request:?}");
        assert_ne!(message, "");
    }

    timeout(TEST_TIMEOUT, malformed_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn forwards_several_requests_on_one_keep_alive_connection() -> Result<(), TestError> {
    let (upstream_addr, upstream_task) = serve_upstream(|request: Request<Incoming>| async move {
        let path = request.uri().path().to_owned();
        Response::builder().body(Full::new(Bytes::from(path)).boxed())
    })
    .await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;

    for path in ["/first", "/second"] {
        let request =
            format!("GET http://{upstream_addr}{path} HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n");
        client.write_all(request.as_bytes()).await?;
        let headers = String::from_utf8(timeout(TEST_TIMEOUT, read_headers(&mut client)).await??)?;
        assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers:?}");
        let length: usize = field_values(&headers, "content-length")
            .first()
            .ok_or("content-length missing")?
            .parse()?;
        let mut body = vec![0_u8; length];
        timeout(TEST_TIMEOUT, client.read_exact(&mut body)).await??;
        assert_eq!(body, path.as_bytes());
    }

    for path in ["/first", "/second"] {
        let record = next_record(&mut proxy).await?;
        assert!(record.target().ends_with(path));
        assert!(matches!(
            record.outcome(),
            CaptureOutcome::Response { status: 200, .. }
        ));
    }

    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn keeps_forwarding_and_counts_records_dropped_from_a_full_queue() -> Result<(), TestError> {
    const REQUESTS: u64 = 4;
    let (upstream_addr, upstream_task) = serve_upstream(|_request: Request<Incoming>| async {
        Response::builder().body(Full::new(Bytes::from_static(b"ok")).boxed())
    })
    .await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_event_capacity(1))?;

    for index in 0..REQUESTS {
        let request = format!(
            "GET http://{upstream_addr}/{index} HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
        );
        let response = exchange_raw(proxy.local_addr(), &request).await?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("ok"));
    }

    assert_eq!(proxy.dropped_records(), REQUESTS - 1);
    let kept = next_record(&mut proxy).await?;
    assert!(kept.target().ends_with("/0"));
    assert!(proxy.try_receive().is_none());

    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn stop_closes_pending_exchanges_and_open_tunnels() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (received_sender, received_receiver) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        let _send_result = received_sender.send(());
        let (_tunnel, _) = upstream.accept().await?;
        tokio::time::sleep(TEST_TIMEOUT).await;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let proxy_addr = proxy.local_addr();
    let mut pending = TcpStream::connect(proxy_addr).await?;
    let request = format!(
        "GET http://{upstream_addr}/never HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    pending.write_all(request.as_bytes()).await?;
    timeout(TEST_TIMEOUT, received_receiver).await??;

    let mut tunnel = TcpStream::connect(proxy_addr).await?;
    let connect = format!("CONNECT {upstream_addr} HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n");
    tunnel.write_all(connect.as_bytes()).await?;
    let tunnel_response = timeout(TEST_TIMEOUT, read_headers(&mut tunnel)).await??;
    assert!(tunnel_response.starts_with(b"HTTP/1.1 200 OK"));
    assert!(matches!(
        next_record(&mut proxy).await?.outcome(),
        CaptureOutcome::TunnelEstablished { .. }
    ));

    tokio::task::spawn_blocking(move || proxy.stop()).await??;

    let mut rest = Vec::new();
    let pending_closed = timeout(TEST_TIMEOUT, pending.read_to_end(&mut rest)).await?;
    assert!(
        pending_closed.is_err() || rest.is_empty(),
        "a pending exchange must end without a forwarded response"
    );
    let tunnel_closed = timeout(TEST_TIMEOUT, tunnel.read_to_end(&mut rest)).await?;
    assert!(tunnel_closed.is_err() || rest.is_empty());
    assert!(TcpStream::connect(proxy_addr).await.is_err());

    upstream_task.abort();
    Ok(())
}

/// Response body with a fixed number of equal chunks and no length hint,
/// so HTTP/1.1 sends it with chunked transfer coding.
struct RepeatBody {
    chunk: Bytes,
    remaining: usize,
}

impl RepeatBody {
    fn new(byte: u8, chunk_bytes: usize, chunks: usize) -> Self {
        Self {
            chunk: Bytes::from(vec![byte; chunk_bytes]),
            remaining: chunks,
        }
    }
}

impl Body for RepeatBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            return Poll::Ready(None);
        }
        this.remaining -= 1;
        Poll::Ready(Some(Ok(Frame::data(this.chunk.clone()))))
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

async fn serve_upstream<F, Fut>(handler: F) -> Result<(SocketAddr, JoinHandle<()>), TestError>
where
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = http::Result<Response<TestBody>>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let handler = handler.clone();
            tokio::spawn(async move {
                let service = service_fn(&handler);
                let _ended = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Ok((address, task))
}

async fn unused_addr() -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    listener.local_addr()
}

async fn exchange_raw(proxy_addr: SocketAddr, request: &str) -> Result<String, TestError> {
    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    Ok(String::from_utf8(response)?)
}

async fn next_record(proxy: &mut ProxyHandle) -> Result<CaptureRecord, TestError> {
    Ok(timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?)
}

async fn wait_for_body_end(body: &BodyPreview) -> Result<BodySnapshot, TestError> {
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

fn field_values<'a>(message: &'a str, name: &str) -> Vec<&'a str> {
    message
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .filter(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .collect()
}

fn captured_values<'a>(headers: &'a HeaderPreview, name: &str) -> Vec<&'a str> {
    headers
        .entries()
        .iter()
        .filter(|header| header.name() == name)
        .map(http_proxy::CapturedHeader::value)
        .collect()
}

async fn read_headers<S>(stream: &mut S) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    while !data.ends_with(b"\r\n\r\n") {
        data.push(stream.read_u8().await?);
        if data.len() > 16 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "headers exceeded the test limit",
            ));
        }
    }
    Ok(data)
}
