//! Manual measurement of forwarding throughput with and without the body store.
//!
//! Run in release mode:
//! `cargo test --release --test body_store_throughput -- --ignored --nocapture`.
//! `BODY_STORE_BENCH_MIB` changes the response size (default 1024 MiB) and
//! `BODY_STORE_BENCH_RATE_MIBPS` paces the upstream server (default unpaced).

use std::time::{Duration, Instant};

use http_proxy::exchange::BodySide;
use http_proxy::interception::{ProxyConfig, ProxyHandle};
use http_proxy::recording::BodyStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub mod common;

use common::{TestResult, read_headers};

const CHUNK: usize = 64 * 1024;
const RUNS: usize = 3;

async fn serve(listener: TcpListener, length: u64, rate: Option<u64>) {
    let chunk = vec![0x5a; CHUNK];
    while let Ok((mut stream, _)) = listener.accept().await {
        let chunk = chunk.clone();
        tokio::spawn(async move {
            if read_headers(&mut stream).await.is_err() {
                return;
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(response.as_bytes()).await.is_err() {
                return;
            }
            let started = Instant::now();
            let mut left = length;
            while left > 0 {
                if let Some(rate) = rate {
                    let nanos =
                        u128::from(length - left) * 1_000_000_000 / u128::from(rate * 1024 * 1024);
                    let due = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
                    tokio::time::sleep_until((started + due).into()).await;
                }
                let size = usize::try_from(left).unwrap_or(CHUNK).min(CHUNK);
                if stream
                    .write_all(chunk.get(..size).unwrap_or_default())
                    .await
                    .is_err()
                {
                    return;
                }
                left -= size as u64;
            }
        });
    }
}

/// Downloads one response through the proxy and returns the time to the
/// first body byte and the total time.
async fn download(
    proxy: std::net::SocketAddr,
    upstream: std::net::SocketAddr,
    length: u64,
) -> TestResult<(Duration, Duration)> {
    let started = Instant::now();
    let mut client = TcpStream::connect(proxy).await?;
    let request = format!(
        "GET http://{upstream}/video.mp4 HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n\r\n"
    );
    client.write_all(request.as_bytes()).await?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(client.read_u8().await?);
    }
    let mut buffer = vec![0; 256 * 1024];
    let mut received: u64 = 0;
    let mut first_byte = None;
    loop {
        let read = client.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        first_byte.get_or_insert_with(|| started.elapsed());
        received += read as u64;
    }
    if received != length {
        return Err(format!("received {received} of {length} bytes").into());
    }
    Ok((first_byte.unwrap_or_default(), started.elapsed()))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual measurement; run in release mode with --ignored --nocapture"]
#[expect(
    clippy::print_stderr,
    clippy::cast_precision_loss,
    reason = "reports measured values"
)]
async fn compare_forwarding_with_and_without_the_body_store() -> TestResult {
    let mib: u64 = std::env::var("BODY_STORE_BENCH_MIB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024);
    let length = mib * 1024 * 1024;
    let rate: Option<u64> = std::env::var("BODY_STORE_BENCH_RATE_MIBPS")
        .ok()
        .and_then(|value| value.parse().ok());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = listener.local_addr()?;
    tokio::spawn(serve(listener, length, rate));
    for store_enabled in [false, true, false, true, false, true] {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, 0)?;
        let mut config = ProxyConfig::loopback(0);
        if store_enabled {
            config = config.with_body_store(store.clone());
        }
        let mut proxy = ProxyHandle::start(config)?;
        let mut times = Vec::new();
        for _ in 0..RUNS {
            times.push(download(proxy.local_addr(), upstream, length).await?);
        }
        let mut stored = Vec::new();
        while let Some(exchange) = proxy.try_receive() {
            if let Some(body) = store.body(exchange.id(), BodySide::Response) {
                stored.push((body.stored_bytes(), body.gap()));
            }
        }
        proxy.stop()?;
        for (first_byte, total) in times {
            eprintln!(
                "store={store_enabled} size={mib}MiB rate={rate:?}MiB/s ttfb={:.2}ms total={:.3}s throughput={:.1}MiB/s",
                first_byte.as_secs_f64() * 1000.0,
                total.as_secs_f64(),
                mib as f64 / total.as_secs_f64()
            );
        }
        if store_enabled {
            eprintln!("stored bodies (bytes, gap): {stored:?}");
        }
    }
    Ok(())
}
