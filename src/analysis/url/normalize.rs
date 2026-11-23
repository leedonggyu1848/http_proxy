//! URL parsing, canonical normalization, percent-decoding for display, and
//! query-parameter name extraction (never retaining query values).

use std::fmt;

use crate::CaptureSource;

/// Maximum length of a single normalized path segment stored in the map.
const MAX_SEGMENT_BYTES: usize = 256;
/// Maximum length of a single query parameter name stored in the map.
const MAX_QUERY_NAME_BYTES: usize = 128;

/// Normalized origin root (`scheme://host[:port]`) used as top-level tree keys.
///
/// Hosts are ASCII-lowercased and default ports (`80` for `http`, `443` for
/// `https`) are omitted.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OriginKey {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl OriginKey {
    /// Creates a normalized origin key, dropping default ports for `http`/`https`.
    #[must_use]
    pub fn new(scheme: &str, host: &str, port: Option<u16>) -> Self {
        let scheme = scheme.trim().to_ascii_lowercase();
        let host = normalize_host(host);
        let port = normalize_port(&scheme, port);
        Self { scheme, host, port }
    }

    /// Returns the lowercase URL scheme (e.g. `http`, `https`).
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the lowercase host (brackets retained for IPv6 literals).
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the non-default port, if any.
    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }

    /// Returns `host` or `host:port` when a non-default port is present.
    #[must_use]
    pub fn authority(&self) -> String {
        self.port
            .map_or_else(|| self.host.clone(), |port| format!("{}:{port}", self.host))
    }
}

impl fmt::Display for OriginKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.port {
            Some(port) => write!(f, "{}://{}:{port}", self.scheme, self.host),
            None => write!(f, "{}://{}", self.scheme, self.host),
        }
    }
}

/// Canonical URL representation stored by the URL map.
///
/// Query values and fragments are stripped during parsing and are never stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedUrl {
    origin: OriginKey,
    segments: Vec<String>,
    query_param_names: Vec<String>,
    is_tunnel: bool,
}

impl NormalizedUrl {
    /// Constructs a normalized URL from pre-validated components.
    #[must_use]
    pub const fn new(
        origin: OriginKey,
        segments: Vec<String>,
        query_param_names: Vec<String>,
        is_tunnel: bool,
    ) -> Self {
        Self {
            origin,
            segments,
            query_param_names,
            is_tunnel,
        }
    }

    /// Returns the normalized origin (`scheme://host[:port]`).
    #[must_use]
    pub const fn origin(&self) -> &OriginKey {
        &self.origin
    }

    /// Returns normalized path segments (raw percent-encoding preserved, empty segments removed).
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Returns query parameter names observed in the URL. Values are never stored.
    #[must_use]
    pub fn query_param_names(&self) -> &[String] {
        &self.query_param_names
    }

    /// Reports whether this target came from an opaque `CONNECT` tunnel without path info.
    #[must_use]
    pub const fn is_tunnel(&self) -> bool {
        self.is_tunnel
    }

    /// Formats `scheme://host[:port]/path` without query string or fragment.
    #[must_use]
    pub fn path_only(&self) -> String {
        if self.is_tunnel {
            return self.origin.to_string();
        }
        if self.segments.is_empty() {
            return format!("{}/", self.origin);
        }
        let mut out = self.origin.to_string();
        for segment in &self.segments {
            out.push('/');
            out.push_str(segment);
        }
        out
    }
}

/// Parses a captured exchange target into a [`NormalizedUrl`].
///
/// For `CaptureSource::Connect`, parses `host:port` authority form as an opaque
/// `https` tunnel origin with no path segments.
#[must_use]
pub fn parse_exchange_target(target: &str, source: CaptureSource) -> Option<NormalizedUrl> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    if source == CaptureSource::Connect && !trimmed.contains("://") {
        let (host, port) = split_authority(trimmed)?;
        let origin = OriginKey::new("https", host, port);
        return Some(NormalizedUrl::new(origin, Vec::new(), Vec::new(), true));
    }
    parse_url(trimmed)
}

/// Parses an absolute URL (`http://...`, `https://...`) into a [`NormalizedUrl`].
///
/// Strips fragments and query values while keeping raw percent-encoded segments
/// and query parameter names.
#[must_use]
pub fn parse_url(raw: &str) -> Option<NormalizedUrl> {
    let without_fragment = raw.trim().split('#').next()?.trim();
    let (scheme, rest) = without_fragment.split_once("://")?;
    if !is_valid_scheme(scheme) {
        return None;
    }
    let (authority_part, path_and_query) = split_authority_and_rest(rest);
    let authority = strip_userinfo(authority_part);
    let (host, port) = split_authority(authority)?;
    let origin = OriginKey::new(scheme, host, port);

    let (path_part, query_part) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };
    let segments = normalize_path_segments(path_part);
    let query_param_names = query_part.map_or_else(Vec::new, extract_query_param_names);

    Some(NormalizedUrl::new(
        origin,
        segments,
        query_param_names,
        false,
    ))
}

/// Parses an HTTP `Origin` header value into an [`OriginKey`].
///
/// Returns `None` for `"null"` or malformed headers.
#[must_use]
pub fn parse_origin_header(raw: &str) -> Option<OriginKey> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }
    parse_url(trimmed).map(|url| url.origin)
}

/// Resolves an HTTP `Location` header value against `base`, returning a
/// [`NormalizedUrl`] with query values stripped.
#[must_use]
pub fn resolve_location(base: &NormalizedUrl, location: &str) -> Option<NormalizedUrl> {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(scheme_end) = trimmed.find("://")
        && let Some(scheme) = trimmed.get(..scheme_end)
        && is_valid_scheme(scheme)
    {
        return parse_url(trimmed);
    }
    if trimmed.starts_with("//") {
        let synthetic = format!("{}:{trimmed}", base.origin.scheme());
        return parse_url(&synthetic);
    }
    let without_fragment = trimmed.split('#').next()?.trim();
    let (path_part, query_part) = match without_fragment.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (without_fragment, None),
    };
    let segments = if path_part.starts_with('/') {
        resolve_dot_segments(path_part)
    } else if path_part.is_empty() {
        base.segments.clone()
    } else {
        let mut combined = base.segments.clone();
        // Relative paths replace the last segment (file name) unless base is root.
        let _ = combined.pop();
        let mut joined = String::new();
        for seg in &combined {
            joined.push('/');
            joined.push_str(seg);
        }
        joined.push('/');
        joined.push_str(path_part);
        resolve_dot_segments(&joined)
    };
    let query_param_names = query_part.map_or_else(Vec::new, extract_query_param_names);
    Some(NormalizedUrl::new(
        base.origin.clone(),
        segments,
        query_param_names,
        false,
    ))
}

/// Decodes percent-encoded octets in `raw` for display only.
///
/// Invalid percent escapes are left untouched; non-UTF-8 byte sequences are
/// lossily replaced so display never fails.
#[must_use]
pub fn percent_decode_display(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if !bytes.contains(&b'%') {
        return raw.to_owned();
    }
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%'
            && let (Some(&hi), Some(&lo)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(h), Some(l)) = (hex_value(hi), hex_value(lo))
        {
            decoded.push((h << 4) | l);
            index += 3;
            continue;
        }
        decoded.push(byte);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.bytes();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

fn split_authority_and_rest(rest: &str) -> (&str, &str) {
    for (idx, byte) in rest.bytes().enumerate() {
        if byte == b'/' || byte == b'?' {
            return rest.split_at(idx);
        }
    }
    (rest, "")
}

fn strip_userinfo(authority: &str) -> &str {
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_port)| host_port)
}

fn is_valid_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return !inner.is_empty()
            && inner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'%' | b'-'));
    }
    !host.bytes().any(|b| {
        b <= 0x20 || b == 0x7f || matches!(b, b'/' | b'?' | b'#' | b'\\' | b'@' | b'[' | b']')
    })
}

fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.is_empty() || authority.bytes().any(|b| b <= 0x20 || b == 0x7f) {
        return None;
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let end = after_open.find(']')?;
        let host = authority.get(..end + 2)?;
        if !is_valid_host(host) {
            return None;
        }
        let rest = after_open.get(end + 1..)?;
        if rest.is_empty() {
            return Some((host, None));
        }
        let port_str = rest.strip_prefix(':')?;
        let port = parse_optional_port(port_str).ok()?;
        return Some((host, port));
    }
    if let Some((host, port_str)) = authority.rsplit_once(':') {
        if !is_valid_host(host) {
            return None;
        }
        let port = parse_optional_port(port_str).ok()?;
        return Some((host, port));
    }
    if !is_valid_host(authority) {
        return None;
    }
    Some((authority, None))
}

fn parse_optional_port(port_str: &str) -> Result<Option<u16>, std::num::ParseIntError> {
    if port_str.is_empty() {
        return Ok(None);
    }
    port_str.parse::<u16>().map(Some)
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn normalize_port(scheme: &str, port: Option<u16>) -> Option<u16> {
    match (scheme, port) {
        ("http", Some(80)) | ("https", Some(443)) => None,
        (_, other) => other,
    }
}

fn normalize_path_segments(path: &str) -> Vec<String> {
    resolve_dot_segments(path)
}

fn resolve_dot_segments(path: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    for raw in path.split('/') {
        if raw.is_empty() || raw == "." {
            continue;
        }
        if raw == ".." {
            let _ = segments.pop();
            continue;
        }
        segments.push(bounded_str(raw, MAX_SEGMENT_BYTES));
    }
    segments
}

/// Extracts only parameter names from a raw query string (never retaining values).
fn extract_query_param_names(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let name = pair.split('=').next().unwrap_or_default().trim();
        if !name.is_empty() {
            names.push(bounded_str(name, MAX_QUERY_NAME_BYTES));
        }
    }
    names
}

/// Calls `visit` with each query parameter name of the absolute or
/// origin-form `target` (bounded exactly as [`NormalizedUrl::query_param_names`])
/// and its percent-decoded value.
///
/// The value is lent only for the duration of the call so callers can
/// classify it without keeping it.
pub fn visit_query_values(target: &str, mut visit: impl FnMut(String, &str)) {
    let without_fragment = target.split('#').next().unwrap_or_default();
    let Some((_, query)) = without_fragment.split_once('?') else {
        return;
    };
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let value = percent_decode_display(&value.replace('+', " "));
        visit(bounded_str(name, MAX_QUERY_NAME_BYTES), &value);
    }
}

fn bounded_str(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_owned();
    }
    let cut = raw.floor_char_boundary(max_bytes);
    raw.get(..cut).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_host_case_and_default_ports() {
        let plain_url = parse_url("HTTP://Example.COM:80/api/v1/").ok_or("parse");
        assert!(plain_url.is_ok());
        if let Ok(url) = plain_url {
            assert_eq!(url.origin().to_string(), "http://example.com");
            assert_eq!(url.segments(), ["api", "v1"]);
            assert_eq!(url.path_only(), "http://example.com/api/v1");
        }

        let tls_url = parse_url("https://API.Example.com:443///users//profile/").ok_or("p");
        assert!(tls_url.is_ok());
        if let Ok(url) = tls_url {
            assert_eq!(url.origin().to_string(), "https://api.example.com");
            assert_eq!(url.segments(), ["users", "profile"]);
        }

        let custom_port = parse_url("https://Example.com:8443/v2").ok_or("p");
        assert!(custom_port.is_ok());
        if let Ok(url) = custom_port {
            assert_eq!(url.origin().to_string(), "https://example.com:8443");
            assert_eq!(url.origin().port(), Some(8443));
        }
    }

    #[test]
    fn keeps_percent_encoding_in_keys_and_decodes_only_for_display() {
        let url = parse_url("https://example.com/docs/hello%20world/%ED%95%9C%EA%B8%80");
        assert!(url.is_some());
        if let Some(url) = url {
            assert_eq!(
                url.segments(),
                ["docs", "hello%20world", "%ED%95%9C%EA%B8%80"]
            );
            assert_eq!(percent_decode_display("hello%20world"), "hello world");
            assert_eq!(percent_decode_display("%ED%95%9C%EA%B8%80"), "한글");
            assert_eq!(percent_decode_display("bad%ZZescape"), "bad%ZZescape");
        }
    }

    #[test]
    fn strips_query_values_and_fragments_completely() {
        let url = parse_url(
            "https://example.com/auth/callback?access_token=SUPER_SECRET_123&state=XYZ_999&empty_val=&flag#frag=secret",
        );
        assert!(url.is_some());
        if let Some(url) = url {
            assert_eq!(url.segments(), ["auth", "callback"]);
            assert_eq!(
                url.query_param_names(),
                ["access_token", "state", "empty_val", "flag"]
            );
            let path_only = url.path_only();
            assert!(!path_only.contains("SUPER_SECRET_123"));
            assert!(!path_only.contains("XYZ_999"));
            assert!(!path_only.contains("frag"));
        }
    }

    #[test]
    fn parses_connect_tunnels_and_ipv6_authorities() {
        let tunnel = parse_exchange_target("Example.COM:443", CaptureSource::Connect);
        assert!(tunnel.is_some());
        if let Some(url) = tunnel {
            assert!(url.is_tunnel());
            assert_eq!(url.origin().to_string(), "https://example.com");
            assert_eq!(url.segments().len(), 0);
        }

        let ipv6 = parse_url("http://[::1]:8080/health");
        assert!(ipv6.is_some());
        if let Some(url) = ipv6 {
            assert_eq!(url.origin().host(), "[::1]");
            assert_eq!(url.origin().port(), Some(8080));
            assert_eq!(url.origin().to_string(), "http://[::1]:8080");
        }
    }

    #[test]
    fn resolves_relative_and_absolute_redirect_locations_without_query_values() {
        let Some(base) = parse_url("https://app.example.com/auth/login?next=secret") else {
            unreachable!("base URL is valid");
        };
        let abs = resolve_location(&base, "https://id.other.com/oauth?code=SECRET_CODE");
        assert_eq!(
            abs.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://id.other.com/oauth")
        );
        assert_eq!(
            abs.as_ref().map(NormalizedUrl::query_param_names),
            Some(&["code".to_owned()][..])
        );

        let root_rel = resolve_location(&base, "/dashboard/home?session=TOPSECRET");
        assert_eq!(
            root_rel.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://app.example.com/dashboard/home")
        );

        let proto_rel = resolve_location(&base, "//cdn.example.com/static/app.js?v=42");
        assert_eq!(
            proto_rel.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://cdn.example.com/static/app.js")
        );

        let rel = resolve_location(&base, "../welcome");
        assert_eq!(
            rel.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://app.example.com/welcome")
        );

        // Relative Location with `://` inside a query parameter value must resolve as relative
        // and strip all query values cleanly (e.g. OAuth / login `?next=https://...`).
        let oauth_rel = resolve_location(
            &base,
            "/login?next=https://app.example.com/home?token=SECRET_REDIRECT",
        );
        assert_eq!(
            oauth_rel.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://app.example.com/login")
        );
        assert_eq!(
            oauth_rel.as_ref().map(NormalizedUrl::query_param_names),
            Some(&["next".to_owned()][..])
        );
        assert!(
            oauth_rel
                .as_ref()
                .is_none_or(|url| !format!("{url:?}").contains("SECRET_REDIRECT"))
        );

        let dir_rel = resolve_location(&base, "callback?return_to=https://app.example.com/done");
        assert_eq!(
            dir_rel.as_ref().map(NormalizedUrl::path_only).as_deref(),
            Some("https://app.example.com/auth/callback")
        );
    }

    #[test]
    fn rejects_hosts_with_whitespace_or_control_characters() {
        assert_eq!(parse_url("http://bad host.example/path"), None);
        assert_eq!(parse_url("http://evil\r\n.example/path"), None);
        assert_eq!(parse_url("http://tab\t.example:8080/path"), None);
        assert_eq!(parse_url("http://[::1 bad]/path"), None);
        assert_eq!(parse_origin_header("https://bad host.example"), None);
    }

    #[test]
    fn parses_origin_header_and_rejects_null() {
        assert_eq!(parse_origin_header("null"), None);
        assert_eq!(parse_origin_header("NULL"), None);
        assert_eq!(
            parse_origin_header("https://Web.Example.com:443").map(|o| o.to_string()),
            Some("https://web.example.com".to_owned())
        );
    }
}
