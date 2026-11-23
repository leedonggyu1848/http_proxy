//! What one exchange offers to search, and whether it matches a query.
//!
//! Bodies are searched in their decoded preview: the first 8 KiB that the
//! detail panel shows, after removing `Content-Encoding` under the same
//! limits as the preview. Binary bodies are skipped.

use std::collections::HashMap;
use std::sync::Arc;

use http_proxy::text_search::Matcher;
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, Exchange, HeaderPreview, MediaType, Outcome,
    RecordId,
};

use crate::ui::body_display::BodySide;
use crate::ui::outcome_view;
use crate::ui::search::SearchScope;
use http_proxy::content::decode as body_view;

/// Bytes at the start of a decoded body that are checked for NUL when the
/// media type does not say whether the body is text.
const SNIFF_BYTES: usize = 1024;

/// Why a body that did not match may still contain the query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) struct Coverage {
    /// Only the first part of a body was searched.
    pub(in crate::ui) partial: bool,
    /// A body was binary and was not searched.
    pub(in crate::ui) binary: bool,
    /// A body could not be decoded and was not searched.
    pub(in crate::ui) undecodable: bool,
}

impl Coverage {
    const fn add(&mut self, body: &BodyText) {
        match body {
            BodyText::Searchable { partial, .. } => self.partial |= *partial,
            BodyText::Binary => self.binary = true,
            BodyText::Undecodable => self.undecodable = true,
            BodyText::Empty => {}
        }
    }
}

/// The result of checking one exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct Checked {
    pub(in crate::ui) id: RecordId,
    pub(in crate::ui) matched: bool,
    /// Nothing searched can change any more, so the result is final.
    pub(in crate::ui) settled: bool,
    /// Set only for exchanges that did not match.
    pub(in crate::ui) coverage: Coverage,
}

/// The searchable form of one body.
#[derive(Debug)]
enum BodyText {
    Searchable { bytes: Vec<u8>, partial: bool },
    Binary,
    Undecodable,
    Empty,
}

/// Decoded bodies of settled exchanges, so a new query does not decompress
/// them again.
#[derive(Default)]
pub(in crate::ui) struct BodyCache {
    bodies: HashMap<(RecordId, BodySide), Arc<BodyText>>,
}

impl BodyCache {
    /// Forgets bodies of exchanges older than `oldest`. Record ids grow, and
    /// the list drops its oldest records first, so these are no longer shown.
    pub(in crate::ui) fn forget_before(&mut self, oldest: RecordId) {
        self.bodies
            .retain(|(id, _), _| id.value() >= oldest.value());
    }

    pub(in crate::ui) fn len(&self) -> usize {
        self.bodies.len()
    }
}

/// Reports whether every part of `record` that search reads is final.
pub(in crate::ui) fn is_settled(record: &Exchange) -> bool {
    let ended = |body: &BodyPreview| body.lifecycle() != BodyLifecycle::Streaming;
    let request = record.request().body().is_none_or(ended);
    request
        && match record.outcome() {
            Outcome::InFlight => false,
            Outcome::Response(response) => ended(response.body()),
            _ => true,
        }
}

/// Checks `record` against `matcher` in `scope`.
pub(in crate::ui) fn check(
    record: &Exchange,
    matcher: &Matcher,
    scope: SearchScope,
    cache: &mut BodyCache,
) -> Checked {
    let settled = is_settled(record);
    let mut coverage = Coverage::default();
    let hit = metadata_matches(record, matcher, scope)
        || [BodySide::Request, BodySide::Response]
            .into_iter()
            .filter(|side| scope.covers_body(*side))
            .any(|side| {
                let Some(body) = body_text(record, side, settled, cache) else {
                    return false;
                };
                coverage.add(&body);
                matches!(&*body, BodyText::Searchable { bytes, .. } if matcher.is_match(bytes))
            });
    Checked {
        id: record.id(),
        matched: hit,
        settled,
        coverage: if hit { Coverage::default() } else { coverage },
    }
}

fn metadata_matches(record: &Exchange, matcher: &Matcher, scope: SearchScope) -> bool {
    let request = record.request();
    let response_headers = match record.outcome() {
        Outcome::Response(response) => Some(response.headers()),
        _ => None,
    };
    let text = |value: &str| matcher.is_match(value.as_bytes());
    (scope.covers_url() && text(request.target()))
        || (scope.covers_headers()
            && (headers_match(request.headers(), matcher)
                || response_headers.is_some_and(|headers| headers_match(headers, matcher))))
        || (scope == SearchScope::All
            && (text(request.method().as_str())
                || outcome_view::outcome_matches(record.outcome(), matcher)))
}

/// Matches header names and the values kept at capture. Values such as
/// `Authorization` were replaced before they reached the UI, so their
/// original text cannot be found.
pub(in crate::ui) fn headers_match(headers: &HeaderPreview, matcher: &Matcher) -> bool {
    headers.entries().iter().any(|header| {
        matcher.is_match(header.name().as_bytes()) || matcher.is_match(header.value().as_bytes())
    })
}

/// The searchable text of one body, decoded once and cached when settled.
/// `None` when the body is not available yet.
fn body_text(
    record: &Exchange,
    side: BodySide,
    settled: bool,
    cache: &mut BodyCache,
) -> Option<Arc<BodyText>> {
    let key = (record.id(), side);
    if let Some(body) = cache.bodies.get(&key) {
        return Some(Arc::clone(body));
    }
    let (body, headers, media_type) = match (side, record.outcome()) {
        (BodySide::Request, _) => {
            let request = record.request();
            (request.body()?, request.headers(), request.media_type())
        }
        (BodySide::Response, Outcome::Response(response)) => {
            (response.body(), response.headers(), response.media_type())
        }
        (BodySide::Response, _) => return None,
    };
    let snapshot = body.snapshot()?;
    let encodings: Vec<&str> = headers.get("content-encoding").collect();
    let text = Arc::new(searchable(
        snapshot.bytes(),
        &encodings,
        media_type,
        snapshot.retention() != BodyRetention::Prefix
            || snapshot.lifecycle() != BodyLifecycle::Complete,
    ));
    if settled {
        cache.bodies.insert(key, Arc::clone(&text));
    }
    Some(text)
}

/// Decodes retained bytes into searchable text. `incomplete` means the
/// retained bytes are not the whole body.
fn searchable(
    bytes: &[u8],
    encodings: &[&str],
    media_type: Option<&MediaType>,
    incomplete: bool,
) -> BodyText {
    if bytes.is_empty() {
        return if incomplete {
            BodyText::Searchable {
                bytes: Vec::new(),
                partial: true,
            }
        } else {
            BodyText::Empty
        };
    }
    if media_type.is_some_and(is_binary_media_type) {
        return BodyText::Binary;
    }
    let Ok(decoded) = body_view::decode_prefix(bytes, encodings) else {
        return BodyText::Undecodable;
    };
    let sniffed = decoded.bytes.get(..SNIFF_BYTES).unwrap_or(&decoded.bytes);
    if memchr::memchr(0, sniffed).is_some() {
        return BodyText::Binary;
    }
    BodyText::Searchable {
        partial: decoded.cut || incomplete,
        bytes: decoded.bytes,
    }
}

/// Media types whose bodies are not text. SVG is an image written as XML,
/// so it is searched.
fn is_binary_media_type(media_type: &MediaType) -> bool {
    match media_type.kind() {
        "image" => media_type.subtype() != "svg+xml",
        "audio" | "video" | "font" => true,
        "application" => {
            let subtype = media_type.subtype();
            matches!(
                subtype,
                "octet-stream"
                    | "pdf"
                    | "zip"
                    | "gzip"
                    | "x-gzip"
                    | "zstd"
                    | "x-tar"
                    | "x-7z-compressed"
                    | "x-rar-compressed"
                    | "wasm"
                    | "protobuf"
                    | "x-protobuf"
                    | "vnd.google.protobuf"
                    | "msgpack"
                    | "x-msgpack"
                    | "cbor"
                    | "vnd.ms-fontobject"
            ) || subtype.starts_with("grpc")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn media(value: &str) -> Option<MediaType> {
        MediaType::parse(value)
    }

    fn found(body: &BodyText, query: &str) -> bool {
        let Ok(matcher) = Matcher::new(query, http_proxy::text_search::MatchOptions::default())
        else {
            return false;
        };
        matches!(body, BodyText::Searchable { bytes, .. } if matcher.is_match(bytes))
    }

    #[test]
    fn compressed_bodies_are_searched_after_decoding() -> Result<(), Box<dyn std::error::Error>> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(br#"{"error":"Quota exceeded"}"#)?;
        let gzip = encoder.finish()?;
        let body = searchable(&gzip, &["gzip"], media("application/json").as_ref(), false);
        assert!(found(&body, "quota EXCEEDED"));
        assert!(matches!(body, BodyText::Searchable { partial: false, .. }));
        let zstd = zstd::encode_all(&b"zstd needle"[..], 0)?;
        assert!(found(&searchable(&zstd, &["zstd"], None, false), "needle"));
        Ok(())
    }

    #[test]
    fn long_or_incomplete_bodies_are_marked_partial() {
        let long = vec![b'a'; http_proxy::BODY_PREVIEW_BYTES + 10];
        assert!(matches!(
            searchable(&long, &[], None, false),
            BodyText::Searchable { partial: true, .. }
        ));
        assert!(matches!(
            searchable(b"abc", &[], None, true),
            BodyText::Searchable { partial: true, .. }
        ));
        assert!(matches!(searchable(b"", &[], None, false), BodyText::Empty));
    }

    #[test]
    fn binary_bodies_are_skipped_by_type_or_content() {
        let png = searchable(b"\x89PNG needle", &[], media("image/png").as_ref(), false);
        assert!(matches!(png, BodyText::Binary));
        let unknown = searchable(b"needle\0\x01", &[], None, false);
        assert!(matches!(unknown, BodyText::Binary));
        let svg = searchable(
            b"<svg>needle</svg>",
            &[],
            media("image/svg+xml").as_ref(),
            false,
        );
        assert!(found(&svg, "needle"));
        assert!(matches!(
            searchable(b"x", &["compress"], None, false),
            BodyText::Undecodable
        ));
    }

    mod parity {
        use std::error::Error;
        use std::net::SocketAddr;
        use std::time::Duration;

        use http::header::{
            AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST,
            PROXY_AUTHORIZATION, SET_COOKIE,
        };
        use http::{HeaderName, HeaderValue, Method, StatusCode};
        use http_proxy::text_search::{MatchOptions, Matcher};
        use http_proxy::{
            BodyLifecycle, Exchange, ExchangeBuilder, Outcome, ProxyConfig, ProxyHandle,
            ResponseFixture, TunnelState,
        };
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::{TcpListener, TcpStream};
        use tokio::time::timeout;

        use super::super::{BodyCache, Coverage, SearchScope, check};

        type TestResult<T = ()> = Result<T, Box<dyn Error>>;
        type SearchRow = (bool, bool, Coverage);

        const SECRET: &str = "secret-authorization-token";
        const REQ_BODY: &str = "name=request-body-text";
        const RESP_BODY: &str = "{\"answer\":\"response-body-text\"}";

        async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
            let mut head = Vec::new();
            let mut byte = [0; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await?;
                head.extend_from_slice(&byte);
            }
            Ok(head)
        }

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
            let _head = read_head(&mut server).await?;
            let mut req_bytes = vec![0; REQ_BODY.len()];
            server.read_exact(&mut req_bytes).await?;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Set-Cookie: id=secret-set; HttpOnly\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{RESP_BODY}",
                RESP_BODY.len()
            );
            server.write_all(resp.as_bytes()).await?;
            drop(server);
            let _read = client.read_to_end(&mut Vec::new()).await?;
            let record = proxy.receive().await.ok_or("missing response")?;
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
                .write_all(
                    format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
                )
                .await?;
            let (mut server, _) = upstream.accept().await?;
            let tunnel = proxy.receive().await.ok_or("missing tunnel")?;
            let _reply = read_head(&mut client).await?;
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

        #[test]
        fn builder_records_search_like_proxy_captures() -> TestResult {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                timeout(Duration::from_secs(10), async {
                    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
                    let (resp_addr, response) = capture_response(&mut proxy).await?;
                    let upstream = TcpListener::bind("127.0.0.1:0").await?;
                    let address = upstream.local_addr()?;
                    let tunnel = capture_tunnel(&mut proxy, &upstream, address).await?;

                    let mut pending_client = TcpStream::connect(proxy.local_addr()).await?;
                    pending_client
                        .write_all(
                            format!(
                                "GET http://{address}/slow HTTP/1.1\r\nHost: {address}\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await?;
                    let (pending_server, _) = upstream.accept().await?;
                    let pending = proxy.receive().await.ok_or("missing pending")?;
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
                    let failed = proxy.receive().await.ok_or("missing failure")?;
                    wait_ended(&failed).await;

                    let built = build_fixtures(resp_addr, address, failed.outcome())?;
                    let captured = vec![response, tunnel, pending, failed];
                    assert_eq!(search_all(&built)?, search_all(&captured)?);
                    drop((pending_client, pending_server));
                    proxy.stop()?;
                    Ok::<_, Box<dyn Error>>(())
                })
                .await?
            })
        }
    }
}
