//! Removal of hop-by-hop header fields before a message is forwarded.

use http::HeaderMap;
use http::header::{
    CONNECTION, HeaderName, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
    TRANSFER_ENCODING, UPGRADE,
};

const FIXED_HOP_BY_HOP: [&str; 3] = [
    "proxy-connection",
    "keep-alive",
    "proxy-authentication-info",
];

/// Removes fields that apply only to the current connection (RFC 9110 7.6.1).
///
/// Fields named by every `Connection` header are removed first, regardless of
/// case or how many `Connection` headers the message carries.
pub(in crate::engine) fn remove_hop_by_hop_headers(headers: &mut HeaderMap) {
    for name in connection_options(headers) {
        headers.remove(name);
    }
    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }
    for name in FIXED_HOP_BY_HOP {
        headers.remove(name);
    }
}

fn connection_options(headers: &HeaderMap) -> Vec<HeaderName> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter_map(|token| HeaderName::from_bytes(token.as_bytes()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(fields: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in fields {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn removes_fields_listed_in_connection() {
        let mut map = headers(&[
            ("connection", "close, X-Internal"),
            ("x-internal", "secret"),
            ("x-kept", "yes"),
        ]);
        remove_hop_by_hop_headers(&mut map);
        assert!(!map.contains_key("x-internal"));
        assert!(!map.contains_key(CONNECTION));
        assert_eq!(map.get("x-kept"), Some(&HeaderValue::from_static("yes")));
    }

    #[test]
    fn handles_repeated_connection_headers_and_case() {
        let mut map = headers(&[
            ("connection", "X-First"),
            ("connection", " x-SECOND ,,"),
            ("x-first", "1"),
            ("x-second", "2"),
            ("x-second", "3"),
        ]);
        remove_hop_by_hop_headers(&mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn removes_proxy_authenticate_and_fixed_fields() {
        let mut map = headers(&[
            ("proxy-authenticate", "Basic"),
            ("proxy-authorization", "Basic abc"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("content-type", "text/plain"),
        ]);
        remove_hop_by_hop_headers(&mut map);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("content-type"));
    }
}
