//! Integration test verifying real proxy captures search identically to
//! [`http_proxy::ExchangeBuilder`] fixtures.

use std::net::SocketAddr;
use std::time::Duration;

use http::header::{
    AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, PROXY_AUTHORIZATION,
    SET_COOKIE,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use http_proxy::analysis::search::{
    BodyCache, Coverage, MatchOptions, Matcher, SearchScope, check,
};
use http_proxy::{
    BodyLifecycle, Exchange, ExchangeBuilder, Outcome, ProxyConfig, ProxyHandle, ResponseFixture,
    TunnelState,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{TEST_TIMEOUT, TestResult, next_record, read_headers, read_request};

type SearchRow = (bool, bool, Coverage);

const SECRET: &str = "secret-authorization-token";
const REQ_BODY: &str = "name=request-body-text";
const RESP_BODY: &str = "{\"answer\":\"response-body-text\"}";

async fn wait_ended(record: &Exchange) {
    while record.is_live() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn capture_response(proxy: &mut ProxyHandle) -> TestResult<(SocketAddr, Exchange)> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let req = format!(
        "POST http://{address}/login?user=alice&empty HTTP/1.1\r\nHost: {address}\r\n\
         Authorization: Bearer {SECRET}\r\nProxy-Authorization: Basic secret-proxy\r\n\
         Cookie: session=secret-cookie\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{REQ_BODY}",
        REQ_BODY.len()
    );
    client.write_all(req.as_bytes()).await?;
    let (mut server, _) = upstream.accept().await?;
    let _received = read_request(&mut server).await?;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Set-Cookie: id=secret-set; HttpOnly\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{RESP_BODY}",
        RESP_BODY.len()
    );
    server.write_all(resp.as_bytes()).await?;
    drop(server);
    let _read = client.read_to_end(&mut Vec::new()).await?;
    let record = next_record(proxy).await?;
    wait_ended(&record).await;
    Ok((address, record))
}

async fn capture_tunnel(
    proxy: &mut ProxyHandle,
    upstream: &TcpListener,
    address: SocketAddr,
) -> TestResult<Exchange> {
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
    wait_ended(&tunnel).await;
    Ok(tunnel)
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

fn build_fixtures(
    resp_addr: SocketAddr,
    address: SocketAddr,
    failure: &Outcome,
) -> TestResult<Vec<Exchange>> {
    let Outcome::Failed { stage, message } = failure else {
        return Err("expected a failed capture".into());
    };
    let built_response = with_headers(
        ExchangeBuilder::http(
            Method::POST,
            format!("http://{resp_addr}/login?user=alice&empty"),
        )?,
        vec![
            (HOST, resp_addr.to_string()),
            (AUTHORIZATION, format!("Bearer {SECRET}")),
            (PROXY_AUTHORIZATION, "Basic secret-proxy".to_owned()),
            (COOKIE, "session=secret-cookie".to_owned()),
            (CONTENT_TYPE, "application/x-www-form-urlencoded".to_owned()),
            (CONTENT_LENGTH, REQ_BODY.len().to_string()),
            (CONNECTION, "close".to_owned()),
        ],
        ExchangeBuilder::header,
    )?
    .request_body(REQ_BODY, BodyLifecycle::Complete)
    .respond(
        with_headers(
            ResponseFixture::new(StatusCode::OK),
            vec![
                (CONTENT_TYPE, "application/json".to_owned()),
                (SET_COOKIE, "id=secret-set; HttpOnly".to_owned()),
                (CONTENT_LENGTH, RESP_BODY.len().to_string()),
                (CONNECTION, "close".to_owned()),
            ],
            ResponseFixture::header,
        )?
        .body(RESP_BODY, BodyLifecycle::Complete),
    )?;
    let host = HeaderValue::try_from(address.to_string())?;
    Ok(vec![
        built_response,
        ExchangeBuilder::connect(address.to_string())
            .header(HOST, host.clone())
            .tunnel(4, 5, TunnelState::Closed)?,
        ExchangeBuilder::http(Method::GET, format!("http://{address}/slow"))?
            .header(HOST, host.clone())
            .request_body(Vec::new(), BodyLifecycle::Complete)
            .in_flight()?,
        ExchangeBuilder::http(Method::GET, format!("http://{address}/"))?
            .header(HOST, host)
            .header(CONNECTION, HeaderValue::from_static("close"))
            .fail(*stage, message)?,
    ])
}

fn search_all(records: &[Exchange]) -> TestResult<Vec<SearchRow>> {
    let mut results = Vec::new();
    let mut cache = BodyCache::default();
    for query in [
        "POST",
        "login",
        "request-body-text",
        "response-body-text",
        "application/json",
        "[REDACTED]",
        SECRET,
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

#[tokio::test]
async fn builder_records_search_like_proxy_captures() -> TestResult {
    timeout(TEST_TIMEOUT, async {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let (resp_addr, response) = capture_response(&mut proxy).await?;
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;
        let tunnel = capture_tunnel(&mut proxy, &upstream, address).await?;

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
                    "GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\n\
                     Connection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let failed = next_record(&mut proxy).await?;
        wait_ended(&failed).await;

        let built = build_fixtures(resp_addr, address, failed.outcome())?;
        let captured = vec![response, tunnel, pending, failed];
        assert_eq!(search_all(&built)?, search_all(&captured)?);
        drop((pending_client, pending_server));
        proxy.stop()?;
        Ok(())
    })
    .await?
}
