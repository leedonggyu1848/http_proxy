#![forbid(unsafe_code)]

mod engine;

pub use engine::{
    BODY_PREVIEW_BYTES, IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES,
    is_previewable_image_media_type,
};
pub use engine::{
    BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES, BodyLayout, BodyStore, BulkProgress,
    BulkSave, BulkSummary, ContentRange, RangePlan, SaveBody, SaveFormat, SavedBody, StoreGap,
    StoredBody,
};
pub use engine::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureSource, CapturedHeader,
    CapturedRequest, CapturedResponse, Exchange, FailureStage, HeaderPreview, ImageFormat,
    MediaType, Outcome, RecordId, SessionId, Timing, TunnelPreview, TunnelSnapshot, TunnelState,
};
pub use engine::{BodySide, FullLog, SESSION_BODY_LIMIT_BYTES};
pub use engine::{Browser, BrowserEvent, BrowserSession};
pub use engine::{
    CURL_REDACTED_PLACEHOLDER, CurlCommand, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};
pub use engine::{CaInfo, HttpsInspection, HttpsSetupError};
pub use engine::{CaTrustError, CaTrustStatus, CaTrustStore};
pub use engine::{
    EngineState, ProxyConfig, ProxyHandle, REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField,
    ReplayTask, StartError, StopError,
};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";

/// Default loopback proxy listener port.
pub const DEFAULT_PORT: u16 = 8080;
