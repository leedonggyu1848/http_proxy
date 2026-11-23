//! Integration test verifying real proxy captures export HAR 1.2 documents and
//! URL maps identically to [`http_proxy::ExchangeBuilder`] fixtures.
#![expect(
    clippy::indexing_slicing,
    reason = "Reading a serde_json::Value by key yields Null instead of panicking."
)]

use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use http::header::{
    AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, PROXY_AUTHORIZATION,
    SET_COOKIE,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use http_proxy::interop::har::{HarOptions, document};
use http_proxy::interop::map_export::{export_graph_mermaid, export_tree_json};
use http_proxy::url_map::UrlMap;
use http_proxy::{
    BodyLifecycle, Exchange, ExchangeBuilder, Outcome, ProxyConfig, ProxyHandle, ResponseFixture,
    TunnelState,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{TEST_TIMEOUT, TestResult, next_record, read_headers, read_request};

const SECRETS: [&str; 4] = [
    "secret-authorization-token",
    "secret-proxy-credentials",
    "secret-session-cookie",
    "secret-set-cookie",
];
const REQUEST_BODY: &str = "name=request-body-text";
const RESPONSE_BODY: &str = "{\"answer\":\"response-body-text\"}";

async fn wait_until_ended(record: &Exchange) {
    while record.is_live() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn sensitive_request(address: SocketAddr) -> String {
    format!(
        "POST http://{address}/login?user=alice&empty HTTP/1.1\r\nHost: {address}\r\n\
         Authorization: Bearer {}\r\nProxy-Authorization: Basic {}\r\n\
         Cookie: session={}\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{REQUEST_BODY}",
        SECRETS[0],
        SECRETS[1],
        SECRETS[2],
        REQUEST_BODY.len(),
    )
}

fn sensitive_response() -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Set-Cookie: id={}; HttpOnly\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{RESPONSE_BODY}",
        SECRETS[3],
        RESPONSE_BODY.len(),
    )
    .into_bytes()
}

async fn response_capture(proxy: &mut ProxyHandle) -> TestResult<(SocketAddr, Exchange)> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client
        .write_all(sensitive_request(address).as_bytes())
        .await?;
    let (mut server, _) = upstream.accept().await?;
    let _received = read_request(&mut server).await?;
    server.write_all(&sensitive_response()).await?;
    drop(server);
    let _bytes = client.read_to_end(&mut Vec::new()).await?;
    let record = next_record(proxy).await?;
    wait_until_ended(&record).await;
    Ok((address, record))
}

async fn tunnel_capture(proxy: &mut ProxyHandle, upstream: &TcpListener) -> TestResult<Exchange> {
    let address = upstream.local_addr()?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    client
        .write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await?;
    let (mut server, _) = upstream.accept().await?;
    let tunnel = next_record(proxy).await?;
    let _reply = read_headers(&mut client).await?;
    client.write_all(b"ping").await?;
    let mut ping = [0; 4];
    server.read_exact(&mut ping).await?;
    server.write_all(b"pong!").await?;
    drop(server);
    let _read = client.read_to_end(&mut Vec::new()).await?;
    drop(client);
    wait_until_ended(&tunnel).await;
    Ok(tunnel)
}

struct Captures {
    proxy: ProxyHandle,
    response_address: SocketAddr,
    address: SocketAddr,
    records: Vec<Exchange>,
    _waiting: [TcpStream; 2],
}

async fn proxy_captures() -> TestResult<Captures> {
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let (response_address, response) = response_capture(&mut proxy).await?;
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
    let pending = next_record(&mut proxy).await?;
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
    let failed = next_record(&mut proxy).await?;
    wait_until_ended(&failed).await;
    Ok(Captures {
        proxy,
        response_address,
        address,
        records: vec![response, tunnel, pending, failed],
        _waiting: [pending_client, pending_server],
    })
}

fn with_headers<T>(
    target: T,
    headers: Vec<(HeaderName, String)>,
    add: impl Fn(T, HeaderName, HeaderValue) -> T,
) -> TestResult<T> {
    headers
        .into_iter()
        .try_fold(target, |target, (name, value)| {
            Ok(add(target, name, HeaderValue::try_from(value)?))
        })
}

fn fixtures(
    response_address: SocketAddr,
    address: SocketAddr,
    failure: &Outcome,
) -> TestResult<Vec<Exchange>> {
    let Outcome::Failed { stage, message } = failure else {
        return Err("expected a failed capture".into());
    };
    let response = with_headers(
        ExchangeBuilder::http(
            Method::POST,
            format!("http://{response_address}/login?user=alice&empty"),
        )?,
        vec![
            (HOST, response_address.to_string()),
            (AUTHORIZATION, format!("Bearer {}", SECRETS[0])),
            (PROXY_AUTHORIZATION, format!("Basic {}", SECRETS[1])),
            (COOKIE, format!("session={}", SECRETS[2])),
            (CONTENT_TYPE, "application/x-www-form-urlencoded".to_owned()),
            (CONTENT_LENGTH, REQUEST_BODY.len().to_string()),
            (CONNECTION, "close".to_owned()),
        ],
        ExchangeBuilder::header,
    )?
    .request_body(REQUEST_BODY, BodyLifecycle::Complete)
    .respond(
        with_headers(
            ResponseFixture::new(StatusCode::OK),
            vec![
                (CONTENT_TYPE, "application/json".to_owned()),
                (SET_COOKIE, format!("id={}; HttpOnly", SECRETS[3])),
                (CONTENT_LENGTH, RESPONSE_BODY.len().to_string()),
                (CONNECTION, "close".to_owned()),
            ],
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

#[tokio::test]
async fn proxy_captures_export_har_and_map_like_builder_fixtures() -> TestResult {
    timeout(TEST_TIMEOUT, async {
        let Captures {
            proxy,
            response_address,
            address,
            records: captured,
            _waiting,
        } = proxy_captures().await?;
        let [_, _, _, failed] = captured.as_slice() else {
            return Err("expected four captures".into());
        };
        let built = fixtures(response_address, address, failed.outcome())?;

        let now = SystemTime::now();
        for options in [
            HarOptions::default(),
            HarOptions {
                include_bodies: true,
                include_sensitive_headers: true,
            },
        ] {
            let captured_har = document(&captured, options, now);
            let serialized = serde_json::to_string(&captured_har)?;
            for secret in SECRETS {
                assert!(!serialized.contains(secret), "{secret} leaked");
            }
            assert_eq!(captured_har["log"]["version"], "1.2");
            assert_eq!(
                without_clock(document(&built, options, now)),
                without_clock(captured_har),
            );
        }
        assert_eq!(url_map(&built), url_map(&captured));
        proxy.stop()?;
        Ok(())
    })
    .await?
}
