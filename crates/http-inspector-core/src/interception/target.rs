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

/// Reason a plain HTTP request has no usable upstream target.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum TargetError {
    #[error("only http:// requests are supported without CONNECT")]
    UnsupportedScheme,
    #[error("the request has no upstream authority")]
    MissingAuthority,
    #[error("the request path is invalid")]
    InvalidPath,
}

/// Reads the upstream authority and origin-form path from a proxied request.
pub(super) fn upstream_target<B>(request: &Request<B>) -> Result<UpstreamTarget, TargetError> {
    if let Some(scheme) = request.uri().scheme_str()
        && scheme != "http"
    {
        return Err(TargetError::UnsupportedScheme);
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
        .ok_or(TargetError::MissingAuthority)?;
    let path = match request.uri().path_and_query() {
        Some(value) => value
            .as_str()
            .parse()
            .map_err(|_| TargetError::InvalidPath)?,
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
    fn rejects_unusable_targets_by_kind() -> Result<(), http::Error> {
        let request = Request::builder().uri("https://example.com/").body(())?;
        assert!(matches!(
            upstream_target(&request),
            Err(TargetError::UnsupportedScheme)
        ));
        let request = Request::builder().uri("/").body(())?;
        assert!(matches!(
            upstream_target(&request),
            Err(TargetError::MissingAuthority)
        ));
        let request = Request::builder()
            .uri("/path?q=1")
            .header(HOST, "example.com")
            .body(())?;
        assert!(upstream_target(&request).is_ok());
        Ok(())
    }

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
