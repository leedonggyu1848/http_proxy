//! Single-segment HTTP/HTTPS fetcher with cancellation, redirect handling, and decoding.

use std::fs::File;
use std::io::{self, Write as _};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONTENT_ENCODING, HOST, LOCATION};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Empty};
use http_inspector_encoding::{EmptyTokens, Encodings};
use hyper::body::Incoming;
use hyper::client::conn::http1 as client_http1;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

use super::headers::SafeRequestHeaders;
use super::url::{HlsUrl, HttpScheme};
use crate::https::{Destination, build_upstream_connector};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;
const BROTLI_BUFFER: usize = 64 * 1024;
/// Upper bound on one decoded segment (512 MiB) to stop decompression bombs.
const MAX_DECODED_SEGMENT_BYTES: u64 = 512 * 1024 * 1024;

/// Shared network configuration for fetching missing HLS segments.
#[derive(Clone)]
pub(super) struct SegmentFetcher {
    tls: Option<Arc<TlsConnector>>,
    additional_root: Option<std::path::PathBuf>,
    proxy_addr: Option<SocketAddr>,
}

impl SegmentFetcher {
    pub(super) const fn new(
        additional_root: Option<std::path::PathBuf>,
        proxy_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            tls: None,
            additional_root,
            proxy_addr,
        }
    }

    pub(super) fn ensure_tls(&mut self) -> Result<Arc<TlsConnector>, String> {
        if let Some(tls) = &self.tls {
            return Ok(Arc::clone(tls));
        }
        let connector = build_upstream_connector(self.additional_root.as_deref())
            .map_err(|error| error.to_string())?;
        let connector = Arc::new(connector);
        self.tls = Some(Arc::clone(&connector));
        Ok(connector)
    }
}

/// Failure while fetching one HLS segment.
#[derive(Debug)]
pub(super) enum FetchError {
    Cancelled,
    HttpStatus { status: u16, reason: String },
    TooManyRedirects,
    InvalidRedirect(String),
    Network(String),
    Decode(String),
    TlsSetup(String),
    Io(io::Error),
}

/// Fetches `url` into `destination_path`, following safe redirects and decoding `Content-Encoding`.
pub(super) async fn fetch_segment_to_file(
    fetcher: &mut SegmentFetcher,
    initial_url: &HlsUrl,
    initial_headers: &SafeRequestHeaders,
    destination_path: &Path,
    cancel: &mut watch::Receiver<bool>,
    on_bytes: impl Fn(u64) + Send + Sync,
) -> Result<u64, FetchError> {
    let mut current_url = initial_url.clone();
    let mut current_headers = initial_headers.clone();

    for redirect_count in 0..=MAX_REDIRECTS {
        if *cancel.borrow() {
            return Err(FetchError::Cancelled);
        }
        let response = send_get_request(fetcher, &current_url, &current_headers, cancel).await?;
        let status = response.status();
        if is_redirect(status) {
            if redirect_count == MAX_REDIRECTS {
                return Err(FetchError::TooManyRedirects);
            }
            let next_url = resolve_redirect(&current_url, &response)?;
            if next_url.origin() != current_url.origin() {
                current_headers = current_headers.for_cross_origin_redirect();
            }
            current_url = next_url;
            continue;
        }
        if status != StatusCode::OK {
            let reason = status.canonical_reason().unwrap_or("Error").to_owned();
            return Err(FetchError::HttpStatus {
                status: status.as_u16(),
                reason,
            });
        }
        return write_response_body(response, destination_path, cancel, &on_bytes).await;
    }

    Err(FetchError::TooManyRedirects)
}

const fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn resolve_redirect(base: &HlsUrl, response: &Response<Incoming>) -> Result<HlsUrl, FetchError> {
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| FetchError::InvalidRedirect("missing Location header".to_owned()))?;
    base.resolve(location)
        .map_err(|error| FetchError::InvalidRedirect(error.to_string()))
}

async fn send_get_request(
    fetcher: &mut SegmentFetcher,
    url: &HlsUrl,
    headers: &SafeRequestHeaders,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Response<Incoming>, FetchError> {
    let addresses = resolve_addresses(url, fetcher.proxy_addr, cancel).await?;
    let tcp = connect_tcp(&addresses, cancel).await?;
    let request = build_http_request(url, headers)?;

    match url.scheme() {
        HttpScheme::Http => send_over_io(tcp, request, cancel).await,
        HttpScheme::Https => {
            let tls = fetcher.ensure_tls().map_err(FetchError::TlsSetup)?;
            let authority = url
                .authority()
                .parse::<http::uri::Authority>()
                .map_err(|error| FetchError::Network(error.to_string()))?;
            Destination::from_authority(&authority)
                .map_err(|error| FetchError::Network(error.to_string()))?;
            let host = url.host().trim_start_matches('[').trim_end_matches(']');
            let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
                .map_err(|error| FetchError::Network(error.to_string()))?;
            let tls_stream = run_cancellable(
                cancel,
                timeout(CONNECT_TIMEOUT, tls.connect(server_name, tcp)),
            )
            .await?
            .map_err(|_| FetchError::Network("TLS handshake timed out".to_owned()))?
            .map_err(|error| FetchError::Network(format!("TLS handshake failed: {error}")))?;
            send_over_io(tls_stream, request, cancel).await
        }
    }
}

async fn resolve_addresses(
    url: &HlsUrl,
    proxy_addr: Option<SocketAddr>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Vec<SocketAddr>, FetchError> {
    let host = url.host().trim_start_matches('[').trim_end_matches(']');
    let lookup = tokio::net::lookup_host((host, url.port()));
    let addresses: Vec<SocketAddr> = run_cancellable(cancel, timeout(CONNECT_TIMEOUT, lookup))
        .await?
        .map_err(|_| FetchError::Network("DNS resolution timed out".to_owned()))?
        .map_err(|error| FetchError::Network(format!("DNS lookup failed: {error}")))?
        .collect();
    if addresses.is_empty() {
        return Err(FetchError::Network(
            "DNS lookup returned no addresses".to_owned(),
        ));
    }
    if let Some(proxy_addr) = proxy_addr
        && addresses.contains(&proxy_addr)
    {
        return Err(FetchError::Network(
            "segment URL targets the local proxy itself".to_owned(),
        ));
    }
    Ok(addresses)
}

async fn connect_tcp(
    addresses: &[SocketAddr],
    cancel: &mut watch::Receiver<bool>,
) -> Result<TcpStream, FetchError> {
    run_cancellable(
        cancel,
        timeout(CONNECT_TIMEOUT, TcpStream::connect(addresses)),
    )
    .await?
    .map_err(|_| FetchError::Network("connection timed out".to_owned()))?
    .map_err(|error| FetchError::Network(format!("connection failed: {error}")))
}

fn build_http_request(
    url: &HlsUrl,
    headers: &SafeRequestHeaders,
) -> Result<Request<Empty<Bytes>>, FetchError> {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(url.path_and_query())
        .header(HOST, url.authority());
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    builder
        .body(Empty::new())
        .map_err(|error| FetchError::Network(error.to_string()))
}

async fn send_over_io<IO>(
    stream: IO,
    request: Request<Empty<Bytes>>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Response<Incoming>, FetchError>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = run_cancellable(
        cancel,
        timeout(
            CONNECT_TIMEOUT,
            client_http1::handshake(TokioIo::new(stream)),
        ),
    )
    .await?
    .map_err(|_| FetchError::Network("HTTP handshake timed out".to_owned()))?
    .map_err(|error| FetchError::Network(format!("HTTP handshake failed: {error}")))?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!(%error, "HLS segment HTTP connection ended");
        }
    });

    run_cancellable(
        cancel,
        timeout(RESPONSE_TIMEOUT, sender.send_request(request)),
    )
    .await?
    .map_err(|_| FetchError::Network("response headers timed out".to_owned()))?
    .map_err(|error| FetchError::Network(format!("request failed: {error}")))
}

async fn write_response_body(
    response: Response<Incoming>,
    destination_path: &Path,
    cancel: &mut watch::Receiver<bool>,
    on_bytes: &(impl Fn(u64) + Send + Sync),
) -> Result<u64, FetchError> {
    let encodings = parse_response_encodings(&response)?;
    let mut body = response.into_body();

    if encodings.is_empty() {
        let mut file = io::BufWriter::new(File::create(destination_path).map_err(FetchError::Io)?);
        let written = stream_body_to_writer(&mut body, &mut file, cancel, on_bytes).await?;
        file.flush().map_err(FetchError::Io)?;
        return Ok(written);
    }

    let raw_path = destination_path.with_extension("encoded");
    let raw_result = async {
        let mut raw_file = io::BufWriter::new(File::create(&raw_path).map_err(FetchError::Io)?);
        let written = stream_body_to_writer(&mut body, &mut raw_file, cancel, on_bytes).await?;
        raw_file.flush().map_err(FetchError::Io)?;
        Ok::<u64, FetchError>(written)
    }
    .await;

    let decode_result =
        raw_result.and_then(|_| decode_file(&raw_path, destination_path, &encodings));
    let _ = std::fs::remove_file(&raw_path);
    decode_result
}

fn parse_response_encodings(response: &Response<Incoming>) -> Result<Encodings, FetchError> {
    let values = response
        .headers()
        .get_all(CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok());
    Encodings::parse(values, EmptyTokens::Skip)
        .map_err(|error| FetchError::Decode(error.to_string()))
}

async fn stream_body_to_writer(
    body: &mut Incoming,
    writer: &mut (impl io::Write + Send),
    cancel: &mut watch::Receiver<bool>,
    on_bytes: &(impl Fn(u64) + Send + Sync),
) -> Result<u64, FetchError> {
    let mut total: u64 = 0;
    loop {
        let frame = run_cancellable(cancel, timeout(BODY_IDLE_TIMEOUT, body.frame()))
            .await?
            .map_err(|_| FetchError::Network("segment body read timed out".to_owned()))?;
        let Some(frame) = frame else {
            return Ok(total);
        };
        let frame =
            frame.map_err(|error| FetchError::Network(format!("body read error: {error}")))?;
        if let Some(chunk) = frame.data_ref() {
            writer.write_all(chunk).map_err(FetchError::Io)?;
            let len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            total = total.saturating_add(len);
            if total > MAX_DECODED_SEGMENT_BYTES {
                return Err(FetchError::Decode(
                    "segment exceeded maximum size limit".to_owned(),
                ));
            }
            on_bytes(len);
        }
    }
}

fn decode_file(
    raw_path: &Path,
    destination_path: &Path,
    encodings: &Encodings,
) -> Result<u64, FetchError> {
    use std::io::Read as _;
    let input = io::BufReader::new(File::open(raw_path).map_err(FetchError::Io)?);
    let probe = MAX_DECODED_SEGMENT_BYTES.saturating_add(1);
    let mut reader = encodings
        .reader(input, probe, BROTLI_BUFFER)
        .map_err(|error| FetchError::Decode(error.to_string()))?
        .take(probe);
    let mut output = io::BufWriter::new(File::create(destination_path).map_err(FetchError::Io)?);
    let written = io::copy(&mut reader, &mut output)
        .map_err(|error| FetchError::Decode(error.to_string()))?;
    output.flush().map_err(FetchError::Io)?;
    if written > MAX_DECODED_SEGMENT_BYTES {
        return Err(FetchError::Decode(
            "decoded segment exceeded maximum size limit".to_owned(),
        ));
    }
    Ok(written)
}

async fn run_cancellable<T>(
    cancel: &mut watch::Receiver<bool>,
    future: impl std::future::Future<Output = T>,
) -> Result<T, FetchError> {
    if *cancel.borrow() {
        return Err(FetchError::Cancelled);
    }
    tokio::select! {
        biased;
        () = wait_cancelled(cancel) => Err(FetchError::Cancelled),
        output = future => Ok(output),
    }
}

async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow() {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
