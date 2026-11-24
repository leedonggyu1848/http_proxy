//! How an exchange is published, settled, and told apart from other endings.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use http_proxy::exchange::{BodyLifecycle, CaptureSource, FailureStage, Outcome};
use http_proxy::interception::{ProxyConfig, ProxyHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{
    TEST_TIMEOUT, TestError, exchange_through, next_record, read_headers, settled,
    wait_for_body_end,
};

#[tokio::test]
async fn publishes_an_exchange_before_its_response_arrives() -> Result<(), TestError> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream.local_addr()?;
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel::<()>();
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let _request = read_headers(&mut stream).await?;
        let _released = release_receiver.await;
        stream
            .write_all(
                b"HTTP/1.1 200 Fine Thanks\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            )
            .await?;
        Ok::<_, std::io::Error>(())
    });

    let before = SystemTime::now();
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{upstream_addr}/wait HTTP/1.1\r\nHost: {upstream_addr}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;

    let exchange = next_record(&mut proxy).await?;
    assert!(exchange.outcome().is_in_flight());
    assert_eq!(exchange.timing().settled(), None);
    assert_eq!(exchange.source(), CaptureSource::HttpProxy);
    assert_eq!(exchange.request().method(), http::Method::GET);
    assert!(exchange.started_at() >= before);

    let _released = release_sender.send(());
    let mut response = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
    assert!(response.starts_with(b"HTTP/1.1 200 Fine Thanks"));

    let Outcome::Response(captured) = settled(&exchange).await? else {
        return Err("expected a response".into());
    };
    assert_eq!(captured.status(), 200);
    assert_eq!(captured.reason(), "Fine Thanks");
    assert_eq!(captured.version(), http::Version::HTTP_11);
    let timing = exchange.timing();
    let connect = timing.connect().ok_or("connect time not observed")?;
    let settled_after = timing.settled().ok_or("settle time not observed")?;
    assert!(connect <= settled_after);
    assert_eq!(exchange.elapsed(), settled_after);

    timeout(TEST_TIMEOUT, upstream_task).await???;
    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn distinguishes_no_response_empty_response_connect_failure_and_cut_body()
-> Result<(), TestError> {
    let silent = upstream_replying(None).await?;
    let empty = upstream_replying(Some(
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
    ))
    .await?;
    let cut = upstream_replying(Some(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nonly ten b",
    ))
    .await?;
    let refused = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        listener.local_addr()?
    };
    let config = ProxyConfig::loopback(0).with_response_timeout(Duration::from_millis(200));
    let mut proxy = ProxyHandle::start(config)?;

    let no_response = exchange_through(&mut proxy, silent, "/").await?;
    assert!(matches!(
        settled(&no_response).await?,
        Outcome::Failed {
            stage: FailureStage::ResponseTimeout,
            ..
        }
    ));
    assert!(no_response.timing().connect().is_some());

    let empty_response = exchange_through(&mut proxy, empty, "/").await?;
    let Outcome::Response(response) = settled(&empty_response).await? else {
        return Err("expected an empty response".into());
    };
    assert_eq!(response.status(), 204);
    let body = wait_for_body_end(response.body()).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Complete);
    assert_eq!(body.observed_bytes(), 0);

    let connect_failure = exchange_through(&mut proxy, refused, "/").await?;
    assert!(matches!(
        settled(&connect_failure).await?,
        Outcome::Failed {
            stage: FailureStage::Connect,
            ..
        }
    ));
    assert_eq!(connect_failure.timing().connect(), None);
    assert!(connect_failure.request().body().is_none());

    let cut_body = exchange_through(&mut proxy, cut, "/").await?;
    let Outcome::Response(response) = settled(&cut_body).await? else {
        return Err("expected a response whose body was cut".into());
    };
    let body = wait_for_body_end(response.body()).await?;
    assert_eq!(body.lifecycle(), BodyLifecycle::Interrupted);
    assert_eq!(body.observed_bytes(), 10);

    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn marks_an_exchange_cancelled_when_the_client_disconnects() -> Result<(), TestError> {
    let silent = upstream_replying(None).await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let request = format!(
        "GET http://{silent}/abandoned HTTP/1.1\r\nHost: {silent}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let exchange = next_record(&mut proxy).await?;
    assert!(exchange.outcome().is_in_flight());

    drop(client);
    assert!(matches!(settled(&exchange).await?, Outcome::Cancelled));

    proxy.stop()?;
    Ok(())
}

#[tokio::test]
async fn tags_exchanges_with_their_session_and_source() -> Result<(), TestError> {
    let first = upstream_replying(Some(
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
    ))
    .await?;
    let second = upstream_replying(Some(
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
    ))
    .await?;
    let tunnel_target = TcpListener::bind("127.0.0.1:0").await?;
    let tunnel_addr = tunnel_target.local_addr()?;

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let http = exchange_through(&mut proxy, first, "/").await?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let connect = format!("CONNECT {tunnel_addr} HTTP/1.1\r\nHost: {tunnel_addr}\r\n\r\n");
    client.write_all(connect.as_bytes()).await?;
    let connect = next_record(&mut proxy).await?;
    assert!(matches!(settled(&connect).await?, Outcome::Tunnel(_)));
    assert_eq!(http.source(), CaptureSource::HttpProxy);
    assert_eq!(connect.source(), CaptureSource::Connect);
    assert_eq!(http.session(), connect.session());
    assert_ne!(http.id(), connect.id());
    drop(client);
    proxy.stop()?;

    let mut restarted = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let later = exchange_through(&mut restarted, second, "/").await?;
    assert_ne!(later.session(), http.session());
    restarted.stop()?;
    Ok(())
}

#[tokio::test]
async fn notifies_after_each_queued_exchange() -> Result<(), TestError> {
    let upstream = upstream_replying(Some(
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
    ))
    .await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&notified);
    assert!(proxy.notify_on_event(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    assert!(!proxy.notify_on_event(|| {}));

    let exchange = exchange_through(&mut proxy, upstream, "/").await?;
    assert!(matches!(settled(&exchange).await?, Outcome::Response(_)));
    assert_eq!(notified.load(Ordering::SeqCst), 1);
    proxy.stop()?;
    Ok(())
}

/// Starts a one-shot upstream that reads a request and sends `reply`, or
/// stays silent when `reply` is `None`.
async fn upstream_replying(reply: Option<&'static [u8]>) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        if read_headers(&mut stream).await.is_err() {
            return;
        }
        match reply {
            Some(reply) => {
                let _written = stream.write_all(reply).await;
            }
            None => tokio::time::sleep(TEST_TIMEOUT).await,
        }
    });
    Ok(address)
}
