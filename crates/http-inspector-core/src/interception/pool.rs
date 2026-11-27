//! Upstream HTTP/1.1 connection pool shared by plain HTTP and inspected HTTPS
//! forwarding paths.
//!
//! Connections are partitioned by [`PoolKey`]: normalized `(host, port)` for
//! plain HTTP, and `(host, port, UpstreamTlsKey)` for inspected HTTPS so
//! distinct `CONNECT` destinations and TLS trust configurations never share
//! sockets. Idle connections are bounded per key, evicted lazily on access and
//! proactively by a background reaper, and closed deterministically on proxy
//! shutdown.

use std::collections::{HashMap, VecDeque};
use std::error::Error as StdError;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::CONNECTION;
use http::uri::Authority;
use http::{HeaderMap, Method, Version};
use http_body::{Body, Frame, SizeHint};
use http_inspector_exchange::{BodyLifecycle, BodyPreview, ConnectionId, UpstreamConnection};
use hyper::client::conn::http1 as client_http1;
use tokio::task::AbortHandle;

use super::ProxyBody;
use crate::https::{Destination, UpstreamTlsKey};

const MIN_REAPER_INTERVAL: Duration = Duration::from_millis(10);
const MAX_REAPER_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum request-body bytes buffered on the fly for retrying an idempotent
/// request when a reused idle connection was closed by the upstream server.
pub(super) const MAX_RETRY_BODY_BYTES: usize = 64 * 1024;

/// Cache key partitioning pooled upstream HTTP/1 connections.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) enum PoolKey {
    /// Plain HTTP upstream connection keyed by normalized `(host, port)`.
    Plain { host: String, port: u16 },
    /// Inspected HTTPS upstream connection keyed by normalized `(host, port)`
    /// and the upstream TLS verification fingerprint.
    Https {
        host: String,
        port: u16,
        tls: UpstreamTlsKey,
    },
}

impl PoolKey {
    pub(super) fn plain(authority: &Authority, default_port: u16) -> Self {
        Self::Plain {
            host: normalize_host(authority.host()),
            port: authority.port_u16().unwrap_or(default_port),
        }
    }

    pub(super) fn https(destination: &Destination, tls: UpstreamTlsKey) -> Self {
        Self::Https {
            host: normalize_host(destination.host()),
            port: destination.port(),
            tls,
        }
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

struct IdleEntry {
    id: ConnectionId,
    sender: client_http1::SendRequest<ProxyBody>,
    driver: AbortHandle,
    idle_since: Instant,
}

impl IdleEntry {
    fn is_expired(&self, now: Instant, idle_timeout: Duration) -> bool {
        now.saturating_duration_since(self.idle_since) >= idle_timeout
    }

    fn close(self) {
        self.driver.abort();
        drop(self.sender);
    }
}

/// One checked-out or freshly established upstream HTTP/1 connection.
pub(super) struct PooledConnection {
    id: ConnectionId,
    sender: client_http1::SendRequest<ProxyBody>,
    driver: AbortHandle,
    reused: bool,
}

impl PooledConnection {
    pub(super) const fn fresh(
        id: ConnectionId,
        sender: client_http1::SendRequest<ProxyBody>,
        driver: AbortHandle,
    ) -> Self {
        Self {
            id,
            sender,
            driver,
            reused: false,
        }
    }

    pub(super) const fn is_reused(&self) -> bool {
        self.reused
    }

    pub(super) const fn metadata(&self) -> UpstreamConnection {
        if self.reused {
            UpstreamConnection::reused(self.id)
        } else {
            UpstreamConnection::fresh(self.id)
        }
    }

    pub(super) const fn sender_mut(&mut self) -> &mut client_http1::SendRequest<ProxyBody> {
        &mut self.sender
    }

    /// Aborts the background HTTP/1 driver and drops the sender, closing the
    /// underlying socket immediately.
    pub(super) fn close(self) {
        self.driver.abort();
        drop(self.sender);
    }

    pub(super) fn into_return(
        self,
        pool: Arc<UpstreamPool>,
        key: PoolKey,
        keep_alive: bool,
        request_body: BodyPreview,
    ) -> PoolReturn {
        PoolReturn {
            pool,
            key,
            id: self.id,
            sender: self.sender,
            driver: self.driver,
            keep_alive,
            request_body,
        }
    }
}

struct PoolState {
    idle: HashMap<PoolKey, VecDeque<IdleEntry>>,
    closed: bool,
}

/// Bounded HTTP/1 upstream connection pool shared by one proxy run.
pub(super) struct UpstreamPool {
    max_idle_per_host: usize,
    idle_timeout: Duration,
    state: Mutex<PoolState>,
}

fn lock_recovering<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl UpstreamPool {
    pub(super) fn new(max_idle_per_host: usize, idle_timeout: Duration) -> Self {
        Self {
            max_idle_per_host,
            idle_timeout,
            state: Mutex::new(PoolState {
                idle: HashMap::new(),
                closed: false,
            }),
        }
    }

    /// Interval at which the background reaper evicts expired idle connections.
    pub(super) fn reaper_interval(&self) -> Duration {
        (self.idle_timeout / 2).clamp(MIN_REAPER_INTERVAL, MAX_REAPER_INTERVAL)
    }

    /// Checks out a ready idle connection for `key`, discarding any closed or
    /// expired entries along the way. Returns `None` when a fresh dial is needed.
    pub(super) async fn checkout(&self, key: &PoolKey) -> Option<PooledConnection> {
        loop {
            let mut entry = self.pop_live(key)?;
            if entry.sender.ready().await.is_ok() {
                return Some(PooledConnection {
                    id: entry.id,
                    sender: entry.sender,
                    driver: entry.driver,
                    reused: true,
                });
            }
            entry.close();
        }
    }

    fn pop_live(&self, key: &PoolKey) -> Option<IdleEntry> {
        let now = Instant::now();
        let mut evicted = Vec::new();
        let selected = {
            let mut state = lock_recovering(&self.state);
            if state.closed {
                return None;
            }
            let mut chosen = None;
            let remove_bucket = state.idle.get_mut(key).is_some_and(|queue| {
                // Pop LIFO so the warmest connection is reused first and older
                // idle connections naturally expire at the front.
                while let Some(entry) = queue.pop_back() {
                    if entry.is_expired(now, self.idle_timeout) || entry.sender.is_closed() {
                        evicted.push(entry);
                    } else {
                        chosen = Some(entry);
                        break;
                    }
                }
                queue.is_empty()
            });
            if remove_bucket {
                let _ = state.idle.remove(key);
            }
            drop(state);
            chosen
        };
        for entry in evicted {
            entry.close();
        }
        selected
    }

    /// Returns a healthy keep-alive connection to the pool under `key`.
    pub(super) fn put(
        &self,
        key: PoolKey,
        id: ConnectionId,
        sender: client_http1::SendRequest<ProxyBody>,
        driver: AbortHandle,
    ) {
        if self.max_idle_per_host == 0 || self.idle_timeout.is_zero() || sender.is_closed() {
            driver.abort();
            drop(sender);
            return;
        }
        let now = Instant::now();
        let entry = IdleEntry {
            id,
            sender,
            driver,
            idle_since: now,
        };
        let mut evicted = Vec::new();
        {
            let mut state = lock_recovering(&self.state);
            if state.closed {
                evicted.push(entry);
            } else {
                let queue = state.idle.entry(key).or_default();
                drain_stale(queue, now, self.idle_timeout, &mut evicted);
                queue.push_back(entry);
                while queue.len() > self.max_idle_per_host {
                    if let Some(oldest) = queue.pop_front() {
                        evicted.push(oldest);
                    }
                }
            }
            drop(state);
        }
        for stale in evicted {
            stale.close();
        }
    }

    /// Evicts and closes every expired or already-closed idle connection.
    pub(super) fn evict_expired(&self) {
        let now = Instant::now();
        let mut evicted = Vec::new();
        {
            let mut state = lock_recovering(&self.state);
            for queue in state.idle.values_mut() {
                drain_stale(queue, now, self.idle_timeout, &mut evicted);
            }
            state.idle.retain(|_, queue| !queue.is_empty());
        }
        for entry in evicted {
            entry.close();
        }
    }

    /// Marks the pool closed and immediately closes every idle connection.
    pub(super) fn close_all(&self) {
        let mut evicted = Vec::new();
        {
            let mut state = lock_recovering(&self.state);
            state.closed = true;
            for (_, queue) in state.idle.drain() {
                evicted.extend(queue);
            }
        }
        for entry in evicted {
            entry.close();
        }
    }

    #[cfg(test)]
    pub(super) fn idle_count(&self, key: &PoolKey) -> usize {
        lock_recovering(&self.state)
            .idle
            .get(key)
            .map_or(0, VecDeque::len)
    }

    #[cfg(test)]
    pub(super) fn total_idle(&self) -> usize {
        lock_recovering(&self.state)
            .idle
            .values()
            .map(VecDeque::len)
            .sum()
    }
}

fn drain_stale(
    queue: &mut VecDeque<IdleEntry>,
    now: Instant,
    idle_timeout: Duration,
    evicted: &mut Vec<IdleEntry>,
) {
    let mut kept = VecDeque::with_capacity(queue.len());
    while let Some(entry) = queue.pop_front() {
        if entry.is_expired(now, idle_timeout) || entry.sender.is_closed() {
            evicted.push(entry);
        } else {
            kept.push_back(entry);
        }
    }
    *queue = kept;
}

/// Owns a connection while its response body is still streaming to the client.
///
/// On clean response-body EOF, returns the connection to the pool when both
/// sides completed cleanly and keep-alive is permitted; on error or early drop,
/// aborts and closes the connection so a desynchronized socket is never reused.
pub(super) struct PoolReturn {
    pool: Arc<UpstreamPool>,
    key: PoolKey,
    id: ConnectionId,
    sender: client_http1::SendRequest<ProxyBody>,
    driver: AbortHandle,
    keep_alive: bool,
    request_body: BodyPreview,
}

impl PoolReturn {
    fn finish(self) {
        if self.keep_alive
            && self.request_body.lifecycle() == BodyLifecycle::Complete
            && !self.sender.is_closed()
        {
            self.pool.put(self.key, self.id, self.sender, self.driver);
        } else {
            // Response body reached EOF cleanly without keep-alive; drop the
            // sender so the driver finishes cleanly without cutting off anything.
            drop(self.sender);
        }
    }

    fn discard(self) {
        self.driver.abort();
        drop(self.sender);
    }
}

/// Response body wrapper that returns the underlying upstream connection to the
/// pool as soon as the response body reaches clean end-of-stream.
pub(super) struct PoolReturnBody<B> {
    body: B,
    slot: Option<PoolReturn>,
}

impl<B> PoolReturnBody<B>
where
    B: Body,
{
    pub(super) fn new(body: B, slot: PoolReturn) -> Self {
        let slot = if body.is_end_stream() {
            slot.finish();
            None
        } else {
            Some(slot)
        };
        Self { body, slot }
    }
}

impl<B> Body for PoolReturnBody<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.body).poll_frame(context);
        match &polled {
            Poll::Ready(Some(Ok(_))) => {
                if this.body.is_end_stream()
                    && let Some(slot) = this.slot.take()
                {
                    slot.finish();
                }
            }
            Poll::Ready(None) => {
                if let Some(slot) = this.slot.take() {
                    slot.finish();
                }
            }
            Poll::Ready(Some(Err(_))) => {
                if let Some(slot) = this.slot.take() {
                    slot.discard();
                }
            }
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

impl<B> Drop for PoolReturnBody<B> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.discard();
        }
    }
}

/// Reports whether `method` is idempotent per RFC 9110 §9.2.2 and therefore
/// eligible for a single retry on a fresh connection when a reused idle
/// connection was closed by the upstream server.
pub(super) const fn is_idempotent_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    )
}

/// Reports whether the upstream response head permits keeping the HTTP/1
/// connection persistent per RFC 9112 §9.3. Must be called before stripping
/// hop-by-hop headers.
pub(super) fn response_allows_keep_alive(version: Version, headers: &HeaderMap) -> bool {
    let mut saw_keep_alive = false;
    for value in headers.get_all(CONNECTION) {
        let Ok(text) = value.to_str() else {
            continue;
        };
        for token in text.split(',').map(str::trim) {
            if token.eq_ignore_ascii_case("close") {
                return false;
            }
            if token.eq_ignore_ascii_case("keep-alive") {
                saw_keep_alive = true;
            }
        }
    }
    match version {
        Version::HTTP_11 => true,
        Version::HTTP_10 => saw_keep_alive,
        _ => false,
    }
}

/// Reports whether a `hyper::Error` on a reused pooled connection indicates
/// that the upstream server closed the idle connection before answering.
pub(super) fn is_stale_connection_error(error: &hyper::Error) -> bool {
    if error.is_incomplete_message() || error.is_closed() || error.is_canceled() {
        return true;
    }
    let mut source = StdError::source(error);
    while let Some(err) = source {
        if let Some(io_err) = err.downcast_ref::<io::Error>() {
            return matches!(
                io_err.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::NotConnected
            );
        }
        source = err.source();
    }
    false
}

struct ReplayState<B> {
    inner: Option<B>,
    chunks: VecDeque<Bytes>,
    buffered_bytes: usize,
    limit: usize,
    overflowed: bool,
}

/// Streaming request body wrapper for the first attempt on a reused connection.
///
/// Frames are forwarded immediately without pre-buffering while up to `limit`
/// bytes are remembered so an idempotent request can be replayed cleanly on a
/// fresh connection if the reused idle socket was closed.
pub(super) struct AttemptBody<B> {
    state: Arc<Mutex<ReplayState<B>>>,
    size_hint: SizeHint,
}

/// Handle kept by the forwarding path to reclaim buffered frames and any unread
/// remainder if the first attempt on a reused connection fails cleanly.
pub(super) struct RetryBodyHandle<B> {
    state: Arc<Mutex<ReplayState<B>>>,
}

/// Request body replayed on a fresh connection after a stale pooled connection
/// failed. Emits remembered chunks first, then continues streaming unread
/// frames from the original body so observers only see each byte once.
pub(super) struct ReplayedBody<B> {
    chunks: VecDeque<Bytes>,
    remaining: Option<B>,
}

/// Splits `body` into a streaming first-attempt body and a retry handle.
pub(super) fn replayable_body<B>(body: B, limit: usize) -> (AttemptBody<B>, RetryBodyHandle<B>)
where
    B: Body,
{
    let size_hint = body.size_hint();
    let inner = (!body.is_end_stream()).then_some(body);
    let state = Arc::new(Mutex::new(ReplayState {
        inner,
        chunks: VecDeque::new(),
        buffered_bytes: 0,
        limit,
        overflowed: false,
    }));
    (
        AttemptBody {
            state: Arc::clone(&state),
            size_hint,
        },
        RetryBodyHandle { state },
    )
}

impl<B> RetryBodyHandle<B> {
    /// Extracts a replayable body for a single retry, or returns `None` when
    /// more than `limit` bytes or trailers were already sent on the dead socket.
    pub(super) fn take_retry_body(self) -> Option<ReplayedBody<B>> {
        let mut state = lock_recovering(&self.state);
        if state.overflowed {
            return None;
        }
        let chunks = std::mem::take(&mut state.chunks);
        let remaining = state.inner.take();
        drop(state);
        Some(ReplayedBody { chunks, remaining })
    }
}

impl<B> Body for AttemptBody<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut state = lock_recovering(&self.state);
        let Some(inner) = state.inner.as_mut() else {
            return Poll::Ready(None);
        };
        let polled = Pin::new(&mut *inner).poll_frame(context);
        let end_stream = inner.is_end_stream();
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if !state.overflowed {
                        if state.buffered_bytes.saturating_add(data.len()) <= state.limit {
                            state.buffered_bytes += data.len();
                            state.chunks.push_back(data.clone());
                        } else {
                            state.overflowed = true;
                            state.chunks.clear();
                        }
                    }
                } else {
                    // Trailers are not buffered for replay.
                    state.overflowed = true;
                    state.chunks.clear();
                }
                if end_stream {
                    state.inner = None;
                }
            }
            Poll::Ready(None) => {
                state.inner = None;
            }
            Poll::Ready(Some(Err(_))) => {
                state.overflowed = true;
                state.chunks.clear();
                state.inner = None;
            }
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        lock_recovering(&self.state).inner.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint
    }
}

impl<B> Body for ReplayedBody<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if let Some(chunk) = this.chunks.pop_front() {
            return Poll::Ready(Some(Ok(Frame::data(chunk))));
        }
        let Some(remaining) = this.remaining.as_mut() else {
            return Poll::Ready(None);
        };
        let polled = Pin::new(&mut *remaining).poll_frame(context);
        if matches!(polled, Poll::Ready(None)) || remaining.is_end_stream() {
            this.remaining = None;
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.chunks.is_empty() && self.remaining.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderValue, Request};
    use http_body_util::{BodyExt, Empty};
    use hyper_util::rt::TokioIo;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn open_pair() -> io::Result<(PooledConnection, TcpStream)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (client, server) = tokio::try_join!(TcpStream::connect(addr), async {
            let (stream, _) = listener.accept().await?;
            Ok(stream)
        })?;
        let (sender, connection) = client_http1::handshake(TokioIo::new(client))
            .await
            .map_err(io::Error::other)?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        })
        .abort_handle();
        Ok((
            PooledConnection::fresh(ConnectionId::next(), sender, driver),
            server,
        ))
    }

    #[test]
    fn pool_keys_partition_scheme_host_port_and_tls_verification()
    -> Result<(), Box<dyn std::error::Error>> {
        let lower = PoolKey::plain(&Authority::from_static("example.com"), 80);
        let upper = PoolKey::plain(&Authority::from_static("EXAMPLE.COM:80"), 80);
        assert_eq!(lower, upper);

        let ipv6 = PoolKey::plain(&Authority::from_static("[::1]:8080"), 80);
        assert_eq!(
            ipv6,
            PoolKey::Plain {
                host: "::1".to_owned(),
                port: 8080,
            }
        );

        let dest_a = Destination::from_authority(&Authority::from_static("example.com:443"))?;
        let dest_b = Destination::from_authority(&Authority::from_static("other.example:443"))?;
        let https_sys = PoolKey::https(&dest_a, UpstreamTlsKey::SystemRoots);
        let https_root_a =
            PoolKey::https(&dest_a, UpstreamTlsKey::SystemPlusAdditionalRoots([1; 32]));
        let https_root_b =
            PoolKey::https(&dest_a, UpstreamTlsKey::SystemPlusAdditionalRoots([2; 32]));
        let https_other_host = PoolKey::https(&dest_b, UpstreamTlsKey::SystemRoots);
        let plain_443 = PoolKey::plain(&Authority::from_static("example.com:443"), 80);

        assert_ne!(https_sys, plain_443);
        assert_ne!(https_sys, https_root_a);
        assert_ne!(https_root_a, https_root_b);
        assert_ne!(https_sys, https_other_host);
        Ok(())
    }

    #[test]
    fn classifies_idempotent_methods_and_keep_alive_headers() {
        for method in [
            Method::GET,
            Method::HEAD,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::TRACE,
        ] {
            assert!(is_idempotent_method(&method), "{method}");
        }
        for method in [Method::POST, Method::PATCH, Method::CONNECT] {
            assert!(!is_idempotent_method(&method), "{method}");
        }

        let empty = HeaderMap::new();
        assert!(response_allows_keep_alive(Version::HTTP_11, &empty));
        assert!(!response_allows_keep_alive(Version::HTTP_10, &empty));

        let mut close = HeaderMap::new();
        close.append(CONNECTION, HeaderValue::from_static("keep-alive, Close"));
        assert!(!response_allows_keep_alive(Version::HTTP_11, &close));
        assert!(!response_allows_keep_alive(Version::HTTP_10, &close));

        let mut keep = HeaderMap::new();
        keep.append(CONNECTION, HeaderValue::from_static("Keep-Alive"));
        assert!(response_allows_keep_alive(Version::HTTP_10, &keep));
        assert!(response_allows_keep_alive(Version::HTTP_11, &keep));
    }

    #[tokio::test]
    async fn enforces_max_idle_per_host_and_closes_oldest_socket() -> io::Result<()> {
        let pool = UpstreamPool::new(2, Duration::from_secs(30));
        let key = PoolKey::Plain {
            host: "example.test".to_owned(),
            port: 80,
        };
        let (c1, mut s1) = open_pair().await?;
        let (c2, _s2) = open_pair().await?;
        let (c3, _s3) = open_pair().await?;
        let id2 = c2.id;
        let id3 = c3.id;

        pool.put(key.clone(), c1.id, c1.sender, c1.driver);
        pool.put(key.clone(), c2.id, c2.sender, c2.driver);
        assert_eq!(pool.idle_count(&key), 2);

        // Adding a third idle connection evicts and closes the oldest (c1).
        pool.put(key.clone(), c3.id, c3.sender, c3.driver);
        assert_eq!(pool.idle_count(&key), 2);
        let mut buf = [0_u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(2), s1.read(&mut buf))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "oldest socket stayed open"))??;
        assert_eq!(n, 0, "evicted idle connection must close its socket");

        // LIFO checkout returns c3 first, then c2.
        let out_first = pool
            .checkout(&key)
            .await
            .ok_or_else(|| io::Error::other("missing c3"))?;
        assert_eq!(out_first.id, id3);
        assert!(out_first.is_reused());
        let out_second = pool
            .checkout(&key)
            .await
            .ok_or_else(|| io::Error::other("missing c2"))?;
        assert_eq!(out_second.id, id2);
        assert!(out_second.is_reused());
        assert_eq!(pool.idle_count(&key), 0);

        out_first.close();
        out_second.close();
        Ok(())
    }

    #[tokio::test]
    async fn evicts_expired_idle_connections_and_closes_all_on_shutdown() -> io::Result<()> {
        let pool = UpstreamPool::new(4, Duration::from_millis(40));
        let key = PoolKey::Plain {
            host: "example.test".to_owned(),
            port: 80,
        };
        let (c1, mut s1) = open_pair().await?;
        pool.put(key.clone(), c1.id, c1.sender, c1.driver);
        assert_eq!(pool.total_idle(), 1);

        tokio::time::sleep(Duration::from_millis(60)).await;
        pool.evict_expired();
        assert_eq!(pool.total_idle(), 0);
        let mut buf = [0_u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(2), s1.read(&mut buf))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "expired socket stayed open"))??;
        assert_eq!(n, 0);

        let (c2, mut s2) = open_pair().await?;
        pool.put(key.clone(), c2.id, c2.sender, c2.driver);
        assert_eq!(pool.total_idle(), 1);
        pool.close_all();
        assert_eq!(pool.total_idle(), 0);
        let n = tokio::time::timeout(Duration::from_secs(2), s2.read(&mut buf))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "closed pool left socket open")
            })??;
        assert_eq!(n, 0);

        // Once closed, new returns are immediately discarded.
        let (c3, mut s3) = open_pair().await?;
        pool.put(key, c3.id, c3.sender, c3.driver);
        assert_eq!(pool.total_idle(), 0);
        let n = tokio::time::timeout(Duration::from_secs(2), s3.read(&mut buf))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "post-close socket stayed open")
            })??;
        assert_eq!(n, 0);
        Ok(())
    }

    #[tokio::test]
    async fn does_not_share_connections_across_different_tls_keys() -> io::Result<()> {
        let pool = UpstreamPool::new(4, Duration::from_secs(30));
        let dest = Destination::from_authority(&Authority::from_static("localhost:8443"))
            .map_err(io::Error::other)?;
        let key_a = PoolKey::https(&dest, UpstreamTlsKey::SystemPlusAdditionalRoots([0xAA; 32]));
        let key_b = PoolKey::https(&dest, UpstreamTlsKey::SystemPlusAdditionalRoots([0xBB; 32]));

        let (c1, _s1) = open_pair().await?;
        pool.put(key_a.clone(), c1.id, c1.sender, c1.driver);
        assert!(pool.checkout(&key_b).await.is_none());
        assert_eq!(pool.idle_count(&key_a), 1);

        let checked_out = pool
            .checkout(&key_a)
            .await
            .ok_or_else(|| io::Error::other("expected pooled connection under key_a"))?;
        checked_out.close();
        Ok(())
    }

    #[tokio::test]
    async fn checkout_skips_connections_closed_by_peer() -> io::Result<()> {
        let pool = UpstreamPool::new(4, Duration::from_secs(30));
        let key = PoolKey::Plain {
            host: "example.test".to_owned(),
            port: 80,
        };
        let (c1, mut s1) = open_pair().await?;
        pool.put(key.clone(), c1.id, c1.sender, c1.driver);
        s1.shutdown().await?;
        drop(s1);

        // Wait briefly for the TCP FIN to reach the driver task.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(pool.checkout(&key).await.is_none());
        assert_eq!(pool.idle_count(&key), 0);
        Ok(())
    }

    #[tokio::test]
    async fn replayed_body_emits_captured_chunks_once() -> Result<(), Box<dyn std::error::Error>> {
        let (first, handle) = replayable_body(
            http_body_util::Full::new(Bytes::from_static(b"hello")),
            MAX_RETRY_BODY_BYTES,
        );
        let first_bytes = first.collect().await?.to_bytes();
        assert_eq!(&first_bytes[..], b"hello");

        let replayed = handle
            .take_retry_body()
            .ok_or("expected small body to be replayable")?;
        let retry_bytes = replayed.collect().await?.to_bytes();
        assert_eq!(&retry_bytes[..], b"hello");

        // Overflowing the retry buffer disables replay cleanly.
        let (big, big_handle) =
            replayable_body(http_body_util::Full::new(Bytes::from(vec![b'x'; 32])), 16);
        let _ = big.collect().await?;
        assert!(big_handle.take_retry_body().is_none());

        // Empty bodies are trivially replayable.
        let (empty, empty_handle) = replayable_body(Empty::<Bytes>::new(), MAX_RETRY_BODY_BYTES);
        assert!(empty.is_end_stream());
        let req = Request::builder().uri("/").body(empty)?;
        assert!(req.body().is_end_stream());
        let retry_empty = empty_handle
            .take_retry_body()
            .ok_or("empty body must be replayable")?;
        assert!(retry_empty.is_end_stream());
        Ok(())
    }
}
