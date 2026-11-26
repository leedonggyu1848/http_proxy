//! Chrome request detection and allowlisted outbound header extraction for HLS segment fetches.

use http::header::{HeaderName, HeaderValue};
use http_inspector_exchange::http_semantics::{REDACTED_VALUE, is_sensitive_header};
use http_inspector_exchange::{Exchange, HeaderPreview};

/// Headers copied from a captured same-origin segment request.
///
/// Sensitive credential headers (`Cookie`, `Authorization`, `Proxy-Authorization`),
/// redacted placeholders, hop-by-hop headers, and custom auth headers are never
/// included.
const SAME_ORIGIN_ALLOWLIST: &[&str] = &[
    "user-agent",
    "referer",
    "origin",
    "accept",
    "accept-language",
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "sec-fetch-dest",
    "sec-fetch-mode",
    "sec-fetch-site",
];

/// Non-origin-bound headers allowed when a segment lives on a different origin
/// or after a cross-origin redirect.
const CROSS_ORIGIN_ALLOWLIST: &[&str] = &["user-agent", "accept", "accept-language"];

const CHROME_TOKENS: &[&str] = &["Chrome/", "Chromium/", "HeadlessChrome/", "CriOS/"];
const NON_CHROME_TOKENS: &[&str] = &[
    "Edg/",
    "EdgA/",
    "EdgiOS/",
    "OPR/",
    "Opera/",
    "YaBrowser/",
    "SamsungBrowser/",
    "UCBrowser/",
    "Vivaldi/",
];

/// Reports whether a `User-Agent` value identifies Chrome or Chromium rather
/// than another browser or CLI client.
#[must_use]
pub fn is_chrome_user_agent(user_agent: &str) -> bool {
    let has_chrome = CHROME_TOKENS.iter().any(|token| user_agent.contains(token));
    let has_other_browser = NON_CHROME_TOKENS
        .iter()
        .any(|token| user_agent.contains(token));
    has_chrome && !has_other_browser
}

/// Reports whether `exchange` carries a Chrome/Chromium `User-Agent` header.
#[must_use]
pub fn is_chrome_request(exchange: &Exchange) -> bool {
    exchange
        .request()
        .headers()
        .get("user-agent")
        .any(is_chrome_user_agent)
}

/// Validated, safe outbound HTTP headers derived from a captured segment request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SafeRequestHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SafeRequestHeaders {
    /// Extracts allowlisted headers from a captured request on the same origin.
    #[must_use]
    pub fn from_same_origin(captured: &HeaderPreview) -> Self {
        Self::extract(captured, SAME_ORIGIN_ALLOWLIST)
    }

    /// Extracts only non-origin-bound headers (no `Referer`/`Origin`/`Sec-Fetch-*`).
    #[must_use]
    pub fn from_cross_origin(captured: &HeaderPreview) -> Self {
        Self::extract(captured, CROSS_ORIGIN_ALLOWLIST)
    }

    /// Returns a copy containing only headers safe to keep across a cross-origin redirect.
    #[must_use]
    pub fn for_cross_origin_redirect(&self) -> Self {
        let headers = self
            .headers
            .iter()
            .filter(|(name, _)| {
                CROSS_ORIGIN_ALLOWLIST
                    .iter()
                    .any(|allowed| name.as_str().eq_ignore_ascii_case(allowed))
            })
            .cloned()
            .collect();
        Self { headers }
    }

    /// Returns the retained `(name, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.headers.iter().map(|(name, value)| (name, value))
    }

    /// Returns the first retained value for `name` (ASCII case-insensitive), if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.as_str().eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.to_str().ok())
    }

    /// Returns the number of retained headers.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.headers.len()
    }

    /// Reports whether no headers were retained.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    fn extract(captured: &HeaderPreview, allowlist: &[&str]) -> Self {
        let mut headers = Vec::new();
        for header in captured.entries() {
            let name = header.name();
            let value = header.value();
            if !is_allowed_header(name, value, allowlist) {
                continue;
            }
            if let (Ok(header_name), Ok(header_value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.push((header_name, header_value));
            }
        }
        Self { headers }
    }
}

fn is_allowed_header(name: &str, value: &str, allowlist: &[&str]) -> bool {
    if is_sensitive_header(name) {
        return false;
    }
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == REDACTED_VALUE || trimmed.contains(REDACTED_VALUE) {
        return false;
    }
    allowlist
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

#[cfg(test)]
mod tests {
    use http_inspector_exchange::CapturedHeader;

    use super::*;

    #[test]
    fn distinguishes_chrome_from_other_clients() {
        assert!(is_chrome_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
        ));
        assert!(is_chrome_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0.0.0 Safari/537.36"
        ));
        assert!(is_chrome_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chromium/131.0.0.0 Safari/537.36"
        ));
        assert!(!is_chrome_user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0"
        ));
        assert!(!is_chrome_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"
        ));
        assert!(!is_chrome_user_agent("curl/8.7.1"));
        assert!(!is_chrome_user_agent(""));
    }

    #[test]
    fn allows_safe_same_origin_headers_and_excludes_sensitive_or_redacted_values() {
        let preview = HeaderPreview::new(
            vec![
                CapturedHeader::new(
                    "User-Agent".into(),
                    "Mozilla/5.0 Chrome/131.0.0.0 Safari/537.36".into(),
                ),
                CapturedHeader::new("Referer".into(), "https://player.example/watch".into()),
                CapturedHeader::new("Origin".into(), "https://player.example".into()),
                CapturedHeader::new("Cookie".into(), REDACTED_VALUE.into()),
                CapturedHeader::new("Authorization".into(), REDACTED_VALUE.into()),
                CapturedHeader::new("Proxy-Authorization".into(), REDACTED_VALUE.into()),
                CapturedHeader::new("X-Custom-Secret".into(), "secret-token".into()),
                CapturedHeader::new("Host".into(), "cdn.example".into()),
                CapturedHeader::new("Range".into(), "bytes=0-100".into()),
                CapturedHeader::new("Accept-Language".into(), "en-US,en;q=0.9".into()),
            ],
            false,
        );

        let same_origin = SafeRequestHeaders::from_same_origin(&preview);
        assert_eq!(
            same_origin.get("user-agent"),
            Some("Mozilla/5.0 Chrome/131.0.0.0 Safari/537.36")
        );
        assert_eq!(
            same_origin.get("referer"),
            Some("https://player.example/watch")
        );
        assert_eq!(same_origin.get("origin"), Some("https://player.example"));
        assert_eq!(same_origin.get("accept-language"), Some("en-US,en;q=0.9"));
        assert_eq!(same_origin.get("cookie"), None);
        assert_eq!(same_origin.get("authorization"), None);
        assert_eq!(same_origin.get("proxy-authorization"), None);
        assert_eq!(same_origin.get("x-custom-secret"), None);
        assert_eq!(same_origin.get("host"), None);
        assert_eq!(same_origin.get("range"), None);

        let cross = same_origin.for_cross_origin_redirect();
        assert_eq!(
            cross.get("user-agent"),
            Some("Mozilla/5.0 Chrome/131.0.0.0 Safari/537.36")
        );
        assert_eq!(cross.get("referer"), None);
        assert_eq!(cross.get("origin"), None);
    }

    #[test]
    fn excludes_redacted_placeholder_even_in_allowlisted_header() {
        let preview = HeaderPreview::new(
            vec![
                CapturedHeader::new("Referer".into(), REDACTED_VALUE.into()),
                CapturedHeader::new("User-Agent".into(), "Chrome/131.0".into()),
            ],
            false,
        );
        let headers = SafeRequestHeaders::from_same_origin(&preview);
        assert_eq!(headers.get("referer"), None);
        assert_eq!(headers.get("user-agent"), Some("Chrome/131.0"));
    }
}
