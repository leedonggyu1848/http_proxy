//! How the end of a streamed body, its size, and the exchange's timing
//! phases are recorded without delaying the forwarded stream.

use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use http_proxy::{
    BodyLifecycle, BodyPreview, BodySnapshot, CapturedResponse, Exchange, Outcome, ProxyConfig,
    ProxyHandle,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

type TestError = Box<dyn Error + Send + Sync>;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const LARGE_BODY: usize = 4 * 1024 * 1024;
const REQUEST_BODY: usize = 100 * 1024;

#[tokio::test]
async fn records_size_and_every_phase_of_a_large_exchange() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let headers = read_headers(&mut stream).await?;
        assert!(headers.starts_with(b"POST /large HTTP/1.1"));
        let mut request_body = vec![0_u8; REQUEST_BODY];
        stream.read_exact(&mut request_body).await?;
        let head =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {LARGE_BODY}\r\nConnection: close\r\n\r\n");
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&vec![b'r'; LARGE_BODY]).await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let head = format!(
        "POST http://{upstream_addr}/large HTTP/1.1\r\nHost: {upstream_addr}\r\nContent-Length: {REQUEST_BODY}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(head.as_bytes()).await?;
    client.write_all(&vec![b'q'; REQUEST_BODY]).await?;
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    assert!(response.ends_with(&vec![b'r'; 1024]));

    let exchange = next_record(&mut proxy).await?;
    let response = settled_response(&exchange).await?;
    let request_body = wait_for_end(exchange.request().body().ok_or("no request body")?).await?;
    assert_eq!(request_body.observed_bytes(), REQUEST_BODY as u64);
    assert_eq!(request_body.lifecycle(), BodyLifecycle::Complete);
    let response_body = wait_for_end(response.body()).await?;
    assert_eq!(response_body.observed_bytes(), LARGE_BODY as u64);
    assert_eq!(response_body.lifecycle(), BodyLifecycle::Complete);

    let timing = exchange.timing();
    let phases = [
        timing.connect(),
        timing.send(),
        timing.wait(),
        timing.receive(),
    ];
    assert!(phases.iter().all(Option::is_some), "{timing:?}");
    let settled = timing.settled().ok_or("headers time missing")?;
    let receive = timing.receive().ok_or("receive time missing")?;
    assert_eq!(timing.complete(), Some(settled + receive));

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_the_size_of_a_body_the_upstream_cut_short() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2000000\r\n\r\n")
            .await?;
        stream.write_all(&vec![b'c'; 300_000]).await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let exchange = exchange_through(&mut proxy, upstream_addr, "/cut").await?;
    let body = wait_for_end(settled_response(&exchange).await?.body()).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Interrupted);
    assert_eq!(body.observed_bytes(), 300_000);
    assert!(exchange.timing().receive().is_some());
    assert!(exchange.timing().complete().is_some());

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn records_a_body_cancelled_by_a_departing_client() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhello")
            .await?;
        tokio::time::sleep(TEST_TIMEOUT).await;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/slow HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let _headers = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    let mut start = [0_u8; 5];
    timeout(TEST_TIMEOUT, client.read_exact(&mut start)).await??;
    assert_eq!(&start, b"hello");

    let exchange = next_record(&mut proxy).await?;
    let response = settled_response(&exchange).await?;
    drop(client);
    let body = wait_for_end(response.body()).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Cancelled);
    assert_eq!(body.observed_bytes(), 5);

    upstream_task.abort();
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn relays_each_chunk_before_the_body_ends() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (finish_sender, finish_receiver) = oneshot::channel::<()>();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n")
            .await?;
        let _finish = finish_receiver.await;
        stream.write_all(b"4\r\nlast\r\n0\r\n\r\n").await?;
        Ok::<_, std::io::Error>(())
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/stream HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let _headers = timeout(TEST_TIMEOUT, read_headers(&mut client)).await??;
    let first = timeout(Duration::from_secs(2), read_until(&mut client, b"first")).await??;
    assert!(first.ends_with(b"first"));

    let exchange = next_record(&mut proxy).await?;
    let response = settled_response(&exchange).await?;
    let streaming = response.body().snapshot().ok_or("preview busy")?;
    assert_eq!(streaming.lifecycle(), BodyLifecycle::Streaming);
    assert_eq!(exchange.timing().receive(), None);

    let _finished = finish_sender.send(());
    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut rest)).await??;
    let body = wait_for_end(response.body()).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(body.bytes(), b"firstlast");

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

async fn exchange_through(
    proxy: &mut ProxyHandle,
    upstream: SocketAddr,
    path: &str,
) -> Result<Exchange, TestError> {
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

async fn next_record(proxy: &mut ProxyHandle) -> Result<Exchange, TestError> {
    Ok(timeout(TEST_TIMEOUT, proxy.receive())
        .await?
        .ok_or("capture channel closed")?)
}

async fn settled_response(exchange: &Exchange) -> Result<&CapturedResponse, TestError> {
    match timeout(TEST_TIMEOUT, exchange.settled()).await? {
        Outcome::Response(response) => Ok(response),
        other => Err(format!("expected a response, got {other:?}").into()),
    }
}

async fn wait_for_end(body: &BodyPreview) -> Result<BodySnapshot, TestError> {
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

async fn read_until<S>(stream: &mut S, marker: &[u8]) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    while !data.ends_with(marker) {
        data.push(stream.read_u8().await?);
    }
    Ok(data)
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
