//! Validated HTTP/HTTPS URLs, origins, and RFC 8216 / RFC 3986 relative resolution.

use std::fmt;

use http::Uri;
use thiserror::Error;

/// Allowed URL scheme for HLS playlists and segments.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HttpScheme {
    Http,
    Https,
}

impl HttpScheme {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }

    fn parse(raw: &str) -> Result<Self, HlsUrlError> {
        if raw.eq_ignore_ascii_case("http") {
            Ok(Self::Http)
        } else if raw.eq_ignore_ascii_case("https") {
            Ok(Self::Https)
        } else {
            Err(HlsUrlError::UnsupportedScheme(raw.to_owned()))
        }
    }
}

/// RFC 6454 origin tuple `(scheme, host, port)` used to gate header reuse.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Origin {
    scheme: HttpScheme,
    host: String,
    port: u16,
}

impl Origin {
    #[must_use]
    pub const fn scheme(&self) -> HttpScheme {
        self.scheme
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

/// Reason a playlist or segment URL could not be parsed or resolved.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HlsUrlError {
    #[error("empty URL reference")]
    Empty,
    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),
    #[error("URL is missing a host authority")]
    MissingAuthority,
    #[error("URL contains invalid authority: {0}")]
    InvalidAuthority(String),
    #[error("URL contains invalid characters")]
    InvalidCharacters,
}

/// A validated, normalized absolute `http` or `https` URL without a fragment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HlsUrl {
    scheme: HttpScheme,
    host: String,
    port: u16,
    authority: String,
    path: String,
    query: Option<String>,
    normalized: String,
}

impl HlsUrl {
    /// Parses and normalizes an absolute `http://` or `https://` URL.
    pub fn parse_absolute(raw: &str) -> Result<Self, HlsUrlError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(HlsUrlError::Empty);
        }
        let no_fragment = strip_fragment(trimmed);
        validate_url_chars(no_fragment)?;
        let Some((scheme_raw, rest)) = split_scheme(no_fragment) else {
            return Err(HlsUrlError::UnsupportedScheme(String::new()));
        };
        let scheme = HttpScheme::parse(scheme_raw)?;
        let Some(after_slashes) = rest.strip_prefix("//") else {
            return Err(HlsUrlError::MissingAuthority);
        };
        parse_with_authority(scheme, after_slashes)
    }

    /// Resolves `reference` against `self` per RFC 8216 Section 4.1 and RFC 3986 Section 5.
    pub fn resolve(&self, reference: &str) -> Result<Self, HlsUrlError> {
        let trimmed = reference.trim();
        if trimmed.is_empty() {
            return Err(HlsUrlError::Empty);
        }
        let no_fragment = strip_fragment(trimmed);
        validate_url_chars(no_fragment)?;

        if let Some((scheme_raw, rest)) = split_scheme(no_fragment) {
            let scheme = HttpScheme::parse(scheme_raw)?;
            let Some(after_slashes) = rest.strip_prefix("//") else {
                return Err(HlsUrlError::MissingAuthority);
            };
            return parse_with_authority(scheme, after_slashes);
        }

        if let Some(after_slashes) = no_fragment.strip_prefix("//") {
            return parse_with_authority(self.scheme, after_slashes);
        }

        if no_fragment.is_empty() {
            // Fragment-only reference resolves to base without fragment.
            return Ok(self.clone());
        }

        if let Some(query) = no_fragment.strip_prefix('?') {
            return Ok(Self::build(
                self.scheme,
                self.host.clone(),
                self.port,
                self.authority.clone(),
                self.path.clone(),
                Some(query.to_owned()),
            ));
        }

        let (ref_path, ref_query) = split_path_query(no_fragment);
        let merged = if ref_path.starts_with('/') {
            ref_path.to_owned()
        } else {
            merge_relative_path(&self.path, ref_path)
        };
        let path = remove_dot_segments(&merged);
        Ok(Self::build(
            self.scheme,
            self.host.clone(),
            self.port,
            self.authority.clone(),
            path,
            ref_query.map(str::to_owned),
        ))
    }

    fn build(
        scheme: HttpScheme,
        host: String,
        port: u16,
        authority: String,
        path: String,
        query: Option<String>,
    ) -> Self {
        let path = if path.is_empty() {
            "/".to_owned()
        } else {
            path
        };
        let mut normalized = format!("{}://{}{}", scheme.as_str(), authority, path);
        if let Some(query) = &query {
            normalized.push('?');
            normalized.push_str(query);
        }
        Self {
            scheme,
            host,
            port,
            authority,
            path,
            query,
            normalized,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    #[must_use]
    pub const fn scheme(&self) -> HttpScheme {
        self.scheme
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn path_and_query(&self) -> String {
        self.query.as_ref().map_or_else(
            || self.path.clone(),
            |query| format!("{}?{query}", self.path),
        )
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        Origin {
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
        }
    }

    /// Returns the last non-empty path segment, without query parameters.
    #[must_use]
    pub fn last_path_segment(&self) -> Option<&str> {
        let segment = self.path.rsplit('/').next().unwrap_or_default();
        (!segment.is_empty()).then_some(segment)
    }

    /// Returns the lowercase file extension of the last path segment, if any.
    #[must_use]
    pub fn extension(&self) -> Option<String> {
        let segment = self.last_path_segment()?;
        let (stem, ext) = segment.rsplit_once('.')?;
        (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
    }

    /// Reports whether the URL path ends with `.ts` (ASCII case-insensitive).
    #[must_use]
    pub fn is_ts_segment(&self) -> bool {
        self.extension().is_some_and(|ext| ext == "ts")
    }

    /// Reports whether the URL path ends with `.m3u8` (ASCII case-insensitive).
    #[must_use]
    pub fn is_m3u8_playlist(&self) -> bool {
        self.extension().is_some_and(|ext| ext == "m3u8")
    }

    /// Converts the validated URL into an `http::Uri` suitable for Hyper requests.
    pub fn to_http_uri(&self) -> Result<Uri, HlsUrlError> {
        self.normalized
            .parse::<Uri>()
            .map_err(|_| HlsUrlError::InvalidCharacters)
    }
}

impl fmt::Display for HlsUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

fn strip_fragment(value: &str) -> &str {
    value.split('#').next().unwrap_or(value)
}

fn validate_url_chars(value: &str) -> Result<(), HlsUrlError> {
    if value
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(HlsUrlError::InvalidCharacters);
    }
    Ok(())
}

/// Splits a RFC 3986 scheme prefix (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`).
fn split_scheme(value: &str) -> Option<(&str, &str)> {
    let colon = value.find(':')?;
    let scheme = value.get(..colon)?;
    let first = scheme.bytes().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let valid = scheme
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'));
    if !valid {
        return None;
    }
    Some((scheme, value.get(colon + 1..)?))
}

fn split_path_query(value: &str) -> (&str, Option<&str>) {
    match value.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (value, None),
    }
}

fn parse_with_authority(scheme: HttpScheme, after_slashes: &str) -> Result<HlsUrl, HlsUrlError> {
    let authority_end = after_slashes
        .find(['/', '?'])
        .unwrap_or(after_slashes.len());
    let raw_authority = after_slashes.get(..authority_end).unwrap_or_default();
    if raw_authority.is_empty() || raw_authority.contains('@') {
        return Err(HlsUrlError::InvalidAuthority(raw_authority.to_owned()));
    }
    let rest = after_slashes.get(authority_end..).unwrap_or_default();
    let (raw_path, query) = split_path_query(rest);

    let parsed_authority = raw_authority
        .parse::<http::uri::Authority>()
        .map_err(|_| HlsUrlError::InvalidAuthority(raw_authority.to_owned()))?;
    let host_raw = parsed_authority.host();
    if host_raw.is_empty() {
        return Err(HlsUrlError::MissingAuthority);
    }
    let host = host_raw.to_ascii_lowercase();
    let port = parsed_authority
        .port_u16()
        .unwrap_or_else(|| scheme.default_port());
    let authority = format_authority(&host, port, scheme);
    let path = if raw_path.is_empty() {
        "/".to_owned()
    } else {
        remove_dot_segments(raw_path)
    };
    Ok(HlsUrl::build(
        scheme,
        host,
        port,
        authority,
        path,
        query.map(str::to_owned),
    ))
}

fn format_authority(host: &str, port: u16, scheme: HttpScheme) -> String {
    if port == scheme.default_port() {
        host.to_owned()
    } else {
        format!("{host}:{port}")
    }
}

fn merge_relative_path(base_path: &str, relative: &str) -> String {
    if base_path.is_empty() {
        return format!("/{relative}");
    }
    base_path.rfind('/').map_or_else(
        || format!("/{relative}"),
        |index| {
            let prefix = base_path.get(..=index).unwrap_or("/");
            format!("{prefix}{relative}")
        },
    )
}

/// RFC 3986 Section 5.2.4 `remove_dot_segments`.
fn remove_dot_segments(input: &str) -> String {
    let mut rest = input;
    let mut output: Vec<&str> = Vec::new();
    let mut trailing_slash = false;

    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("../") {
            rest = after;
        } else if let Some(after) = rest.strip_prefix("./") {
            rest = after;
        } else if rest.starts_with("/./") {
            rest = rest.get(2..).unwrap_or_default();
        } else if rest == "/." {
            rest = "";
            trailing_slash = true;
        } else if rest.starts_with("/../") {
            rest = rest.get(3..).unwrap_or_default();
            output.pop();
        } else if rest == "/.." {
            rest = "";
            output.pop();
            trailing_slash = true;
        } else if rest == "." || rest == ".." {
            rest = "";
        } else {
            let leading_slash = rest.starts_with('/');
            let search_start = usize::from(leading_slash);
            let next_slash = rest
                .get(search_start..)
                .and_then(|tail| tail.find('/'))
                .map(|offset| search_start + offset);
            let (segment, remaining) = next_slash.map_or((rest, ""), |index| {
                (
                    rest.get(..index).unwrap_or_default(),
                    rest.get(index..).unwrap_or_default(),
                )
            });
            output.push(segment);
            trailing_slash = false;
            rest = remaining;
        }
    }

    let mut result = output.concat();
    if trailing_slash && !result.ends_with('/') {
        result.push('/');
    }
    if !result.starts_with('/') {
        result.insert(0, '/');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_urls_per_rfc_8216_and_3986() -> Result<(), HlsUrlError> {
        let master = HlsUrl::parse_absolute("https://CDN.Example.COM:443/live/master.m3u8?tok=1")?;
        assert_eq!(
            master.as_str(),
            "https://cdn.example.com/live/master.m3u8?tok=1"
        );

        let variant = master.resolve("720p/index.m3u8?v=2")?;
        assert_eq!(
            variant.as_str(),
            "https://cdn.example.com/live/720p/index.m3u8?v=2"
        );

        assert_eq!(
            variant.resolve("seg0.ts")?.as_str(),
            "https://cdn.example.com/live/720p/seg0.ts"
        );
        assert_eq!(
            variant.resolve("../shared/./seg1.ts?sig=abc")?.as_str(),
            "https://cdn.example.com/live/shared/seg1.ts?sig=abc"
        );
        assert_eq!(
            variant.resolve("/abs/seg2.ts")?.as_str(),
            "https://cdn.example.com/abs/seg2.ts"
        );
        assert_eq!(
            variant.resolve("//other.example/seg3.ts")?.as_str(),
            "https://other.example/seg3.ts"
        );
        assert_eq!(
            variant
                .resolve("http://plain.example:80/seg4.ts#frag")?
                .as_str(),
            "http://plain.example/seg4.ts"
        );
        Ok(())
    }

    #[test]
    fn rejects_non_http_schemes_and_invalid_inputs() -> Result<(), HlsUrlError> {
        let base = HlsUrl::parse_absolute("https://example.test/stream/index.m3u8")?;
        assert!(matches!(
            base.resolve("file:///etc/passwd"),
            Err(HlsUrlError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            base.resolve("javascript:alert(1)"),
            Err(HlsUrlError::UnsupportedScheme(_))
        ));
        assert!(matches!(base.resolve(""), Err(HlsUrlError::Empty)));
        assert!(matches!(
            base.resolve("seg 0.ts"),
            Err(HlsUrlError::InvalidCharacters)
        ));
        assert!(matches!(
            HlsUrl::parse_absolute("http://user:pass@example.test/a.m3u8"),
            Err(HlsUrlError::InvalidAuthority(_))
        ));
        Ok(())
    }

    #[test]
    fn compares_origins_and_extensions() -> Result<(), HlsUrlError> {
        let a = HlsUrl::parse_absolute("http://Example.Test:80/a/seg0.TS?x=1")?;
        let b = HlsUrl::parse_absolute("http://example.test/b/seg1.ts")?;
        let c = HlsUrl::parse_absolute("https://example.test/b/seg1.ts")?;
        assert_eq!(a.origin(), b.origin());
        assert_ne!(a.origin(), c.origin());
        assert!(a.is_ts_segment());
        assert!(!a.is_m3u8_playlist());
        Ok(())
    }
}
