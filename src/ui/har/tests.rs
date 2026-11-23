//! HAR export tests against exchanges captured by a real proxy, and a check
//! that fixtures built without a proxy export, search, and map the same way.
#![expect(
    clippy::indexing_slicing,
    reason = "Reading a serde_json::Value by key yields Null instead of panicking."
)]

use std::error::Error;
use std::io::Write as _;
use std::time::{Duration, SystemTime};

use http::header::{
    AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, PROXY_AUTHORIZATION,
    SET_COOKIE,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use http_proxy::text_search::{MatchOptions, Matcher};
use http_proxy::url_map::{UrlMap, export_graph_mermaid, export_tree_json};
use http_proxy::{
    BodyLifecycle, Exchange, ExchangeBuilder, Outcome, ProxyConfig, ProxyHandle, ResponseFixture,
    TunnelState,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use super::{HarOptions, document, write};
use crate::ui::search::SearchScope;
use crate::ui::search::document::{BodyCache, Coverage, check};

type TestResult = Result<(), Box<dyn Error>>;

const SECRETS: [&str; 4] = [
    "secret-authorization-token",
    "secret-proxy-credentials",
    "secret-session-cookie",
    "secret-set-cookie",
];
const REQUEST_BODY: &str = "name=request-body-text";
const RESPONSE_BODY: &str = "{\"answer\":\"response-body-text\"}";

/// Sends `request` through the proxy, answers it with `response`, and
/// returns the settled capture once nothing more can change.
async fn exchange(
    proxy: &mut ProxyHandle,
    request: impl FnOnce(std::net::SocketAddr) -> String,
    response: &[u8],
) -> Result<Exchange, Box<dyn Error>> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client.write_all(request(address).as_bytes()).await?;
    let (mut server, _) = upstream.accept().await?;
    read_request(&mut server).await?;
    server.write_all(response).await?;
    drop(server);
    let mut bytes = Vec::new();
    client.read_to_end(&mut bytes).await?;
    let record = proxy.receive().await.ok_or("missing capture")?;
    wait_until_ended(&record).await;
    Ok(record)
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        head.extend_from_slice(&byte);
    }
    Ok(head)
}

/// Reads one request head and its `Content-Length` body.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let head = read_head(stream).await?;
    let length = String::from_utf8_lossy(&head)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or_default();
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.map(drop)
}

async fn wait_until_ended(record: &Exchange) {
    while record.is_live() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn sensitive_request(address: std::net::SocketAddr) -> String {
    format!(
        "POST http://{address}/login?user=alice&empty HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nProxy-Authorization: Basic {}\r\nCookie: session={}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{REQUEST_BODY}",
        SECRETS[0],
        SECRETS[1],
        SECRETS[2],
        REQUEST_BODY.len(),
    )
}

fn sensitive_response() -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: id={}; HttpOnly\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{RESPONSE_BODY}",
        SECRETS[3],
        RESPONSE_BODY.len(),
    )
    .into_bytes()
}

fn entries(document: &Value) -> &[Value] {
    document["log"]["entries"]
        .as_array()
        .map_or(&[], Vec::as_slice)
}

fn header_names(headers: &Value) -> Vec<String> {
    headers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|header| header["name"].as_str())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn header_value<'a>(headers: &'a Value, name: &str) -> Option<&'a str> {
    headers.as_array()?.iter().find_map(|header| {
        header["name"]
            .as_str()?
            .eq_ignore_ascii_case(name)
            .then(|| header["value"].as_str())?
    })
}

fn run<F: Future<Output = TestResult>>(test: impl FnOnce() -> F) -> TestResult {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async { timeout(Duration::from_secs(10), test()).await? })
}

#[test]
fn default_export_leaves_out_sensitive_headers_and_bodies() -> TestResult {
    run(|| async {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let record = exchange(&mut proxy, sensitive_request, &sensitive_response()).await?;
        proxy.stop()?;

        let har = document(&[record], HarOptions::default(), SystemTime::now());
        let text = serde_json::to_string(&har)?;
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} leaked");
        }
        assert!(!text.contains("[REDACTED]"));
        assert!(!text.contains("request-body-text"));
        assert!(!text.contains("response-body-text"));
        let entry = entries(&har).first().ok_or("missing entry")?;
        for headers in [&entry["request"]["headers"], &entry["response"]["headers"]] {
            let names = header_names(headers);
            assert_ne!(names.len(), 0);
            for name in super::SENSITIVE_HEADERS {
                assert!(
                    !names.iter().any(|header| header == name),
                    "{name} exported"
                );
            }
        }
        assert!(entry["request"].get("postData").is_none());
        assert!(entry["response"]["content"].get("text").is_none());
        assert_eq!(entry["response"]["content"]["size"], RESPONSE_BODY.len());
        assert_eq!(entry["request"]["cookies"], serde_json::json!([]));
        assert_eq!(entry["response"]["cookies"], serde_json::json!([]));
        assert_har_1_2(&har);
        Ok(())
    })
}

#[test]
fn opt_in_adds_bodies_and_redacted_header_names_only() -> TestResult {
    run(|| async {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let record = exchange(&mut proxy, sensitive_request, &sensitive_response()).await?;
        proxy.stop()?;

        let options = HarOptions {
            include_bodies: true,
            include_sensitive_headers: true,
        };
        let har = document(&[record], options, SystemTime::now());
        let text = serde_json::to_string(&har)?;
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} leaked");
        }
        let entry = entries(&har).first().ok_or("missing entry")?;
        let request = &entry["request"];
        for name in ["authorization", "cookie"] {
            assert_eq!(header_value(&request["headers"], name), Some("[REDACTED]"));
        }
        assert_eq!(
            header_value(&entry["response"]["headers"], "set-cookie"),
            Some("[REDACTED]")
        );
        assert_eq!(request["postData"]["text"], REQUEST_BODY);
        assert_eq!(
            request["postData"]["mimeType"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(request["bodySize"], REQUEST_BODY.len());
        assert_eq!(
            request["queryString"],
            serde_json::json!([
                { "name": "user", "value": "alice" },
                { "name": "empty", "value": "" },
            ])
        );
        let content = &entry["response"]["content"];
        assert_eq!(content["text"], RESPONSE_BODY);
        assert_eq!(content["mimeType"], "application/json");
        assert!(content.get("comment").is_none());
        assert_har_1_2(&har);
        Ok(())
    })
}

#[test]
fn truncated_binary_and_compressed_bodies_are_marked() -> TestResult {
    run(|| async {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let get = |address| {
            format!(
                "GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            )
        };
        let large = "a".repeat(http_proxy::BODY_PREVIEW_BYTES + 100);
        let truncated = exchange(
            &mut proxy,
            get,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{large}",
                large.len()
            )
            .as_bytes(),
        )
        .await?;
        let mut binary_response =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\n\r\n"
                .to_vec();
        binary_response.extend_from_slice(&[0xff, 0x00, 0x80]);
        let binary = exchange(&mut proxy, get, &binary_response).await?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"decoded text")?;
        let gzip = encoder.finish()?;
        let mut gzip_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            gzip.len()
        )
        .into_bytes();
        gzip_response.extend_from_slice(&gzip);
        let compressed = exchange(&mut proxy, get, &gzip_response).await?;
        proxy.stop()?;

        let options = HarOptions {
            include_bodies: true,
            include_sensitive_headers: false,
        };
        let har = document(&[truncated, binary, compressed], options, SystemTime::now());
        let [truncated, binary, compressed] = entries(&har) else {
            return Err("expected three entries".into());
        };
        let content = &truncated["response"]["content"];
        assert_eq!(
            content["text"].as_str().map(str::len),
            Some(http_proxy::BODY_PREVIEW_BYTES)
        );
        assert_eq!(content["size"], large.len());
        assert!(
            content["comment"]
                .as_str()
                .is_some_and(|comment| comment.starts_with("Truncated: only the first 8192 of"))
        );
        let content = &binary["response"]["content"];
        assert_eq!(content["text"], "/wCA");
        assert_eq!(content["encoding"], "base64");
        let content = &compressed["response"]["content"];
        assert_eq!(content["text"], "decoded text");
        assert_eq!(content["size"], 12);
        assert_eq!(compressed["response"]["bodySize"], gzip.len());
        assert_eq!(content["compression"], gzip.len() - 12);
        assert_har_1_2(&har);
        Ok(())
    })
}

#[test]
fn failures_tunnels_and_pending_requests_follow_the_documented_mapping() -> TestResult {
    run(|| async {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;

        let mut tunnel_client = TcpStream::connect(proxy.local_addr()).await?;
        tunnel_client
            .write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .await?;
        let (mut tunnel_server, _) = upstream.accept().await?;
        let tunnel = proxy.receive().await.ok_or("missing tunnel")?;
        let _reply = read_head(&mut tunnel_client).await?;
        tunnel_client.write_all(b"ping").await?;
        let mut ping = [0; 4];
        tunnel_server.read_exact(&mut ping).await?;
        tunnel_server.write_all(b"pong!").await?;
        drop(tunnel_server);
        let mut rest = Vec::new();
        let _read = tunnel_client.read_to_end(&mut rest).await?;
        drop(tunnel_client);
        wait_until_ended(&tunnel).await;

        let mut pending_client = TcpStream::connect(proxy.local_addr()).await?;
        pending_client
            .write_all(
                format!("GET http://{address}/slow HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
            )
            .await?;
        let (_pending_server, _) = upstream.accept().await?;
        let pending = proxy.receive().await.ok_or("missing pending request")?;

        drop(upstream);
        let mut failed_client = TcpStream::connect(proxy.local_addr()).await?;
        failed_client
            .write_all(
                format!(
                    "GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let failed = proxy.receive().await.ok_or("missing failure")?;
        wait_until_ended(&failed).await;

        let har = document(
            &[tunnel, pending, failed],
            HarOptions::default(),
            SystemTime::now(),
        );
        proxy.stop()?;
        assert_har_1_2(&har);
        let [tunnel, pending, failed] = entries(&har) else {
            return Err("expected three entries".into());
        };
        assert_eq!(tunnel["request"]["method"], "CONNECT");
        assert_eq!(tunnel["request"]["url"], format!("https://{address}"));
        assert_eq!(tunnel["request"]["bodySize"], 4);
        assert_eq!(tunnel["response"]["status"], 200);
        assert_eq!(tunnel["response"]["bodySize"], 5);
        assert!(
            tunnel["comment"]
                .as_str()
                .is_some_and(|comment| comment.starts_with("CONNECT tunnel to"))
        );
        assert_eq!(pending["response"]["status"], 0);
        assert!(pending["response"].get("_error").is_none());
        assert!(
            pending["comment"]
                .as_str()
                .is_some_and(|comment| comment.contains("Still waiting"))
        );
        assert_eq!(failed["response"]["status"], 0);
        assert!(
            failed["response"]["_error"]
                .as_str()
                .is_some_and(|error| error.starts_with("Connection"))
        );
        Ok(())
    })
}

/// Request headers in the order `sensitive_request` sends them.
fn sensitive_request_headers(address: std::net::SocketAddr) -> Vec<(HeaderName, String)> {
    vec![
        (HOST, address.to_string()),
        (AUTHORIZATION, format!("Bearer {}", SECRETS[0])),
        (PROXY_AUTHORIZATION, format!("Basic {}", SECRETS[1])),
        (COOKIE, format!("session={}", SECRETS[2])),
        (CONTENT_TYPE, "application/x-www-form-urlencoded".to_owned()),
        (CONTENT_LENGTH, REQUEST_BODY.len().to_string()),
        (CONNECTION, "close".to_owned()),
    ]
}

/// Response headers in the order `sensitive_response` sends them.
fn sensitive_response_headers() -> Vec<(HeaderName, String)> {
    vec![
        (CONTENT_TYPE, "application/json".to_owned()),
        (SET_COOKIE, format!("id={}; HttpOnly", SECRETS[3])),
        (CONTENT_LENGTH, RESPONSE_BODY.len().to_string()),
        (CONNECTION, "close".to_owned()),
    ]
}

fn with_headers<T>(
    target: T,
    headers: Vec<(HeaderName, String)>,
    add: impl Fn(T, HeaderName, HeaderValue) -> T,
) -> Result<T, Box<dyn Error>> {
    headers
        .into_iter()
        .try_fold(target, |target, (name, value)| {
            Ok(add(target, name, HeaderValue::try_from(value)?))
        })
}

/// Relays four bytes up and five bytes down through a tunnel that both
/// sides then close.
async fn tunnel_capture(
    proxy: &mut ProxyHandle,
    upstream: &TcpListener,
) -> Result<Exchange, Box<dyn Error>> {
    let address = upstream.local_addr()?;
    let mut tunnel_client = TcpStream::connect(proxy.local_addr()).await?;
    tunnel_client
        .write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await?;
    let (mut tunnel_server, _) = upstream.accept().await?;
    let tunnel = proxy.receive().await.ok_or("missing tunnel")?;
    let _reply = read_head(&mut tunnel_client).await?;
    tunnel_client.write_all(b"ping").await?;
    let mut ping = [0; 4];
    tunnel_server.read_exact(&mut ping).await?;
    tunnel_server.write_all(b"pong!").await?;
    drop(tunnel_server);
    let _read = tunnel_client.read_to_end(&mut Vec::new()).await?;
    drop(tunnel_client);
    wait_until_ended(&tunnel).await;
    Ok(tunnel)
}

/// Records that a running proxy captured, with the connections that keep
/// the waiting request in flight.
struct Captures {
    proxy: ProxyHandle,
    address: std::net::SocketAddr,
    records: Vec<Exchange>,
    _waiting: [TcpStream; 2],
}

/// Captures, through a real proxy, a response, a closed tunnel, a request
/// still waiting for its response, and a connection failure. The proxy keeps
/// running so the waiting request stays in flight.
async fn proxy_captures() -> Result<Captures, Box<dyn Error>> {
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let response = exchange(&mut proxy, sensitive_request, &sensitive_response()).await?;
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;

    let tunnel = tunnel_capture(&mut proxy, &upstream).await?;

    let mut pending_client = TcpStream::connect(proxy.local_addr()).await?;
    pending_client
        .write_all(
            format!("GET http://{address}/slow HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
        )
        .await?;
    let (pending_server, _) = upstream.accept().await?;
    let pending = proxy.receive().await.ok_or("missing pending request")?;
    // The upstream accepting the connection can race the proxy recording
    // it, so wait until the request is forwarded as the fixture describes.
    while pending
        .request()
        .body()
        .is_none_or(|body| body.lifecycle() == BodyLifecycle::Streaming)
    {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    drop(upstream);
    let mut failed_client = TcpStream::connect(proxy.local_addr()).await?;
    failed_client
        .write_all(
            format!(
                "GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let failed = proxy.receive().await.ok_or("missing failure")?;
    wait_until_ended(&failed).await;
    Ok(Captures {
        proxy,
        address,
        records: vec![response, tunnel, pending, failed],
        _waiting: [pending_client, pending_server],
    })
}

/// Builds, without a proxy, the same four records that `proxy_captures`
/// captured. Only the server address and the operating system's connection
/// error text are taken from the captures.
fn fixtures(
    response_address: std::net::SocketAddr,
    address: std::net::SocketAddr,
    failure: &Outcome,
) -> Result<Vec<Exchange>, Box<dyn Error>> {
    let Outcome::Failed { stage, message } = failure else {
        return Err("expected a failed capture".into());
    };
    let response = with_headers(
        ExchangeBuilder::http(
            Method::POST,
            format!("http://{response_address}/login?user=alice&empty"),
        )?,
        sensitive_request_headers(response_address),
        ExchangeBuilder::header,
    )?
    .request_body(REQUEST_BODY, BodyLifecycle::Complete)
    .respond(
        with_headers(
            ResponseFixture::new(StatusCode::OK),
            sensitive_response_headers(),
            ResponseFixture::header,
        )?
        .body(RESPONSE_BODY, BodyLifecycle::Complete),
    )?;
    let host = HeaderValue::try_from(address.to_string())?;
    let tunnel = ExchangeBuilder::connect(address.to_string())
        .header(HOST, host.clone())
        .tunnel(4, 5, TunnelState::Closed)?;
    let pending = ExchangeBuilder::http(Method::GET, format!("http://{address}/slow"))?
        .header(HOST, host.clone())
        .request_body(Vec::new(), BodyLifecycle::Complete)
        .in_flight()?;
    let failed = ExchangeBuilder::http(Method::GET, format!("http://{address}/"))?
        .header(HOST, host)
        .header(CONNECTION, HeaderValue::from_static("close"))
        .fail(*stage, message)?;
    Ok(vec![response, tunnel, pending, failed])
}

/// Replaces values that depend on when and how fast the exchange ran with
/// whether they were observed, keeping everything else.
fn without_clock(mut har: Value) -> Value {
    if let Some(entries) = har["log"]["entries"].as_array_mut() {
        for entry in entries {
            let Some(fields) = entry.as_object_mut() else {
                continue;
            };
            let _ = fields.remove("startedDateTime");
            let _ = fields.remove("time");
            if let Some(Value::Object(timings)) = fields.get_mut("timings") {
                for value in timings.values_mut() {
                    *value = Value::Bool(value.as_f64().is_some_and(|value| value >= 0.0));
                }
            }
        }
    }
    har
}

/// Leaves out URL map statistics that record when requests ran and how
/// long they took.
fn without_clock_stats(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for clock in [
                "avg_duration_ms",
                "max_duration_ms",
                "first_seen_unix_ms",
                "last_seen_unix_ms",
            ] {
                let _ = fields.remove(clock);
            }
            fields.values_mut().for_each(without_clock_stats);
        }
        Value::Array(items) => items.iter_mut().for_each(without_clock_stats),
        _ => {}
    }
}

fn url_map(records: &[Exchange]) -> (Value, String) {
    let mut map = UrlMap::new();
    for record in records {
        let _ = map.observe_exchange(record);
    }
    let _ = map.sync_live();
    let mut tree = export_tree_json(&map, true);
    without_clock_stats(&mut tree);
    (tree, export_graph_mermaid(&map, 16))
}

/// Whether a record matched, whether that is final, and why it may not have.
type SearchResult = (bool, bool, Coverage);

fn search_results(records: &[Exchange]) -> Result<Vec<SearchResult>, Box<dyn Error>> {
    let mut results = Vec::new();
    let mut cache = BodyCache::default();
    for query in [
        "POST",
        "login",
        "request-body-text",
        "response-body-text",
        "application/json",
        "[REDACTED]",
        SECRETS[0],
        "200",
        "CONNECT",
        "slow",
        "Failed",
    ] {
        let matcher = Matcher::new(query, MatchOptions::default())?;
        for scope in SearchScope::ALL {
            for record in records {
                let checked = check(record, &matcher, scope, &mut cache);
                results.push((checked.matched, checked.settled, checked.coverage));
            }
        }
    }
    Ok(results)
}

#[test]
fn builder_records_export_search_and_map_like_proxy_captures() -> TestResult {
    run(|| async {
        let Captures {
            proxy,
            address,
            records: captured,
            _waiting,
        } = proxy_captures().await?;
        let [response, _, _, failed] = captured.as_slice() else {
            return Err("expected four captures".into());
        };
        let response_address = response
            .request()
            .target()
            .strip_prefix("http://")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(authority, _)| authority.parse().ok())
            .ok_or("missing response address")?;
        let built = fixtures(response_address, address, failed.outcome())?;

        let now = SystemTime::now();
        for options in [
            HarOptions::default(),
            HarOptions {
                include_bodies: true,
                include_sensitive_headers: true,
            },
        ] {
            assert_eq!(
                without_clock(document(&built, options, now)),
                without_clock(document(&captured, options, now)),
            );
        }
        assert_eq!(search_results(&built)?, search_results(&captured)?);
        assert_eq!(url_map(&built), url_map(&captured));
        proxy.stop()?;
        Ok(())
    })
}

#[test]
fn write_creates_a_private_file_and_never_replaces_one() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("export.har");
    let har = document(&[], HarOptions::default(), SystemTime::now());
    assert_eq!(write(&path, &har)?, path);
    let written: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(written, har);
    assert_har_1_2(&written);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o077, 0);
    }
    let error = write(&path, &serde_json::json!({}))
        .err()
        .ok_or("replaced")?;
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path)?)?,
        har
    );
    let missing = directory.path().join("missing").join("export.har");
    assert!(write(&missing, &har).is_err());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

/// Checks the fields and types HAR 1.2 requires, and that `time` is the sum
/// of the observed timings.
fn assert_har_1_2(har: &Value) {
    let log = &har["log"];
    assert_eq!(log["version"], "1.2");
    assert!(log["creator"]["name"].is_string() && log["creator"]["version"].is_string());
    assert!(log["entries"].is_array());
    for entry in entries(har) {
        assert_entry(entry);
    }
}

fn assert_entry(entry: &Value) {
    let started = entry["startedDateTime"].as_str().unwrap_or_default();
    assert_eq!(started.len(), 24, "{started}");
    assert!(started.ends_with('Z') && started.as_bytes().get(10) == Some(&b'T'));
    assert!(entry["cache"].is_object());
    let request = &entry["request"];
    for field in ["method", "url", "httpVersion"] {
        assert!(request[field].is_string(), "request.{field}");
    }
    assert!(
        request["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
    );
    let response = &entry["response"];
    assert!(response["status"].is_u64());
    for field in ["statusText", "httpVersion", "redirectURL"] {
        assert!(response[field].is_string(), "response.{field}");
    }
    for message in [request, response] {
        assert_message(message);
    }
    assert!(request["queryString"].is_array());
    if let Some(post_data) = request.get("postData") {
        assert!(post_data["mimeType"].is_string() && post_data["text"].is_string());
    }
    let content = &response["content"];
    assert!(content["size"].is_i64() && content["mimeType"].is_string());
    assert_timings(entry);
}

fn assert_message(message: &Value) {
    for field in ["cookies", "headers"] {
        assert!(message[field].is_array(), "{field}");
    }
    for field in ["headersSize", "bodySize"] {
        assert!(message[field].is_i64(), "{field}");
    }
    for header in message["headers"].as_array().into_iter().flatten() {
        assert!(header["name"].is_string() && header["value"].is_string());
    }
}

/// Checks that required phases are observed, optional ones are `-1` when
/// not, and `time` is their sum (`ssl` is already part of `connect`).
fn assert_timings(entry: &Value) {
    let timings = &entry["timings"];
    let mut sum = 0.0;
    for phase in [
        "blocked", "dns", "connect", "send", "wait", "receive", "ssl",
    ] {
        let value = timings[phase].as_f64().unwrap_or(f64::NAN);
        let required = matches!(phase, "send" | "wait" | "receive");
        assert!(
            value >= 0.0 || (!required && (value + 1.0).abs() < f64::EPSILON),
            "{phase}"
        );
        if value >= 0.0 && phase != "ssl" {
            sum += value;
        }
    }
    let time = entry["time"].as_f64().unwrap_or(f64::NAN);
    assert!((time - sum).abs() < 0.01, "time {time} != timings {sum}");
}
