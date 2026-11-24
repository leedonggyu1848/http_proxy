//! Saving bodies captured through a running proxy from the body store.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use http_proxy::exchange::{BodyLifecycle, BodySide, Exchange};
use http_proxy::interception::{ProxyConfig, ProxyHandle};
use http_proxy::recording::{BodyLayout, BodyStore, SaveBody, SaveFormat, SavedBody, StoreGap};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{TEST_TIMEOUT, TestResult, read_headers};

const IMAGE: &[u8] = include_bytes!("fixtures/images/animated.gif");
const JSON: &[u8] = include_bytes!("fixtures/body-preview.json");

/// One canned upstream response.
struct Reply {
    headers: String,
    body: Vec<u8>,
    /// Bytes actually sent; fewer than `body` cuts the connection early.
    sent: usize,
}

impl Reply {
    fn new(headers: &str, body: Vec<u8>) -> Self {
        Self {
            headers: headers.to_owned(),
            sent: body.len(),
            body,
        }
    }
}

fn reply_for(path: &str) -> Reply {
    match path {
        "/media/animation" => Reply::new("Content-Type: image/gif\r\n", IMAGE.to_vec()),
        "/docs/report.pdf" => Reply::new(
            "Content-Type: application/pdf\r\n",
            pseudo_random(70_000, 1),
        ),
        "/download" => Reply::new(
            "Content-Type: application/zip\r\nContent-Disposition: attachment; filename=\"bundle.zip\"\r\n",
            pseudo_random(300_000, 2),
        ),
        "/api/data" => Reply::new(
            "Content-Type: application/json\r\nContent-Encoding: gzip\r\n",
            gzip(JSON).unwrap_or_default(),
        ),
        "/evil" => Reply::new(
            "Content-Type: text/plain\r\nContent-Disposition: attachment; filename*=UTF-8''..%2F..%2F%2Fetc%2Fpasswd%0A\r\n",
            b"not a password file".to_vec(),
        ),
        "/cut" => {
            let mut reply = Reply::new("Content-Type: video/mp2t\r\n", pseudo_random(10_000, 3));
            reply.sent = 4_000;
            reply
        }
        _ => Reply::new("Content-Type: text/plain\r\n", b"ok".to_vec()),
    }
}

/// Length of the resources served in ranges.
const RANGED_LENGTH: usize = 1_000;

/// Answers `Range: bytes=<a>-<b>` requests for `/ranged.mp4` and
/// `/gappy.mp4` with `206 Partial Content`; a `X-Version: 2` request header
/// gets a different `ETag`.
fn ranged_reply(path: &str, head: &str) -> Option<Vec<u8>> {
    // Hostile or broken servers: a tiny body far into a huge resource, and
    // a body shorter than its Content-Range.
    let odd = match path {
        // Larger than any disk, so every machine refuses it the same way.
        "/huge.bin" => Some((
            "bytes 18446744073709551600-18446744073709551609/18446744073709551610",
            10,
        )),
        "/short.mp4" => Some(("bytes 0-99/1000", 50)),
        _ => None,
    };
    if let Some((range, length)) = odd {
        let mut response = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: {range}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
        .into_bytes();
        response.extend(std::iter::repeat_n(b'x', length));
        return Some(response);
    }
    if path != "/ranged.mp4" && path != "/gappy.mp4" {
        return None;
    }
    let lower = head.to_ascii_lowercase();
    let range = lower
        .lines()
        .find_map(|line| line.strip_prefix("range: bytes="))?;
    let (start, end) = range.trim().split_once('-')?;
    let (start, end): (usize, usize) = (start.parse().ok()?, end.parse().ok()?);
    let version = if lower.contains("x-version: 2") { 2 } else { 1 };
    let resource = pseudo_random(RANGED_LENGTH, version);
    let body = resource.get(start..=end)?;
    let mut response = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: bytes {start}-{end}/{RANGED_LENGTH}\r\nETag: \"v{version}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    Some(response)
}

fn pseudo_random(length: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// Serves canned replies; a request with `Content-Length` has its body read first.
async fn serve(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let Ok(head) = read_headers(&mut stream).await else {
                return;
            };
            let head = String::from_utf8_lossy(&head);
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            if stream.read_exact(&mut body).await.is_err() {
                return;
            }
            let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
            if let Some(response) = ranged_reply(&path, &head) {
                let _closed = stream.write_all(&response).await;
                return;
            }
            let reply = reply_for(&path);
            let response = format!(
                "HTTP/1.1 200 OK\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                reply.headers,
                reply.body.len()
            );
            let _closed = stream.write_all(response.as_bytes()).await;
            let _closed = stream
                .write_all(reply.body.get(..reply.sent).unwrap_or_default())
                .await;
        });
    }
}

struct Harness {
    proxy: ProxyHandle,
    upstream: std::net::SocketAddr,
    store: BodyStore,
    /// Exchanges received from the proxy so far.
    seen: Vec<Exchange>,
    _root: tempfile::TempDir,
}

impl Harness {
    async fn start(limit_bytes: u64) -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), limit_bytes, 0)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream = listener.local_addr()?;
        tokio::spawn(serve(listener));
        let proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_body_store(store.clone()))?;
        Ok(Self {
            proxy,
            upstream,
            store,
            seen: Vec::new(),
            _root: root,
        })
    }

    /// Sends one request through the proxy and returns the response body bytes.
    async fn fetch(&self, method: &str, path: &str, body: &[u8]) -> TestResult<Vec<u8>> {
        let mut client = TcpStream::connect(self.proxy.local_addr()).await?;
        let address = self.upstream;
        let request = format!(
            "{method} http://{address}{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        client.write_all(request.as_bytes()).await?;
        client.write_all(body).await?;
        let mut response = Vec::new();
        // A cut response ends with an error after its partial body.
        let _cut = timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await?;
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or("no response head")?;
        Ok(response.split_off(split + 4))
    }

    /// Sends a range request through the proxy and returns the response body.
    async fn fetch_range(&self, path: &str, range: &str, extra: &str) -> TestResult<Vec<u8>> {
        let mut client = TcpStream::connect(self.proxy.local_addr()).await?;
        let address = self.upstream;
        let request = format!(
            "GET http://{address}{path} HTTP/1.1\r\nHost: {address}\r\nRange: bytes={range}\r\n{extra}Connection: close\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await?;
        let mut response = Vec::new();
        timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await??;
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or("no response head")?;
        Ok(response.split_off(split + 4))
    }

    /// Returns every exchange for `path` once `count` of them have settled bodies.
    async fn all_exchanges(&mut self, path: &str, count: usize) -> TestResult<Vec<Exchange>> {
        let started = Instant::now();
        loop {
            while let Some(exchange) = self.proxy.try_receive() {
                self.seen.push(exchange);
            }
            let settled: Vec<Exchange> = self
                .seen
                .iter()
                .filter(|exchange| exchange.request().target().ends_with(path))
                .filter(|exchange| {
                    self.store
                        .body(exchange.id(), BodySide::Response)
                        .is_some_and(|body| body.lifecycle() != BodyLifecycle::Streaming)
                })
                .cloned()
                .collect();
            if settled.len() >= count {
                return Ok(settled);
            }
            if started.elapsed() > TEST_TIMEOUT {
                return Err(format!("only {} settled exchanges for {path}", settled.len()).into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Returns the exchange for `path` once both bodies stopped streaming.
    async fn exchange(&mut self, path: &str) -> TestResult<Exchange> {
        let started = Instant::now();
        loop {
            while let Some(exchange) = self.proxy.try_receive() {
                self.seen.push(exchange);
            }
            let found = self
                .seen
                .iter()
                .find(|exchange| exchange.request().target().ends_with(path))
                .cloned();
            if let Some(exchange) = found {
                let ended = [BodySide::Request, BodySide::Response].iter().all(|&side| {
                    self.store
                        .body(exchange.id(), side)
                        .is_some_and(|body| body.lifecycle() != BodyLifecycle::Streaming)
                });
                if ended {
                    return Ok(exchange);
                }
            }
            if started.elapsed() > TEST_TIMEOUT {
                return Err(format!("no settled exchange for {path}").into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn save(
        &mut self,
        path: &str,
        side: BodySide,
        format: SaveFormat,
        directory: &Path,
    ) -> TestResult<SavedBody> {
        let exchange = self.exchange(path).await?;
        let request =
            SaveBody::new(&exchange, side, format, directory.to_path_buf()).ok_or("no response")?;
        let receiver = self.store.save(request)?;
        Ok(tokio::task::spawn_blocking(move || receiver.recv_timeout(TEST_TIMEOUT)).await???)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_bodies_match_the_received_bytes() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    let cases: [(&str, &str, Vec<u8>); 3] = [
        ("/media/animation", "animation.gif", IMAGE.to_vec()),
        ("/docs/report.pdf", "report.pdf", pseudo_random(70_000, 1)),
        ("/download", "bundle.zip", pseudo_random(300_000, 2)),
    ];
    for (path, name, expected) in cases {
        let forwarded = harness.fetch("GET", path, b"").await?;
        assert_eq!(sha256(&forwarded), sha256(&expected), "{path} forwarded");
        let saved = harness
            .save(path, BodySide::Response, SaveFormat::Decoded, output.path())
            .await?;
        assert_eq!(saved.path(), output.path().join(name));
        assert!(saved.complete());
        assert_eq!(
            sha256(&std::fs::read(saved.path())?),
            sha256(&expected),
            "{path} saved"
        );
    }

    let encoded = harness.fetch("GET", "/api/data", b"").await?;
    let raw = harness
        .save(
            "/api/data",
            BodySide::Response,
            SaveFormat::AsReceived,
            output.path(),
        )
        .await?;
    assert_eq!(raw.path(), output.path().join("data.json.gz"));
    assert_eq!(sha256(&std::fs::read(raw.path())?), sha256(&encoded));
    let decoded = harness
        .save(
            "/api/data",
            BodySide::Response,
            SaveFormat::Decoded,
            output.path(),
        )
        .await?;
    assert_eq!(decoded.path(), output.path().join("data.json"));
    assert_eq!(std::fs::read(decoded.path())?, JSON);
    let again = harness
        .save(
            "/api/data",
            BodySide::Response,
            SaveFormat::Decoded,
            output.path(),
        )
        .await?;
    assert_eq!(again.path(), output.path().join("data (1).json"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_request_bodies_and_keeps_names_inside_the_directory() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    harness.fetch("POST", "/upload", JSON).await?;
    let request = harness
        .save(
            "/upload",
            BodySide::Request,
            SaveFormat::Decoded,
            output.path(),
        )
        .await?;
    let id = harness.exchange("/upload").await?.id().value();
    assert_eq!(
        request.path(),
        output.path().join(format!("request-{id}.json"))
    );
    assert_eq!(std::fs::read(request.path())?, JSON);

    harness.fetch("GET", "/evil", b"").await?;
    let evil = harness
        .save(
            "/evil",
            BodySide::Response,
            SaveFormat::AsReceived,
            output.path(),
        )
        .await?;
    assert_eq!(evil.path(), output.path().join("passwd.txt"));
    assert_eq!(std::fs::read_dir(output.path())?.count(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn incomplete_bodies_are_saved_as_partial_files() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    let forwarded = harness.fetch("GET", "/cut", b"").await?;
    assert_eq!(forwarded.len(), 4_000);
    let exchange = harness.exchange("/cut").await?;
    let stored = harness.store.body(exchange.id(), BodySide::Response);
    assert!(stored.is_some_and(|body| body.lifecycle() == BodyLifecycle::Interrupted));
    let saved = harness
        .save(
            "/cut",
            BodySide::Response,
            SaveFormat::Decoded,
            output.path(),
        )
        .await?;
    assert!(!saved.complete());
    assert_eq!(saved.path(), output.path().join("cut.ts.partial"));
    assert_eq!(std::fs::read(saved.path())?, forwarded);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reaching_the_session_limit_keeps_forwarding() -> TestResult {
    let mut harness = Harness::start(1_000).await?;
    let output = tempfile::tempdir()?;
    let forwarded = harness.fetch("GET", "/download", b"").await?;
    assert_eq!(sha256(&forwarded), sha256(&pseudo_random(300_000, 2)));
    let exchange = harness.exchange("/download").await?;
    let stored = harness
        .store
        .body(exchange.id(), BodySide::Response)
        .ok_or("not stored")?;
    assert_eq!(stored.gap(), Some(StoreGap::SessionLimit));
    assert!(harness.store.limit_reached());
    let saved = harness
        .save(
            "/download",
            BodySide::Response,
            SaveFormat::AsReceived,
            output.path(),
        )
        .await?;
    assert_eq!(saved.path(), output.path().join("bundle.zip.partial"));
    let kept = std::fs::read(saved.path())?;
    assert!(forwarded.starts_with(&kept));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_many_bodies_with_a_manifest() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    let paths = ["/media/animation", "/api/data", "/manifest.json", "/cut"];
    for path in paths {
        harness.fetch("GET", path, b"").await?;
    }
    let mut bodies = Vec::new();
    for path in paths {
        let exchange = harness.exchange(path).await?;
        bodies.push(
            SaveBody::new(
                &exchange,
                BodySide::Response,
                SaveFormat::Decoded,
                output.path().to_path_buf(),
            )
            .ok_or("no response")?,
        );
    }
    let folder = output.path().join("bulk");
    let running = harness
        .store
        .save_all(bodies, BodyLayout::HostPath, folder.clone())?;
    let summary = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        loop {
            if let Some(result) = running.try_finish() {
                return result.map(|summary| (summary, running.progress().done()));
            }
            if started.elapsed() > TEST_TIMEOUT {
                return Err(io::Error::other("bulk save timed out"));
            }
            std::thread::yield_now();
        }
    })
    .await??;
    let (summary, done) = summary;
    assert_eq!(
        (summary.saved(), summary.incomplete(), summary.failed()),
        (4, 1, 0)
    );
    assert_eq!(done, 4);
    assert!(!summary.cancelled());

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join("manifest.json"))?)?;
    let field = |entry: &serde_json::Value, name: &str| entry.get(name).cloned();
    assert_eq!(field(&manifest, "layout"), Some("host-path".into()));
    let entries = manifest
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or("no entries")?;
    assert_eq!(entries.len(), 4);
    let host = harness.upstream.to_string().replace(':', "_");
    let expected = [
        (format!("{host}/media/animation.gif"), IMAGE.to_vec(), true),
        (format!("{host}/api/data.json"), JSON.to_vec(), true),
        (format!("{host}/manifest.json"), b"ok".to_vec(), true),
        (format!("{host}/cut.ts.partial"), Vec::new(), false),
    ];
    for (entry, (file, bytes, complete)) in entries.iter().zip(expected) {
        assert_eq!(field(entry, "file"), Some(file.as_str().into()), "{entry}");
        assert_eq!(field(entry, "status"), Some(200.into()));
        assert_eq!(field(entry, "side"), Some("response".into()));
        assert_eq!(field(entry, "complete"), Some(complete.into()));
        let saved = std::fs::read(folder.join(&file))?;
        let digest = sha256(&saved).iter().fold(String::new(), |mut hex, byte| {
            let _infallible = write!(hex, "{byte:02x}");
            hex
        });
        assert_eq!(field(entry, "sha256"), Some(digest.into()));
        assert_eq!(field(entry, "bytes"), Some(saved.len().into()));
        if complete {
            assert_eq!(saved, bytes);
        } else {
            assert_eq!(saved.len(), 4_000);
        }
    }
    Ok(())
}

fn response_bodies(exchanges: &[Exchange], directory: &Path) -> TestResult<Vec<SaveBody>> {
    exchanges
        .iter()
        .map(|exchange| {
            SaveBody::new(
                exchange,
                BodySide::Response,
                SaveFormat::AsReceived,
                directory.to_path_buf(),
            )
            .ok_or_else(|| "no response".into())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn merges_overlapping_ranges_in_position_order() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    // Fetched out of order, overlapping, plus a part of another version.
    for range in ["600-999", "0-399", "300-699"] {
        harness.fetch_range("/ranged.mp4", range, "").await?;
    }
    harness
        .fetch_range("/ranged.mp4", "400-499", "X-Version: 2\r\n")
        .await?;
    let exchanges = harness.all_exchanges("/ranged.mp4", 4).await?;
    let bodies = response_bodies(&exchanges, output.path())?;
    let single = bodies.first().ok_or("no body")?;
    let range = single.content_range().ok_or("no range")?;
    assert_eq!(
        (range.start(), range.end(), range.total()),
        (600, 999, Some(1000))
    );
    let saved = harness
        .save(
            "/ranged.mp4",
            BodySide::Response,
            SaveFormat::AsReceived,
            output.path(),
        )
        .await?;
    assert_eq!(saved.path(), output.path().join("ranged.bytes-600-999.mp4"));

    let plan = harness
        .store
        .plan_ranges(single, &bodies)?
        .ok_or("not a range response")?;
    assert_eq!(plan.parts(), 3, "the other version is not merged");
    assert!(plan.is_complete());
    assert_eq!(plan.gaps(), []);
    assert_eq!(plan.validator(), Some("\"v1\""));
    let receiver = harness
        .store
        .save_ranges(plan, output.path().to_path_buf())?;
    let merged =
        tokio::task::spawn_blocking(move || receiver.recv_timeout(TEST_TIMEOUT)).await???;
    assert!(merged.complete());
    assert_eq!(merged.path(), output.path().join("ranged.mp4"));
    assert_eq!(
        std::fs::read(merged.path())?,
        pseudo_random(RANGED_LENGTH, 1)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gaps_between_ranges_are_reported_and_zero_filled() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    for range in ["0-99", "300-399"] {
        harness.fetch_range("/gappy.mp4", range, "").await?;
    }
    let exchanges = harness.all_exchanges("/gappy.mp4", 2).await?;
    let bodies = response_bodies(&exchanges, output.path())?;
    let plan = harness
        .store
        .plan_ranges(bodies.first().ok_or("no body")?, &bodies)?
        .ok_or("not a range response")?;
    assert_eq!(plan.gaps(), [(100, 299), (400, 999)]);
    assert!(!plan.is_complete());
    assert_eq!(plan.file_bytes(), 400);
    assert_eq!(plan.zero_filled_bytes(), 200);
    let reason = plan.incomplete_reason().unwrap_or_default();
    assert!(
        reason.contains("Bytes 100-299, 400-999 were not captured."),
        "{reason}"
    );
    assert!(reason.contains("the file ends at 400 bytes"), "{reason}");
    let receiver = harness
        .store
        .save_ranges(plan, output.path().to_path_buf())?;
    let merged =
        tokio::task::spawn_blocking(move || receiver.recv_timeout(TEST_TIMEOUT)).await???;
    assert!(!merged.complete());
    assert_eq!(merged.path(), output.path().join("gappy.mp4.partial"));
    let resource = pseudo_random(RANGED_LENGTH, 1);
    let mut expected = resource.get(..100).ok_or("short")?.to_vec();
    expected.extend(std::iter::repeat_n(0, 200));
    expected.extend_from_slice(resource.get(300..400).ok_or("short")?);
    assert_eq!(std::fs::read(merged.path())?, expected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merged_files_that_would_fill_the_disk_are_refused() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    harness.fetch_range("/huge.bin", "0-9", "").await?;
    let exchanges = harness.all_exchanges("/huge.bin", 1).await?;
    let bodies = response_bodies(&exchanges, output.path())?;
    let receiver = harness
        .store
        .plan_ranges_in_background(bodies.first().ok_or("no body")?.clone(), bodies.clone())?;
    let plan = tokio::task::spawn_blocking(move || receiver.recv_timeout(TEST_TIMEOUT))
        .await???
        .ok_or("not a range response")?;
    assert_eq!(plan.file_bytes(), 18_446_744_073_709_551_610);
    assert_eq!(plan.zero_filled_bytes(), 18_446_744_073_709_551_600);
    let receiver = harness
        .store
        .save_ranges(plan, output.path().to_path_buf())?;
    let refused =
        tokio::task::spawn_blocking(move || receiver.recv_timeout(TEST_TIMEOUT)).await??;
    assert_eq!(
        refused.err().map(|error| error.kind()),
        Some(io::ErrorKind::StorageFull)
    );
    assert_eq!(
        std::fs::read_dir(output.path())?.count(),
        0,
        "nothing was written"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_shorter_than_its_range_is_saved_as_partial() -> TestResult {
    let mut harness = Harness::start(u64::MAX).await?;
    let output = tempfile::tempdir()?;
    harness.fetch_range("/short.mp4", "0-99", "").await?;
    let saved = harness
        .save(
            "/short.mp4",
            BodySide::Response,
            SaveFormat::AsReceived,
            output.path(),
        )
        .await?;
    assert!(!saved.complete());
    assert_eq!(
        saved.path(),
        output.path().join("short.bytes-0-99.mp4.partial")
    );
    assert!(
        saved
            .incomplete_reason()
            .is_some_and(|reason| reason.contains("50 bytes but Content-Range promises 100"))
    );
    Ok(())
}
