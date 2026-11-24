use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::Response;
use http_body::{Body, Frame, SizeHint};
use http_inspector_exchange::capture::captured_response_head;
use http_inspector_exchange::{
    BodyLifecycle, BodyPreview, BodyPreviewWriter, BodySide, CapturedResponse,
};
use hyper::ext::ReasonPhrase;

use crate::recording::body_store::BodyRecorder;
use crate::recording::journal::FullLog;

pub(super) use http_inspector_exchange::capture::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, bounded_text, captured_request, response_body_preview,
};

/// Captures the response head received from the upstream server, preserving
/// any non-canonical HTTP/1 reason phrase parsed by Hyper.
pub(super) fn captured_response<B>(response: &Response<B>, body: BodyPreview) -> CapturedResponse {
    let reason = response
        .extensions()
        .get::<ReasonPhrase>()
        .map(ReasonPhrase::as_bytes);
    captured_response_head(
        response.status(),
        response.version(),
        reason,
        response.headers(),
        body,
    )
}

pub(super) struct BodyLog {
    log: FullLog,
    id: u64,
    side: BodySide,
    ended: bool,
    limited: bool,
}

impl BodyLog {
    pub(super) const fn new(log: FullLog, id: u64, side: BodySide) -> Self {
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
pub(super) struct BodySinks {
    pub(super) log: Option<BodyLog>,
    pub(super) store: Option<BodyRecorder>,
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

pub(super) struct ObservedBody<B> {
    body: B,
    preview: BodyPreviewWriter,
    sinks: BodySinks,
}

impl<B> ObservedBody<B>
where
    B: Body,
{
    pub(super) fn new(body: B, preview: BodyPreviewWriter, mut sinks: BodySinks) -> Self {
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
             [9] Response body: later bytes omitted because the session body limit of 3 B was reached\n\
             [9] Response body: complete\n"
        );
        Ok(())
    }
}
