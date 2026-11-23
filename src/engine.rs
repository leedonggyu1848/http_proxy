mod browser;
mod capture;
mod curl;
#[cfg(feature = "test-support")]
mod exchange_builder;
mod proxy;

pub use crate::exchange::BodySide;
pub use crate::exchange::{
    AuthScheme, BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureSource,
    CapturedHeader, CapturedRequest, CapturedResponse, Exchange, FailureStage, HeaderPreview,
    ImageFormat, MediaType, Outcome, RecordId, SessionId, Timing, TunnelPreview, TunnelSnapshot,
    TunnelState,
};
pub use browser::{Browser, BrowserEvent, BrowserSession};
pub use capture::{
    BODY_PREVIEW_BYTES, IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES,
    is_previewable_image_media_type,
};
pub use curl::{
    CURL_REDACTED_PLACEHOLDER, CurlCommand, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};
#[cfg(feature = "test-support")]
pub use exchange_builder::{ConnectExchangeBuilder, ExchangeBuilder, ResponseFixture};
pub use proxy::{
    EngineState, ProxyConfig, ProxyHandle, REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField,
    ReplayTask, StartError, StopError,
};
