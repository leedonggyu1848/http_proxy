//! Upstream target parsing, resolution, and self-reference detection.

use std::io;
use std::net::SocketAddr;

use http::header::HOST;
use http::uri::Authority;
use http::{Request, Uri};
use tokio::net::lookup_host;

/// Upstream destination of a plain HTTP request.
pub(super) struct UpstreamTarget {
    pub(super) authority: Authority,
    pub(super) path: Uri,
}

/// Reads the upstream authority and origin-form path from a proxied request.
pub(super) fn upstream_target<B>(request: &Request<B>) -> Result<UpstreamTarget, &'static str> {
    if let Some(scheme) = request.uri().scheme_str()
        && scheme != "http"
    {
        return Err("only http:// requests are supported without CONNECT");
    }

    let authority = request
        .uri()
        .authority()
        .cloned()
        .or_else(|| {
            request
                .headers()
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<Authority>().ok())
        })
        .ok_or("the request has no upstream authority")?;
    let path = match request.uri().path_and_query() {
        Some(value) => value
            .as_str()
            .parse()
            .map_err(|_| "the request path is invalid")?,
        None => Uri::from_static("/"),
    };
    Ok(UpstreamTarget { authority, path })
}

/// Reason an upstream address could not be used.
#[derive(Debug)]
pub(super) enum ResolveError {
    /// The host name did not resolve.
    Lookup(io::Error),
    /// The destination is this proxy's own listener.
    Loop,
}

/// Resolves `authority` and rejects addresses that reach the proxy itself.
pub(super) async fn resolve_upstream(
    authority: &Authority,
    default_port: u16,
    proxy_addr: SocketAddr,
) -> Result<Vec<SocketAddr>, ResolveError> {
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = authority.port_u16().unwrap_or(default_port);
    let addresses: Vec<SocketAddr> = lookup_host((host, port))
        .await
        .map_err(ResolveError::Lookup)?
        .collect();
    if addresses.is_empty() {
        return Err(ResolveError::Lookup(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{host} did not resolve to any address"),
        )));
    }
    if addresses
        .iter()
        .any(|address| reaches_proxy(*address, proxy_addr))
    {
        return Err(ResolveError::Loop);
    }
    Ok(addresses)
}

fn reaches_proxy(address: SocketAddr, proxy_addr: SocketAddr) -> bool {
    address.port() == proxy_addr.port()
        && (address.ip() == proxy_addr.ip() || address.ip().is_unspecified())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    const PROXY: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);

    #[test]
    fn detects_the_proxy_listener_and_unspecified_addresses() {
        assert!(reaches_proxy(PROXY, PROXY));
        assert!(reaches_proxy(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
            PROXY
        ));
        assert!(reaches_proxy(
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8080),
            PROXY
        ));
        assert!(!reaches_proxy(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081),
            PROXY
        ));
    }

    #[tokio::test]
    async fn resolves_localhost_to_the_proxy_as_a_loop() {
        let authority = Authority::from_static("localhost:8080");
        let result = resolve_upstream(&authority, 80, PROXY).await;
        assert!(matches!(result, Err(ResolveError::Loop)));
    }

    #[tokio::test]
    async fn resolves_bracketed_ipv6_literals() -> Result<(), String> {
        let authority = Authority::from_static("[::1]:9");
        let addresses = resolve_upstream(&authority, 80, PROXY)
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            addresses,
            vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 9)]
        );
        Ok(())
    }
}
