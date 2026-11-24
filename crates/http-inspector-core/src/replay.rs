//! Manual request replay: draft validation and execution dispatch.

mod draft;

pub use crate::interception::ReplayTask;
use crate::interception::{ProxyHandle, ReplayExecutionError, execute_replay};
pub use draft::{
    REPLAY_BODY_FILE_LIMIT_BYTES, ReplayBodyFileError, ReplayDraft, ReplayError, ReplayField,
    ReplayHeaderError, ReplayUrlError,
};

impl ProxyHandle {
    /// Sends a user-approved draft on the running proxy, recording a new exchange.
    pub fn replay(&self, draft: &ReplayDraft) -> Result<ReplayTask, ReplayError> {
        let request = draft.request(self.local_addr())?;
        execute_replay(self, request).map_err(|error| match error {
            ReplayExecutionError::ProxyNotRunning => ReplayError::ProxyNotRunning,
            ReplayExecutionError::HttpsInspectionDisabled => ReplayError::HttpsInspectionDisabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http::Method;

    use super::*;
    use crate::exchange::{
        BodyPreviewWriter, CaptureSource, CapturedRequest, ExchangeWriter, HeaderPreview, RecordId,
        SessionId,
    };
    use crate::interception::ProxyConfig;

    fn sample_draft(url: &str) -> Result<ReplayDraft, ReplayError> {
        let writer = ExchangeWriter::start(
            RecordId::new(1),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(
                Method::GET,
                url.to_owned(),
                HeaderPreview::default(),
                vec![],
            ),
        );
        let preview = BodyPreviewWriter::new(64);
        preview.complete();
        writer.attach_request_body(preview.preview());
        ReplayDraft::new(writer.exchange().clone())
    }

    #[test]
    fn replay_reports_https_inspection_disabled_and_stopped_proxy() -> Result<(), Box<dyn Error>> {
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;

        let secure_draft = sample_draft("https://example.test/secure")?;
        let https_error = proxy
            .replay(&secure_draft)
            .err()
            .ok_or("HTTPS replay without inspection must fail")?;
        assert!(
            matches!(https_error, ReplayError::HttpsInspectionDisabled),
            "{https_error:?}"
        );
        assert_eq!(
            https_error.to_string(),
            "Enable HTTPS inspection before replaying HTTPS"
        );

        proxy.halt_for_test()?;

        let plain_draft = sample_draft("http://example.test/plain")?;
        let stopped_error = proxy
            .replay(&plain_draft)
            .err()
            .ok_or("replay on stopped proxy must fail")?;
        assert!(
            matches!(stopped_error, ReplayError::ProxyNotRunning),
            "{stopped_error:?}"
        );
        assert_eq!(stopped_error.to_string(), "Start the proxy before sending");
        Ok(())
    }
}
