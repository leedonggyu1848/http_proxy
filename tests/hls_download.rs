//! End-to-end integration tests for HLS playlist inspection and MPEG-TS download (#141).

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use http_inspector_core::hls::{
    HlsBlockedReason, HlsDownloadConfig, HlsDownloadError, HlsDownloadOutcome, SegmentSource,
    UnsupportedHlsFeature, inspect_hls_session, is_hls_playlist_record, start_hls_download,
};
use http_inspector_core::interception::{ProxyConfig, ProxyHandle};
use http_inspector_core::recording::BodyStore;
use http_inspector_exchange::{Exchange, Outcome};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::time::timeout;

pub mod common;

use common::{
    ReceivedRequest, TEST_TIMEOUT, TestResult, next_record, read_request, settled,
    wait_for_body_end,
};

const CHROME_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Builds valid MPEG-TS 188-byte packets with sync byte `0x47` and recognizable marker payload.
fn fake_ts_packets(tag: u8, packet_count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(packet_count * 188);
    for seq in 0..packet_count {
        let mut packet = [0u8; 188];
        packet[0] = 0x47;
        packet[1] = 0x40;
        packet[2] = tag;
        packet[3] = 0x10 | u8::try_from(seq % 16).unwrap_or(0);
        for slot in packet.iter_mut().skip(4) {
            *slot = tag.wrapping_add(u8::try_from(seq & 0xff).unwrap_or(0));
        }
        out.extend_from_slice(&packet);
    }
    out
}

fn gzip_bytes(input: &[u8]) -> TestResult<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input)?;
    Ok(encoder.finish()?)
}

fn header_value<'a>(req: &'a ReceivedRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| std::str::from_utf8(v).ok())
}

async fn get_via_proxy(
    proxy: &mut ProxyHandle,
    upstream: SocketAddr,
    path: &str,
    extra_headers: &[(&str, &str)],
) -> TestResult<Exchange> {
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let mut request = format!(
        "GET http://{upstream}{path} HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\n"
    );
    for (name, value) in extra_headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    client.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    let _ = timeout(TEST_TIMEOUT, client.read_to_end(&mut response)).await?;
    let exchange = next_record(proxy).await?;
    let _ = settled(&exchange).await?;
    if let Outcome::Response(resp) = exchange.outcome() {
        let _ = wait_for_body_end(resp.body()).await?;
    }
    Ok(exchange)
}

async fn wait_for_body_store(store: &BodyStore, exchange: &Exchange) -> TestResult<()> {
    let poll = async {
        loop {
            if store
                .body(exchange.id(), http_inspector_exchange::BodySide::Response)
                .is_some_and(|stored| stored.is_complete())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    timeout(TEST_TIMEOUT, poll).await?;
    Ok(())
}

fn start_proxy_with_store() -> TestResult<(TempDir, BodyStore, ProxyHandle)> {
    let root = TempDir::new()?;
    let store = BodyStore::open(root.path())?;
    let proxy = ProxyHandle::start(ProxyConfig::loopback(0).with_body_store(store.clone()))?;
    Ok((root, store, proxy))
}

struct MockRoute {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    content_encoding: Option<&'static str>,
    body: Vec<u8>,
    delay: Duration,
    require_referer: Option<String>,
    forbid_sensitive: bool,
    hold_until: Option<Arc<Notify>>,
    on_hit: Option<Arc<Notify>>,
}

impl MockRoute {
    fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type,
            content_encoding: None,
            body,
            delay: Duration::ZERO,
            require_referer: None,
            forbid_sensitive: false,
            hold_until: None,
            on_hit: None,
        }
    }
}

#[derive(Default)]
struct ServerState {
    hits: HashMap<String, usize>,
    received_headers: HashMap<String, Vec<(String, String)>>,
}

async fn spawn_mock_hls_server(
    routes: HashMap<String, MockRoute>,
) -> TestResult<(SocketAddr, Arc<Mutex<ServerState>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let routes = Arc::new(routes);
    let state = Arc::new(Mutex::new(ServerState::default()));
    let server_state = Arc::clone(&state);

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let routes = Arc::clone(&routes);
            let state = Arc::clone(&server_state);
            tokio::spawn(handle_mock_connection(stream, routes, state));
        }
    });

    Ok((addr, state))
}

async fn handle_mock_connection(
    mut stream: TcpStream,
    routes: Arc<HashMap<String, MockRoute>>,
    state: Arc<Mutex<ServerState>>,
) {
    let Ok(req) = read_request(&mut stream).await else {
        return;
    };
    {
        let mut guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard.hits.entry(req.target.clone()).or_insert(0) += 1;
        let hdrs = req
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned()))
            .collect();
        guard.received_headers.insert(req.target.clone(), hdrs);
    }

    let Some(route) = routes.get(&req.target) else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    if let Some(on_hit) = &route.on_hit {
        on_hit.notify_waiters();
    }
    if route.forbid_sensitive && request_has_sensitive_header(&req) {
        let _ = stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return;
    }
    if let Some(expected_referer) = &route.require_referer
        && header_value(&req, "referer") != Some(expected_referer.as_str())
    {
        let _ = stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    if let Some(hold) = &route.hold_until {
        hold.notified().await;
    }
    if !route.delay.is_zero() {
        tokio::time::sleep(route.delay).await;
    }

    let encoding_header = route
        .content_encoding
        .map_or_else(String::new, |enc| format!("Content-Encoding: {enc}\r\n"));
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        route.status,
        route.reason,
        route.content_type,
        encoding_header,
        route.body.len(),
    );
    if stream.write_all(head.as_bytes()).await.is_ok() {
        let _ = stream.write_all(&route.body).await;
    }
}

fn request_has_sensitive_header(req: &ReceivedRequest) -> bool {
    req.headers.iter().any(|(name, value)| {
        let val = String::from_utf8_lossy(value);
        name.eq_ignore_ascii_case("cookie")
            || name.eq_ignore_ascii_case("authorization")
            || name.eq_ignore_ascii_case("proxy-authorization")
            || val.contains("[REDACTED]")
    })
}

fn gating_routes() -> HashMap<String, MockRoute> {
    let mut routes = HashMap::new();
    routes.insert(
        "/live/master.m3u8".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n\
              #EXT-X-STREAM-INF:BANDWIDTH=2400000,RESOLUTION=1280x720\n\
              720p/index.m3u8\n\
              #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n\
              360p/index.m3u8\n"
                .to_vec(),
        ),
    );
    let media_body = b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
    routes.insert(
        "/live/720p/index.m3u8".to_owned(),
        MockRoute::ok("application/vnd.apple.mpegurl", media_body.to_vec()),
    );
    routes.insert(
        "/live/360p/index.m3u8".to_owned(),
        MockRoute::ok("application/vnd.apple.mpegurl", media_body.to_vec()),
    );
    routes.insert(
        "/other/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0xaa, 2)),
    );
    routes.insert(
        "/live/720p/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x70, 2)),
    );
    routes.insert(
        "/live/360p/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x36, 2)),
    );
    routes
}

/// Criterion 1: Only playlist captured, other stream's `.ts`, other quality's `.ts`, or non-Chrome `.ts`
/// keeps download disabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disables_download_without_matching_chrome_ts_request() -> TestResult {
    let (upstream, _state) = spawn_mock_hls_server(gating_routes()).await?;
    let (_store_root, store, mut proxy) = start_proxy_with_store()?;
    let chrome = [("User-Agent", CHROME_UA)];

    let master = get_via_proxy(&mut proxy, upstream, "/live/master.m3u8", &chrome).await?;
    assert!(is_hls_playlist_record(&master));
    let m720 = get_via_proxy(&mut proxy, upstream, "/live/720p/index.m3u8", &chrome).await?;
    let m360 = get_via_proxy(&mut proxy, upstream, "/live/360p/index.m3u8", &chrome).await?;
    let other_ts = get_via_proxy(&mut proxy, upstream, "/other/seg0.ts", &chrome).await?;
    let curl_720 = get_via_proxy(
        &mut proxy,
        upstream,
        "/live/720p/seg0.ts",
        &[("User-Agent", "curl/8.7.1")],
    )
    .await?;

    let records = vec![
        master.clone(),
        m720.clone(),
        m360.clone(),
        other_ts.clone(),
        curl_720.clone(),
    ];
    let plan = inspect_hls_session(&master, &records, Some(&store))?;
    assert!(plan.is_master);
    assert_eq!(plan.variants.len(), 2);
    for variant in &plan.variants {
        assert!(!variant.can_download());
        assert_eq!(
            variant.blocked_reason(),
            Some(HlsBlockedReason::MissingChromeSegmentRequest)
        );
    }

    // Now capture a Chrome .ts request for 360p only: 360p unlocks, 720p stays disabled.
    let chrome_360_ts = get_via_proxy(&mut proxy, upstream, "/live/360p/seg0.ts", &chrome).await?;
    let records_with_360 = vec![
        master.clone(),
        m720,
        m360,
        other_ts,
        curl_720,
        chrome_360_ts,
    ];
    let plan = inspect_hls_session(&master, &records_with_360, Some(&store))?;
    let v720 = plan.variants.first().ok_or("missing 720p")?;
    let v360 = plan.variants.get(1).ok_or("missing 360p")?;
    assert!(!v720.can_download());
    assert_eq!(
        v720.blocked_reason(),
        Some(HlsBlockedReason::MissingChromeSegmentRequest)
    );
    assert!(v360.can_download());
    assert_eq!(plan.default_variant, 1);

    proxy.stop()?;
    Ok(())
}

struct MergeFixture {
    routes: HashMap<String, MockRoute>,
    expected_referer: String,
    expected_bytes: Vec<u8>,
}

fn build_merge_fixture() -> TestResult<MergeFixture> {
    let expected_referer = "https://player.example.test/watch?v=42".to_owned();
    let seg0 = fake_ts_packets(0x10, 120);
    let seg1 = fake_ts_packets(0x20, 40);
    let seg2 = fake_ts_packets(0x30, 40);
    let seg3 = fake_ts_packets(0x40, 40);
    let seg3_gzip = gzip_bytes(&seg3)?;

    let mut routes = HashMap::new();
    routes.insert(
        "/streams/live/master.m3u8?v=1".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720\n720p/index.m3u8?tok=m\n"
                .to_vec(),
        ),
    );
    routes.insert(
        "/streams/live/720p/index.m3u8?tok=m".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg0.ts?k=0\n#EXTINF:4.0,\n../shared/seg1.ts?k=1\n#EXTINF:4.0,\n/abs/seg2.ts?k=2\n#EXTINF:4.0,\nseg3.ts?k=3\n#EXT-X-ENDLIST\n"
                .to_vec(),
        ),
    );
    routes.insert(
        "/streams/live/720p/seg0.ts?k=0".to_owned(),
        MockRoute::ok("video/mp2t", seg0.clone()),
    );
    let mut r1 = MockRoute::ok("video/mp2t", seg1.clone());
    r1.delay = Duration::from_millis(180);
    r1.require_referer = Some(expected_referer.clone());
    r1.forbid_sensitive = true;
    routes.insert("/streams/live/shared/seg1.ts?k=1".to_owned(), r1);

    let mut r2 = MockRoute::ok("video/mp2t", seg2.clone());
    r2.delay = Duration::from_millis(80);
    r2.require_referer = Some(expected_referer.clone());
    r2.forbid_sensitive = true;
    routes.insert("/abs/seg2.ts?k=2".to_owned(), r2);

    let mut r3 = MockRoute::ok("video/mp2t", seg3_gzip);
    r3.content_encoding = Some("gzip");
    r3.require_referer = Some(expected_referer.clone());
    r3.forbid_sensitive = true;
    routes.insert("/streams/live/720p/seg3.ts?k=3".to_owned(), r3);

    let mut expected_bytes = Vec::new();
    expected_bytes.extend_from_slice(&seg0);
    expected_bytes.extend_from_slice(&seg1);
    expected_bytes.extend_from_slice(&seg2);
    expected_bytes.extend_from_slice(&seg3);
    Ok(MergeFixture {
        routes,
        expected_referer,
        expected_bytes,
    })
}

fn assert_safe_fetch_headers(
    segments: &[http_inspector_core::hls::SegmentPlan],
    expected_referer: &str,
) -> TestResult {
    for seg in segments {
        let SegmentSource::Fetch { headers } = &seg.source else {
            return Err("expected fetch source for missing segment".into());
        };
        assert_eq!(headers.get("referer"), Some(expected_referer));
        assert_eq!(headers.get("user-agent"), Some(CHROME_UA));
        assert!(headers.get("cookie").is_none());
        assert!(headers.get("authorization").is_none());
    }
    Ok(())
}

/// Criteria 2 & 3:
/// - Single Chrome `.ts` capture enables download and forwards safe `Referer` (never `Cookie`/`Authorization`).
/// - Cached large segment stored in `BodyStore` is reused without re-requesting from server.
/// - Relative URLs (`720p/index.m3u8`, `../shared/seg1.ts`, `/abs/seg2.ts`) and out-of-order + gzip responses
///   merge in exact playlist order into a playable `.ts` file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_with_referer_reuses_cached_and_merges_out_of_order_relative_segments()
-> TestResult {
    let fixture = build_merge_fixture()?;
    let (upstream, server_state) = spawn_mock_hls_server(fixture.routes).await?;
    let (_store_root, store, mut proxy) = start_proxy_with_store()?;
    let chrome = [("User-Agent", CHROME_UA)];

    let master = get_via_proxy(
        &mut proxy,
        upstream,
        "/streams/live/master.m3u8?v=1",
        &chrome,
    )
    .await?;
    let media = get_via_proxy(
        &mut proxy,
        upstream,
        "/streams/live/720p/index.m3u8?tok=m",
        &chrome,
    )
    .await?;
    let seg0 = get_via_proxy(
        &mut proxy,
        upstream,
        "/streams/live/720p/seg0.ts?k=0",
        &[
            ("User-Agent", CHROME_UA),
            ("Referer", &fixture.expected_referer),
            ("Origin", "https://player.example.test"),
            ("Cookie", "session=topsecret"),
            ("Authorization", "Bearer supersecret"),
        ],
    )
    .await?;
    wait_for_body_store(&store, &seg0).await?;

    let records = vec![master.clone(), media, seg0];
    let session_plan = inspect_hls_session(&master, &records, Some(&store))?;
    let variant = session_plan.variants.first().ok_or("missing variant")?;
    assert!(variant.can_download());
    let media_plan = variant.media().ok_or("missing media plan")?.clone();
    assert_eq!((media_plan.cached_count, media_plan.missing_count), (1, 3));
    assert_safe_fetch_headers(
        media_plan.segments.get(1..).unwrap_or_default(),
        &fixture.expected_referer,
    )?;

    let out_dir = TempDir::new()?;
    let dl_config = HlsDownloadConfig::new(media_plan, out_dir.path().to_path_buf(), Some(store));
    let handle = start_hls_download(dl_config, || {})?;
    let outcome = tokio::task::spawn_blocking(move || handle.wait()).await?;

    let HlsDownloadOutcome::Completed(summary) = outcome else {
        return Err(format!("expected Completed outcome, got {outcome:?}").into());
    };
    assert_eq!(
        (
            summary.total_segments,
            summary.reused_segments,
            summary.downloaded_segments,
            summary.total_bytes,
            summary.path.extension().and_then(|ext| ext.to_str()),
        ),
        (4, 1, 3, fixture.expected_bytes.len() as u64, Some("ts")),
    );
    assert_eq!(std::fs::read(&summary.path)?, fixture.expected_bytes);

    let guard = server_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for path in [
        "/streams/live/720p/seg0.ts?k=0",
        "/streams/live/shared/seg1.ts?k=1",
        "/abs/seg2.ts?k=2",
        "/streams/live/720p/seg3.ts?k=3",
    ] {
        assert_eq!(
            guard.hits.get(path),
            Some(&1),
            "unexpected hit count for {path}"
        );
    }
    drop(guard);

    proxy.stop()?;
    Ok(())
}

/// Criterion 4a: A `403 Forbidden` on a missing segment reports the status and segment,
/// and never writes a completed `.ts` file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_403_failure_and_leaves_no_output_file() -> TestResult {
    let mut routes = HashMap::new();
    routes.insert(
        "/v/index.m3u8".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
        ),
    );
    routes.insert(
        "/v/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x01, 4)),
    );
    let mut forbidden = MockRoute::ok("text/plain", b"denied".to_vec());
    forbidden.status = 403;
    forbidden.reason = "Forbidden";
    routes.insert("/v/seg1.ts".to_owned(), forbidden);
    routes.insert(
        "/v/seg2.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x03, 4)),
    );

    let (upstream, _state) = spawn_mock_hls_server(routes).await?;
    let (_store_root, store, mut proxy) = start_proxy_with_store()?;

    let media = get_via_proxy(
        &mut proxy,
        upstream,
        "/v/index.m3u8",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    let seg0 = get_via_proxy(
        &mut proxy,
        upstream,
        "/v/seg0.ts",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    wait_for_body_store(&store, &seg0).await?;

    let records = vec![media.clone(), seg0];
    let session_plan = inspect_hls_session(&media, &records, Some(&store))?;
    let media_plan = session_plan
        .variants
        .first()
        .and_then(|v| v.media())
        .ok_or("missing media plan")?
        .clone();

    let out_dir = TempDir::new()?;
    let handle = start_hls_download(
        HlsDownloadConfig::new(media_plan, out_dir.path().to_path_buf(), Some(store)),
        || {},
    )?;
    let outcome = tokio::task::spawn_blocking(move || handle.wait()).await?;

    match outcome {
        HlsDownloadOutcome::Failed(HlsDownloadError::HttpStatus {
            status,
            segment_number,
            segment_name,
            ..
        }) => {
            assert_eq!(status, 403);
            assert_eq!(segment_number, 2);
            assert_eq!(segment_name, "seg1.ts");
        }
        other => return Err(format!("expected 403 HttpStatus failure, got {other:?}").into()),
    }

    let entries: Vec<_> = std::fs::read_dir(out_dir.path())?.collect();
    assert!(
        entries.is_empty(),
        "failed download must not leave any output or temp files"
    );

    proxy.stop()?;
    Ok(())
}

/// Criterion 4b: Cancelling an in-flight download stops promptly and leaves no output file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancels_in_flight_download_and_leaves_no_output_file() -> TestResult {
    let seg1_started = Arc::new(Notify::new());
    let seg1_release = Arc::new(Notify::new());

    let mut routes = HashMap::new();
    routes.insert(
        "/cancel/index.m3u8".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n".to_vec(),
        ),
    );
    routes.insert(
        "/cancel/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x01, 4)),
    );
    let mut slow_seg1 = MockRoute::ok("video/mp2t", fake_ts_packets(0x02, 4));
    slow_seg1.on_hit = Some(Arc::clone(&seg1_started));
    slow_seg1.hold_until = Some(Arc::clone(&seg1_release));
    routes.insert("/cancel/seg1.ts".to_owned(), slow_seg1);

    let (upstream, _state) = spawn_mock_hls_server(routes).await?;
    let (_store_root, store, mut proxy) = start_proxy_with_store()?;

    let media = get_via_proxy(
        &mut proxy,
        upstream,
        "/cancel/index.m3u8",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    let seg0 = get_via_proxy(
        &mut proxy,
        upstream,
        "/cancel/seg0.ts",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    wait_for_body_store(&store, &seg0).await?;

    let records = vec![media.clone(), seg0];
    let session_plan = inspect_hls_session(&media, &records, Some(&store))?;
    let media_plan = session_plan
        .variants
        .first()
        .and_then(|v| v.media())
        .ok_or("missing media plan")?
        .clone();

    let out_dir = TempDir::new()?;
    let handle = start_hls_download(
        HlsDownloadConfig::new(media_plan, out_dir.path().to_path_buf(), Some(store)),
        || {},
    )?;

    timeout(TEST_TIMEOUT, seg1_started.notified()).await?;
    handle.cancel();
    let outcome = timeout(
        TEST_TIMEOUT,
        tokio::task::spawn_blocking(move || handle.wait()),
    )
    .await??;
    seg1_release.notify_waiters();

    assert_eq!(outcome, HlsDownloadOutcome::Cancelled);
    let entries: Vec<_> = std::fs::read_dir(out_dir.path())?.collect();
    assert!(
        entries.is_empty(),
        "cancelled download must not leave any output or temp files"
    );

    proxy.stop()?;
    Ok(())
}

/// Unsupported HLS features (AES-128 encryption, fMP4 init map) are blocked explicitly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocks_encrypted_and_fmp4_playlists() -> TestResult {
    let mut routes = HashMap::new();
    routes.insert(
        "/enc/index.m3u8".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:4.0,\nseg0.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
        ),
    );
    routes.insert(
        "/fmp4/index.m3u8".to_owned(),
        MockRoute::ok(
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg0.m4s\n#EXT-X-ENDLIST\n"
                .to_vec(),
        ),
    );
    routes.insert(
        "/enc/seg0.ts".to_owned(),
        MockRoute::ok("video/mp2t", fake_ts_packets(0x05, 2)),
    );

    let (upstream, _state) = spawn_mock_hls_server(routes).await?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;

    let enc_playlist = get_via_proxy(
        &mut proxy,
        upstream,
        "/enc/index.m3u8",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    let enc_seg = get_via_proxy(
        &mut proxy,
        upstream,
        "/enc/seg0.ts",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;
    let fmp4_playlist = get_via_proxy(
        &mut proxy,
        upstream,
        "/fmp4/index.m3u8",
        &[("User-Agent", CHROME_UA)],
    )
    .await?;

    let enc_plan = inspect_hls_session(&enc_playlist, &[enc_playlist.clone(), enc_seg], None)?;
    let enc_variant = enc_plan.variants.first().ok_or("missing enc variant")?;
    assert!(!enc_variant.can_download());
    assert_eq!(
        enc_variant.blocked_reason(),
        Some(HlsBlockedReason::Unsupported(
            UnsupportedHlsFeature::Encrypted("AES-128".to_owned())
        ))
    );

    let fmp4_plan =
        inspect_hls_session(&fmp4_playlist, std::slice::from_ref(&fmp4_playlist), None)?;
    let fmp4_variant = fmp4_plan.variants.first().ok_or("missing fmp4 variant")?;
    assert!(!fmp4_variant.can_download());
    assert_eq!(
        fmp4_variant.blocked_reason(),
        Some(HlsBlockedReason::Unsupported(
            UnsupportedHlsFeature::InitMap
        ))
    );

    proxy.stop()?;
    Ok(())
}
