use std::collections::BTreeSet;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderMap, Method, Response};
use http_body::{Body, Frame, SizeHint};
use hyper::ext::ReasonPhrase;

use crate::exchange::AuthScheme;
use crate::exchange::BodyLifecycle;
use crate::exchange::RetentionBudget;
use crate::exchange::{
    BodyPreview, BodyPreviewWriter, CapturedHeader, CapturedRequest, CapturedResponse,
    HeaderPreview,
};
use crate::exchange::{BodySide, ImageFormat, MediaType};
use crate::http_semantics::redacted_value;
use crate::recording::body_store::BodyRecorder;
use crate::recording::journal::FullLog;

/// Most bytes one non-image body keeps in memory for its preview.
pub const BODY_PREVIEW_BYTES: usize = 8 * 1024;
/// Most bytes one image response keeps in memory so it can be drawn.
pub const IMAGE_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
/// Most bytes all image responses keep in memory together, across proxy runs.
pub const IMAGE_PREVIEW_BUDGET_BYTES: usize = 64 * 1024 * 1024;
static IMAGE_PREVIEW_BUDGET: RetentionBudget = RetentionBudget::new(IMAGE_PREVIEW_BUDGET_BYTES);
const HEADER_PREVIEW_BYTES: usize = 16 * 1024;
const HEADER_PREVIEW_FIELDS: usize = 64;
const TARGET_BYTES: usize = 4 * 1024;
pub(in crate::engine) const ERROR_BYTES: usize = 1024;

fn capture_headers(headers: &HeaderMap) -> HeaderPreview {
    let mut entries = Vec::new();
    let mut retained_bytes: usize = 0;
    let mut omitted = false;

    for (name, value) in headers {
        let value = String::from_utf8_lossy(redacted_value(name, value)).into_owned();
        let field_bytes = name.as_str().len().saturating_add(value.len());
        if entries.len() == HEADER_PREVIEW_FIELDS
            || retained_bytes.saturating_add(field_bytes) > HEADER_PREVIEW_BYTES
        {
            omitted = true;
            break;
        }
        retained_bytes += field_bytes;
        entries.push(CapturedHeader::new(name.as_str().to_owned(), value));
    }

    HeaderPreview::new(entries, omitted)
}

/// Reports whether a media type names a supported image.
#[must_use]
pub fn is_previewable_image_media_type(media_type: &MediaType) -> bool {
    ImageFormat::from_media_type(media_type).is_some()
}

/// Captures the request line and headers as they arrived from the client.
pub(in crate::engine) fn captured_request(
    method: &Method,
    target: &str,
    headers: &HeaderMap,
) -> CapturedRequest {
    CapturedRequest::new(
        method.clone(),
        bounded_text(target, TARGET_BYTES),
        capture_headers(headers),
        capture_auth_schemes(headers),
    )
}

/// Captures the response head received from the upstream server.
pub(in crate::engine) fn captured_response<B>(
    response: &Response<B>,
    body: BodyPreview,
) -> CapturedResponse {
    let status = response.status();
    let reason = response.extensions().get::<ReasonPhrase>().map_or_else(
        || status.canonical_reason().unwrap_or_default().to_owned(),
        |phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned(),
    );
    CapturedResponse::new(
        status,
        response.version(),
        bounded_text(&reason, ERROR_BYTES),
        capture_headers(response.headers()),
        body,
    )
}

/// Creates the response-body preview: image responses keep enough bytes to be
/// drawn, within the shared image budget; others keep the default prefix.
/// An image whose declared length already exceeds the per-image limit could
/// never be drawn, so it keeps the default prefix without using the budget.
pub(in crate::engine) fn response_body_preview(headers: &HeaderMap) -> BodyPreviewWriter {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let image = header(http::header::CONTENT_TYPE)
        .and_then(MediaType::parse)
        .is_some_and(|media_type| is_previewable_image_media_type(&media_type));
    let fits = header(http::header::CONTENT_LENGTH)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_none_or(|length| length <= IMAGE_PREVIEW_BYTES as u64);
    if image && fits {
        BodyPreviewWriter::with_budget(IMAGE_PREVIEW_BYTES, Some(&IMAGE_PREVIEW_BUDGET))
    } else {
        BodyPreviewWriter::new(BODY_PREVIEW_BYTES)
    }
}

pub(in crate::engine) fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let (prefix, _) = value.split_at(value.floor_char_boundary(max_bytes));
    format!("{prefix}…")
}

/// Extracts recognized authentication scheme names from raw request headers
/// before values are redacted. No credential, token, or cookie bytes are kept.
fn capture_auth_schemes(headers: &HeaderMap) -> Vec<AuthScheme> {
    let mut schemes = BTreeSet::new();
    for value in headers.get_all(http::header::AUTHORIZATION) {
        let _ = schemes.insert(classify_authorization_value(value.as_bytes()));
    }
    if headers.contains_key(http::header::PROXY_AUTHORIZATION) {
        let _ = schemes.insert(AuthScheme::ProxyAuthorization);
    }
    if headers.contains_key(http::header::COOKIE) {
        let _ = schemes.insert(AuthScheme::Cookie);
    }
    for name in headers.keys() {
        match name.as_str() {
            "x-api-key" | "api-key" | "apikey" | "x-apikey" => {
                let _ = schemes.insert(AuthScheme::ApiKey);
            }
            "x-auth-token" | "x-access-token" => {
                let _ = schemes.insert(AuthScheme::Token);
            }
            _ => {}
        }
    }
    schemes.into_iter().collect()
}

fn classify_authorization_value(raw: &[u8]) -> AuthScheme {
    let Ok(text) = std::str::from_utf8(raw) else {
        return AuthScheme::AuthorizationOther;
    };
    let trimmed = text.trim();
    // Require whitespace separating the scheme name from credentials so a raw
    // single-token secret is never mistaken for a scheme.
    let Some((scheme, rest)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) else {
        return AuthScheme::AuthorizationOther;
    };
    if rest.trim().is_empty() {
        return AuthScheme::AuthorizationOther;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "bearer" => AuthScheme::Bearer,
        "basic" => AuthScheme::Basic,
        "digest" => AuthScheme::Digest,
        "oauth" => AuthScheme::OAuth,
        "aws4-hmac-sha256" => AuthScheme::AwsSigV4,
        "token" => AuthScheme::Token,
        "apikey" | "api-key" => AuthScheme::ApiKey,
        _ => AuthScheme::AuthorizationOther,
    }
}

pub(in crate::engine) struct BodyLog {
    log: FullLog,
    id: u64,
    side: BodySide,
    ended: bool,
    limited: bool,
}

impl BodyLog {
    pub(in crate::engine) const fn new(log: FullLog, id: u64, side: BodySide) -> Self {
        Self {
            log,
            id,
            side,
            ended: false,
            limited: false,
        }
    }

    /// Writes observed body bytes until the session body limit is reached,
    /// then records once that later bytes of this body were omitted.
    fn observe(&mut self, data: &[u8]) {
        if self.limited {
            return;
        }
        if !self.log.body(self.id, self.side, data) {
            self.limited = true;
            self.log.body_omitted(self.id, self.side);
        }
    }

    fn end(&mut self, state: BodyLifecycle) {
        if !self.ended {
            self.log.body_end(self.id, self.side, state);
            self.ended = true;
        }
    }
}

/// Where the bytes of one forwarded body are copied besides its preview.
pub(in crate::engine) struct BodySinks {
    pub(in crate::engine) log: Option<BodyLog>,
    pub(in crate::engine) store: Option<BodyRecorder>,
}

impl BodySinks {
    fn observe(&mut self, data: &[u8]) {
        if let Some(log) = self.log.as_mut() {
            log.observe(data);
        }
        if let Some(store) = self.store.as_mut() {
            store.observe(data);
        }
    }

    fn end(&mut self, state: BodyLifecycle) {
        if let Some(log) = self.log.as_mut() {
            log.end(state);
        }
        if let Some(store) = self.store.as_mut() {
            store.end(state);
        }
    }
}

pub(in crate::engine) struct ObservedBody<B> {
    body: B,
    preview: BodyPreviewWriter,
    sinks: BodySinks,
}

impl<B> ObservedBody<B>
where
    B: Body,
{
    pub(in crate::engine) fn new(
        body: B,
        preview: BodyPreviewWriter,
        mut sinks: BodySinks,
    ) -> Self {
        if body.is_end_stream() {
            preview.complete();
            sinks.end(BodyLifecycle::Complete);
        }
        Self {
            body,
            preview,
            sinks,
        }
    }
}

impl<B> Body for ObservedBody<B>
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
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.preview.observe(data);
                    this.sinks.observe(data);
                }
                if let Some(trailers) = frame.trailers_ref()
                    && let Some(log) = this.sinks.log.as_ref()
                {
                    log.log.trailers(log.id, log.side, trailers);
                }
                if this.body.is_end_stream() {
                    this.preview.complete();
                    this.sinks.end(BodyLifecycle::Complete);
                }
            }
            Poll::Ready(Some(Err(_))) => {
                this.preview.interrupt();
                this.sinks.end(BodyLifecycle::Interrupted);
            }
            Poll::Ready(None) => {
                this.preview.complete();
                this.sinks.end(BodyLifecycle::Complete);
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

impl<B> Drop for ObservedBody<B> {
    fn drop(&mut self) {
        self.preview.cancel();
        self.sinks.end(BodyLifecycle::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn previews_redact_sensitive_headers_and_replace_invalid_bytes()
    -> Result<(), http::header::InvalidHeaderValue> {
        let mut headers = HeaderMap::new();
        for name in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "set-cookie",
        ] {
            headers.insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_static("secret"),
            );
        }
        headers.insert("x-bytes", http::HeaderValue::from_bytes(b"\xff")?);
        let preview = capture_headers(&headers);
        for name in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "set-cookie",
        ] {
            assert_eq!(preview.get(name).next(), Some("[REDACTED]"));
        }
        assert_eq!(preview.get("x-bytes").next(), Some("\u{fffd}"));
        Ok(())
    }

    #[test]
    fn recognizes_previewable_image_media_types() {
        let previewable = |value| {
            MediaType::parse(value).is_some_and(|media| is_previewable_image_media_type(&media))
        };
        for value in [
            "image/png",
            "IMAGE/PNG",
            " image/png ; charset=binary",
            "image/jpeg",
            " IMAGE/JPEG ; q=1",
            "image/gif",
            "image/webp",
            "image/svg+xml",
            "image/bmp",
            "image/x-icon",
            "image/vnd.microsoft.icon",
        ] {
            assert!(previewable(value), "{value}");
        }
        for value in ["image/pngx", "image/avif", "text/plain; type=image/png", ""] {
            assert!(!previewable(value), "{value}");
        }
    }

    #[test]
    fn retains_image_bodies_that_can_be_drawn() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut map = HeaderMap::new();
            for &(name, value) in pairs {
                if let Ok(value) = http::HeaderValue::from_str(value) {
                    map.insert(name, value);
                }
            }
            map
        };
        let retained = |map: &HeaderMap| {
            let preview = response_body_preview(map);
            preview.observe(&vec![0; BODY_PREVIEW_BYTES + 1]);
            preview
                .preview()
                .snapshot()
                .map(|snapshot| snapshot.bytes().len())
        };
        let too_large = (IMAGE_PREVIEW_BYTES + 1).to_string();
        assert_eq!(
            retained(&headers(&[("content-type", "image/png")])),
            Some(BODY_PREVIEW_BYTES + 1)
        );
        assert_eq!(
            retained(&headers(&[
                ("content-type", "image/png"),
                ("content-length", &too_large)
            ])),
            Some(BODY_PREVIEW_BYTES)
        );
        for media_type in ["image/jpeg", "IMAGE/JPEG; charset=binary"] {
            assert_eq!(
                retained(&headers(&[("content-type", media_type)])),
                Some(BODY_PREVIEW_BYTES + 1)
            );
            assert_eq!(
                retained(&headers(&[
                    ("content-type", media_type),
                    ("content-length", &too_large)
                ])),
                Some(BODY_PREVIEW_BYTES)
            );
        }
        assert_eq!(
            retained(&headers(&[("content-type", "text/plain")])),
            Some(BODY_PREVIEW_BYTES)
        );
    }

    #[test]
    fn body_log_marks_the_omission_once_after_the_session_limit() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("limited.log");
        let log = FullLog::with_body_limit(3)?;
        let mut body = BodyLog::new(log.clone(), 9, BodySide::Response);
        body.observe(b"abcd");
        body.observe(b"ef");
        body.end(BodyLifecycle::Complete);
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        assert_eq!(
            std::fs::read_to_string(path)?,
            "[9] Response body bytes: abc\n\
             [9] Response body: later bytes omitted because the session body limit of 3 bytes was reached\n\
             [9] Response body: complete\n"
        );
        Ok(())
    }

    #[test]
    fn captures_auth_scheme_names_without_retaining_credentials() {
        let mut headers = HeaderMap::new();
        headers.append(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer eyJhbGciOiJIUzI1NiJ9.SECRET_JWT"),
        );
        headers.append(
            http::header::COOKIE,
            http::HeaderValue::from_static("session=SECRET_COOKIE_VAL"),
        );
        headers.append("x-api-key", http::HeaderValue::from_static("sk_live_9999"));

        let schemes = capture_auth_schemes(&headers);
        assert_eq!(
            schemes,
            vec![AuthScheme::Bearer, AuthScheme::ApiKey, AuthScheme::Cookie]
        );
        let debug = format!("{schemes:?}");
        assert!(!debug.contains("SECRET_JWT"));
        assert!(!debug.contains("SECRET_COOKIE_VAL"));
        assert!(!debug.contains("sk_live_9999"));

        // Single-word raw token without whitespace maps to AuthorizationOther.
        let mut raw_token = HeaderMap::new();
        raw_token.append(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("RAW_SECRET_NO_SPACE"),
        );
        assert_eq!(
            capture_auth_schemes(&raw_token),
            vec![AuthScheme::AuthorizationOther]
        );

        let mut basic = HeaderMap::new();
        basic.append(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("basic dXNlcjpwYXNz"),
        );
        assert_eq!(capture_auth_schemes(&basic), vec![AuthScheme::Basic]);
    }
}
