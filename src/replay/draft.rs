//! Explicit, validated manual replay drafts.

use std::io::Read as _;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use bytes::Bytes;
use http::header::HOST;
use http::{HeaderMap, Method, Request, Uri};
use http_body_util::Full;

use crate::exchange::{BodyLifecycle, BodyPreview, CaptureSource, Exchange};
use crate::http_semantics::{REDACTED_VALUE, is_sensitive_header, remove_hop_by_hop_headers};

/// Largest replacement body file the editor loads into memory.
pub const REPLAY_BODY_FILE_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

/// An editable header or cookie; disabled rows are never sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayField {
    /// Whether to include this row.
    pub enabled: bool,
    /// HTTP field name.
    pub name: String,
    /// Field value, empty for redacted captures.
    pub value: String,
}

/// Owned editor contents. Original sensitive values are never retained.
#[derive(Clone)]
pub struct ReplayDraft {
    /// Original capture.
    pub original: Exchange,
    /// HTTP method.
    pub method: String,
    /// Absolute HTTP or HTTPS URL.
    pub url: String,
    /// Editable end-to-end headers.
    pub headers: Vec<ReplayField>,
    /// Complete replacement body, shared with the sent request without copying.
    pub body: Bytes,
    /// Requires an explicit complete replacement before sending.
    pub body_missing: bool,
    /// Keep Host aligned with the edited URL.
    pub automatic_host: bool,
}

impl ReplayDraft {
    /// Copies a capture without attempting to recover redacted values.
    pub fn new(original: Exchange) -> Result<Self, String> {
        if original.source() == CaptureSource::Connect {
            return Err("CONNECT tunnels cannot be replayed".to_owned());
        }
        let snapshot = original.request().body().and_then(BodyPreview::snapshot);
        let body_missing = snapshot.as_ref().is_none_or(|body| {
            body.lifecycle() != BodyLifecycle::Complete
                || body.observed_bytes() != body.bytes().len() as u64
        }) || original.request().headers().omitted();
        let entries = original.request().headers().entries();
        let removed =
            framing_header_names(entries.iter().map(|field| (field.name(), field.value())));
        // Keep the captured text as-is; `request` rejects values that cannot be sent exactly.
        let headers = entries
            .iter()
            .filter(|field| !removed.contains(&field.name().to_ascii_lowercase()))
            .map(|field| ReplayField {
                enabled: true,
                name: field.name().to_owned(),
                value: if field.value() == REDACTED_VALUE {
                    String::new()
                } else {
                    field.value().to_owned()
                },
            })
            .collect();
        Ok(Self {
            method: original.request().method().to_string(),
            url: original.request().target().to_owned(),
            headers,
            body: snapshot.map_or_else(Bytes::new, |body| Bytes::copy_from_slice(body.bytes())),
            body_missing,
            automatic_host: true,
            original,
        })
    }

    /// Validates framing and fields before any network operation.
    pub fn request(&self, proxy: SocketAddr) -> Result<Request<Full<Bytes>>, String> {
        if self.body_missing {
            return Err(
                "Incomplete body or headers: load a complete replacement before sending".to_owned(),
            );
        }
        let method = self
            .method
            .parse::<Method>()
            .map_err(|_| "Invalid method")?;
        if method == Method::CONNECT || method == Method::TRACE {
            return Err("CONNECT and TRACE cannot be replayed".to_owned());
        }
        let uri = self.url.parse::<Uri>().map_err(|_| {
            if self.url.ends_with('…') {
                "The captured URL was truncated; enter the complete URL"
            } else {
                "Invalid URL"
            }
        })?;
        if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
            return Err("Use an absolute http:// or https:// URL".to_owned());
        }
        let host = uri.host().unwrap_or_default();
        if host.contains('@') || self.url.contains('#') {
            return Err("URL credentials and fragments are not supported".to_owned());
        }
        let port = uri.port_u16().unwrap_or_else(|| {
            if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            }
        });
        if port == proxy.port()
            && (host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback() || ip == proxy.ip()))
        {
            return Err("The URL targets this proxy itself".to_owned());
        }
        let mut request = Request::builder()
            .method(method)
            .uri(uri.clone())
            .body(Full::new(self.body.clone()))
            .map_err(|e| e.to_string())?;
        for field in self.headers.iter().filter(|field| field.enabled) {
            let (name, value) = header_pair(field)?;
            request.headers_mut().append(name, value);
        }
        remove_hop_by_hop_headers(request.headers_mut());
        request.headers_mut().remove(http::header::EXPECT);
        if self.automatic_host {
            let authority = uri.authority().ok_or("Missing URL authority")?;
            request.headers_mut().insert(
                HOST,
                authority.as_str().parse().map_err(|_| "Invalid Host")?,
            );
        }
        request.headers_mut().insert(
            http::header::CONTENT_LENGTH,
            self.body
                .len()
                .to_string()
                .parse()
                .map_err(|_| "Invalid body length")?,
        );
        request.extensions_mut().insert(self.original.id());
        Ok(request)
    }

    /// Replaces the body with a complete file, refusing files above
    /// [`REPLAY_BODY_FILE_LIMIT_BYTES`] before reading them.
    pub fn load_body_file(&mut self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let too_large = || {
            format!(
                "Body files are limited to {} MiB",
                REPLAY_BODY_FILE_LIMIT_BYTES / (1024 * 1024)
            )
        };
        let length = file.metadata().map_err(|error| error.to_string())?.len();
        if length > REPLAY_BODY_FILE_LIMIT_BYTES {
            return Err(too_large());
        }
        let mut body = Vec::with_capacity(usize::try_from(length).unwrap_or_default());
        // The file may grow after the size check; never read past the limit.
        file.take(REPLAY_BODY_FILE_LIMIT_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        if body.len() as u64 > REPLAY_BODY_FILE_LIMIT_BYTES {
            return Err(too_large());
        }
        self.body = Bytes::from(body);
        self.body_missing = self.original.request().headers().omitted();
        Ok(())
    }

    /// Whether sending requires the user's explicit side-effect confirmation.
    #[must_use]
    pub fn needs_confirmation(&self) -> bool {
        !matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS")
    }
}

/// Converts an enabled editor row, refusing values that would not be sent as entered.
fn header_pair(field: &ReplayField) -> Result<(http::HeaderName, http::HeaderValue), String> {
    let name = http::HeaderName::from_bytes(field.name.as_bytes())
        .map_err(|_| format!("Invalid header name: {}", field.name))?;
    if field.value == REDACTED_VALUE
        || (field.value.is_empty() && is_sensitive_header(name.as_str()))
    {
        return Err("Enter redacted header values or disable their rows".to_owned());
    }
    if field.value.contains(char::REPLACEMENT_CHARACTER) {
        return Err(format!(
            "Header {name} was captured with non-UTF-8 bytes and cannot be reproduced; \
             re-enter its value or disable the row"
        ));
    }
    let value = http::HeaderValue::from_str(&field.value)
        .map_err(|_| format!("Invalid value for header {name}"))?;
    Ok((name, value))
}

/// Lowercase names that the proxy regenerates or must not forward.
fn framing_header_names<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut headers = HeaderMap::new();
    for (name, value) in fields {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_str(value),
        ) {
            headers.append(name, value);
        }
    }
    let before: Vec<_> = headers.keys().map(ToString::to_string).collect();
    remove_hop_by_hop_headers(&mut headers);
    let mut removed: Vec<_> = before
        .into_iter()
        .filter(|name| !headers.contains_key(name.as_str()))
        .collect();
    removed.push(http::header::CONTENT_LENGTH.as_str().to_owned());
    removed
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::exchange::{
        BodyPreviewWriter, CapturedHeader, CapturedRequest, ExchangeWriter, HeaderPreview,
        RecordId, SessionId,
    };

    const PROXY: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);

    fn exchange(
        source: CaptureSource,
        method: Method,
        target: &str,
        headers: &[(&str, &str)],
        omitted_headers: bool,
        body: Option<(&[u8], usize, BodyLifecycle)>,
    ) -> Exchange {
        let headers = HeaderPreview::new(
            headers
                .iter()
                .map(|(name, value)| CapturedHeader::new((*name).into(), (*value).into()))
                .collect(),
            omitted_headers,
        );
        let writer = ExchangeWriter::start(
            RecordId::new(42),
            SessionId::next(),
            source,
            CapturedRequest::new(method, target.to_owned(), headers, Vec::new()),
        );
        if let Some((bytes, limit, end)) = body {
            let preview = BodyPreviewWriter::new(limit);
            preview.observe(bytes);
            match end {
                BodyLifecycle::Streaming => {}
                BodyLifecycle::Complete => preview.complete(),
                BodyLifecycle::Interrupted => preview.interrupt(),
                BodyLifecycle::Cancelled => preview.cancel(),
            }
            writer.attach_request_body(preview.preview());
        }
        writer.exchange().clone()
    }

    fn base_draft() -> Result<ReplayDraft, String> {
        let record = exchange(
            CaptureSource::HttpProxy,
            Method::GET,
            "http://example.test/items",
            &[("host", "example.test"), ("accept", "application/json")],
            false,
            Some((b"", 64, BodyLifecycle::Complete)),
        );
        ReplayDraft::new(record)
    }

    #[test]
    fn new_strips_framing_clears_redacted_and_tracks_missing_body() -> Result<(), String> {
        let connect = exchange(
            CaptureSource::Connect,
            Method::CONNECT,
            "example.test:443",
            &[],
            false,
            None,
        );
        assert!(ReplayDraft::new(connect).is_err());

        let complete = exchange(
            CaptureSource::HttpProxy,
            Method::POST,
            "http://example.test/submit",
            &[
                ("connection", "keep-alive, x-hop"),
                ("x-hop", "drop"),
                ("proxy-connection", "keep-alive"),
                ("content-length", "5"),
                ("authorization", REDACTED_VALUE),
                ("x-keep", "value"),
            ],
            false,
            Some((b"hello", 64, BodyLifecycle::Complete)),
        );
        let draft = ReplayDraft::new(complete)?;
        assert_eq!(draft.method, "POST");
        assert_eq!(draft.url, "http://example.test/submit");
        assert_eq!(draft.body.as_ref(), b"hello");
        assert!(!draft.body_missing);
        assert!(draft.automatic_host);
        assert_eq!(
            draft.headers,
            vec![
                ReplayField {
                    enabled: true,
                    name: "authorization".to_owned(),
                    value: String::new(),
                },
                ReplayField {
                    enabled: true,
                    name: "x-keep".to_owned(),
                    value: "value".to_owned(),
                },
            ]
        );

        let truncated = exchange(
            CaptureSource::HttpProxy,
            Method::POST,
            "http://example.test/",
            &[],
            false,
            Some((b"abcdef", 4, BodyLifecycle::Complete)),
        );
        assert!(ReplayDraft::new(truncated)?.body_missing);

        let streaming = exchange(
            CaptureSource::HttpProxy,
            Method::POST,
            "http://example.test/",
            &[],
            false,
            Some((b"ab", 64, BodyLifecycle::Streaming)),
        );
        assert!(ReplayDraft::new(streaming)?.body_missing);

        let omitted = exchange(
            CaptureSource::HttpProxy,
            Method::GET,
            "http://example.test/",
            &[],
            true,
            Some((b"", 64, BodyLifecycle::Complete)),
        );
        assert!(ReplayDraft::new(omitted)?.body_missing);
        Ok(())
    }

    #[test]
    fn request_validates_method_url_and_loop_guard() -> Result<(), String> {
        let mut draft = base_draft()?;
        for bad_method in ["", "GET POST", "CONNECT", "TRACE"] {
            draft.method = bad_method.to_owned();
            assert!(draft.request(PROXY).is_err(), "{bad_method}");
        }
        draft.method = "GET".to_owned();

        draft.url = "http://example.test/a b…".to_owned();
        let truncated_err = draft
            .request(PROXY)
            .err()
            .ok_or("truncated URL must fail")?;
        assert!(truncated_err.contains("truncated"), "{truncated_err}");

        for bad_url in [
            "not a url",
            "/relative/path",
            "ftp://example.test/file",
            "http://example.test/#section",
            "http://127.0.0.1:8080/loop",
            "http://localhost:8080/loop",
            "http://[::1]:8080/loop",
        ] {
            draft.url = bad_url.to_owned();
            assert!(draft.request(PROXY).is_err(), "{bad_url}");
        }

        for ok_url in [
            "http://127.0.0.1:9090/ok",
            "https://localhost/ok",
            "https://example.test:8443/ok?q=1",
        ] {
            draft.url = ok_url.to_owned();
            assert!(draft.request(PROXY).is_ok(), "{ok_url}");
        }
        Ok(())
    }

    #[test]
    fn request_validates_headers_host_and_framing() -> Result<(), String> {
        let mut draft = base_draft()?;
        draft.body_missing = true;
        assert!(draft.request(PROXY).is_err());
        draft.body_missing = false;

        for (name, value) in [
            ("bad name", "ok"),
            ("x-val", "\r\n"),
            ("authorization", ""),
            ("cookie", ""),
            ("x-secret", REDACTED_VALUE),
            ("x-raw", "a\u{fffd}b"),
        ] {
            draft.headers.push(ReplayField {
                enabled: true,
                name: name.to_owned(),
                value: value.to_owned(),
            });
            assert!(draft.request(PROXY).is_err(), "{name}: {value}");
            if let Some(last) = draft.headers.last_mut() {
                last.enabled = false;
            }
            assert!(draft.request(PROXY).is_ok(), "disabled {name}");
            draft.headers.pop();
        }

        draft.headers.push(ReplayField {
            enabled: true,
            name: "expect".to_owned(),
            value: "100-continue".to_owned(),
        });
        draft.headers.push(ReplayField {
            enabled: true,
            name: "connection".to_owned(),
            value: "x-hop".to_owned(),
        });
        draft.headers.push(ReplayField {
            enabled: true,
            name: "x-hop".to_owned(),
            value: "drop".to_owned(),
        });
        draft.url = "http://updated.test:8081/path".to_owned();
        draft.body = Bytes::from_static("한글".as_bytes());
        let request = draft.request(PROXY)?;
        assert_eq!(
            request.headers().get(HOST).and_then(|v| v.to_str().ok()),
            Some("updated.test:8081")
        );
        assert_eq!(
            request
                .headers()
                .get(http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()),
            Some("6")
        );
        assert!(!request.headers().contains_key(http::header::EXPECT));
        assert!(!request.headers().contains_key("x-hop"));
        assert_eq!(
            request.extensions().get::<RecordId>().copied(),
            Some(RecordId::new(42))
        );

        draft.automatic_host = false;
        let manual_host = draft.request(PROXY)?;
        assert_eq!(
            manual_host
                .headers()
                .get(HOST)
                .and_then(|v| v.to_str().ok()),
            Some("example.test")
        );
        Ok(())
    }

    #[test]
    fn load_body_file_replaces_body_and_enforces_limit() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let ok_path = directory.path().join("body.bin");
        std::fs::write(&ok_path, b"payload")?;
        let mut draft = base_draft()?;
        draft.body_missing = true;
        draft.load_body_file(&ok_path)?;
        assert_eq!(draft.body.as_ref(), b"payload");
        assert!(!draft.body_missing);

        let large_path = directory.path().join("large.bin");
        std::fs::File::create(&large_path)?.set_len(REPLAY_BODY_FILE_LIMIT_BYTES + 1)?;
        assert!(draft.load_body_file(&large_path).is_err());
        assert_eq!(draft.body.as_ref(), b"payload");

        let omitted = exchange(
            CaptureSource::HttpProxy,
            Method::POST,
            "http://example.test/",
            &[],
            true,
            None,
        );
        let mut omitted_draft = ReplayDraft::new(omitted)?;
        omitted_draft.load_body_file(&ok_path)?;
        assert!(omitted_draft.body_missing);
        Ok(())
    }

    #[test]
    fn needs_confirmation_flags_methods_with_side_effects() -> Result<(), String> {
        let mut draft = base_draft()?;
        for safe in ["GET", "HEAD", "OPTIONS"] {
            draft.method = safe.to_owned();
            assert!(!draft.needs_confirmation(), "{safe}");
        }
        for unsafe_method in ["POST", "PUT", "PATCH", "DELETE"] {
            draft.method = unsafe_method.to_owned();
            assert!(draft.needs_confirmation(), "{unsafe_method}");
        }
        Ok(())
    }
}
