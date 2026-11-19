use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};

use crate::{BodyPreview, CapturedHeader, FullLog, HeaderPreview};

pub(super) const BODY_PREVIEW_BYTES: usize = 8 * 1024;
const HEADER_PREVIEW_BYTES: usize = 16 * 1024;
const HEADER_PREVIEW_FIELDS: usize = 64;
pub(super) const TARGET_BYTES: usize = 4 * 1024;
pub(super) const ERROR_BYTES: usize = 1024;

pub(super) fn capture_headers(headers: &HeaderMap) -> HeaderPreview {
    let mut entries = Vec::new();
    let mut retained_bytes: usize = 0;
    let mut omitted = false;

    for (name, value) in headers {
        let value = if is_sensitive(name.as_str()) {
            "[REDACTED]".to_owned()
        } else {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        };
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

pub(super) fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let (prefix, _) = value.split_at(value.floor_char_boundary(max_bytes));
    format!("{prefix}…")
}

fn is_sensitive(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    )
}

pub(super) fn full_headers(headers: &HeaderMap) -> String {
    headers
        .iter()
        .fold(String::new(), |mut text, (name, value)| {
            let value = if is_sensitive(name.as_str()) {
                "[REDACTED]".to_owned()
            } else {
                value.as_bytes().escape_ascii().to_string()
            };
            text.push_str(name.as_str());
            text.push_str(": ");
            text.push_str(&value);
            text.push('\n');
            text
        })
}

pub(super) struct BodyLog {
    log: FullLog,
    id: u64,
    side: &'static str,
    ended: bool,
    limited: bool,
}

impl BodyLog {
    pub(super) const fn new(log: FullLog, id: u64, side: &'static str) -> Self {
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

    fn end(&mut self, state: &str) {
        if !self.ended {
            self.log.body_end(self.id, self.side, state);
            self.ended = true;
        }
    }
}

pub(super) struct ObservedBody<B> {
    body: B,
    preview: BodyPreview,
    log: Option<BodyLog>,
}

impl<B> ObservedBody<B>
where
    B: Body,
{
    pub(super) fn new(body: B, preview: BodyPreview, mut log: Option<BodyLog>) -> Self {
        if body.is_end_stream() {
            preview.complete();
            if let Some(log) = log.as_mut() {
                log.end("complete");
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
                    log.log.text(format!(
                        "[{}] {} trailers\n{}",
                        log.id,
                        log.side,
                        full_headers(trailers)
                    ));
                }
                if this.body.is_end_stream() {
                    this.preview.complete();
                    if let Some(log) = this.log.as_mut() {
                        log.end("complete");
                    }
                }
            }
            Poll::Ready(Some(Err(_))) => {
                this.preview.interrupt();
                if let Some(log) = this.log.as_mut() {
                    log.end("interrupted");
                }
            }
            Poll::Ready(None) => {
                this.preview.complete();
                if let Some(log) = this.log.as_mut() {
                    log.end("complete");
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
            log.end("cancelled");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn body_log_marks_the_omission_once_after_the_session_limit() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("limited.log");
        let log = FullLog::with_body_limit(3)?;
        let mut body = BodyLog::new(log.clone(), 9, "Response");
        body.observe(b"abcd");
        body.observe(b"ef");
        body.end("complete");
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        assert_eq!(
            std::fs::read_to_string(path)?,
            "[9] Response body bytes: abc\n\
             [9] Response body: later bytes omitted because the session body limit of 512 MiB was reached\n\
             [9] Response body: complete\n"
        );
        Ok(())
    }
}
