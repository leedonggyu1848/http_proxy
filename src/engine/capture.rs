use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};

use crate::engine::domain::BodyLifecycle;
use crate::engine::domain::ImageFormat;
use crate::engine::domain::RetentionBudget;
use crate::engine::domain::{BodyPreview, CapturedHeader, HeaderPreview};
use crate::engine::journal::BodySide;
use crate::engine::journal::FullLog;

pub(in crate::engine) const BODY_PREVIEW_BYTES: usize = 8 * 1024;
/// Most bytes one image response keeps in memory so it can be drawn.
pub const IMAGE_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
/// Most bytes all image responses keep in memory together, across proxy runs.
pub const IMAGE_PREVIEW_BUDGET_BYTES: usize = 64 * 1024 * 1024;
static IMAGE_PREVIEW_BUDGET: RetentionBudget = RetentionBudget::new(IMAGE_PREVIEW_BUDGET_BYTES);
const HEADER_PREVIEW_BYTES: usize = 16 * 1024;
const HEADER_PREVIEW_FIELDS: usize = 64;
pub(in crate::engine) const TARGET_BYTES: usize = 4 * 1024;
pub(in crate::engine) const ERROR_BYTES: usize = 1024;

pub(in crate::engine) fn capture_headers(headers: &HeaderMap) -> HeaderPreview {
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

/// Reports whether a `Content-Type` value names a supported image, ignoring case
/// and media-type parameters.
#[must_use]
pub fn is_previewable_image_media_type(content_type: &str) -> bool {
    ImageFormat::from_media_type(content_type).is_some()
}

/// Creates the response-body preview: image responses keep enough bytes to be
/// drawn, within the shared image budget; others keep the default prefix.
/// An image whose declared length already exceeds the per-image limit could
/// never be drawn, so it keeps the default prefix without using the budget.
pub(in crate::engine) fn response_body_preview(headers: &HeaderMap) -> BodyPreview {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let image = header(http::header::CONTENT_TYPE).is_some_and(is_previewable_image_media_type);
    let fits = header(http::header::CONTENT_LENGTH)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_none_or(|length| length <= IMAGE_PREVIEW_BYTES as u64);
    if image && fits {
        BodyPreview::with_budget(IMAGE_PREVIEW_BYTES, Some(&IMAGE_PREVIEW_BUDGET))
    } else {
        BodyPreview::new(BODY_PREVIEW_BYTES)
    }
}

pub(in crate::engine) fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let (prefix, _) = value.split_at(value.floor_char_boundary(max_bytes));
    format!("{prefix}…")
}

pub(in crate::engine) fn redacted_value<'a>(
    name: &http::HeaderName,
    value: &'a http::HeaderValue,
) -> &'a [u8] {
    if matches!(
        name.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) {
        b"[REDACTED]"
    } else {
        value.as_bytes()
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

pub(in crate::engine) struct ObservedBody<B> {
    body: B,
    preview: BodyPreview,
    log: Option<BodyLog>,
}

impl<B> ObservedBody<B>
where
    B: Body,
{
    pub(in crate::engine) fn new(body: B, preview: BodyPreview, mut log: Option<BodyLog>) -> Self {
        if body.is_end_stream() {
            preview.complete();
            if let Some(log) = log.as_mut() {
                log.end(BodyLifecycle::Complete);
            }
        }
        Self { body, preview, log }
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
                    if let Some(log) = this.log.as_mut() {
                        log.observe(data);
                    }
                }
                if let Some(trailers) = frame.trailers_ref()
                    && let Some(log) = this.log.as_ref()
                {
                    log.log.trailers(log.id, log.side, trailers);
                }
                if this.body.is_end_stream() {
                    this.preview.complete();
                    if let Some(log) = this.log.as_mut() {
                        log.end(BodyLifecycle::Complete);
                    }
                }
            }
            Poll::Ready(Some(Err(_))) => {
                this.preview.interrupt();
                if let Some(log) = this.log.as_mut() {
                    log.end(BodyLifecycle::Interrupted);
                }
            }
            Poll::Ready(None) => {
                this.preview.complete();
                if let Some(log) = this.log.as_mut() {
                    log.end(BodyLifecycle::Complete);
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

impl<B> Drop for ObservedBody<B> {
    fn drop(&mut self) {
        self.preview.cancel();
        if let Some(log) = self.log.as_mut() {
            log.end(BodyLifecycle::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn recognizes_previewable_image_media_types() {
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
            assert!(is_previewable_image_media_type(value), "{value}");
        }
        for value in ["image/pngx", "image/avif", "text/plain; type=image/png", ""] {
            assert!(!is_previewable_image_media_type(value), "{value}");
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
            preview.snapshot().map(|snapshot| snapshot.bytes().len())
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
}
