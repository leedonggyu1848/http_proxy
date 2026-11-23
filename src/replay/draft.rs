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
