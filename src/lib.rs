#![forbid(unsafe_code)]

pub mod endpoint_schema;
mod engine;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod exchange;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod http_semantics;
pub mod recording;
pub mod settings;
pub mod text_search;
pub mod url_map;

pub use engine::BodySide;
pub use engine::{
    AuthScheme, BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureSource,
    CapturedHeader, CapturedRequest, CapturedResponse, Exchange, FailureStage, HeaderPreview,
    ImageFormat, MediaType, Outcome, RecordId, SessionId, Timing, TunnelPreview, TunnelSnapshot,
    TunnelState,
};
pub use engine::{
    BODY_PREVIEW_BYTES, IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES,
    is_previewable_image_media_type,
};
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
pub use recording::body_store::{
    BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES, BodyLayout, BodyStore, BulkProgress,
    BulkSave, BulkSummary, ContentRange, RangePlan, ReadBodyOutcome, SaveBody, SaveFormat,
    SavedBody, StoreGap, StoredBody, read_captured_body,
};
pub use recording::journal::{FullLog, SESSION_BODY_LIMIT_BYTES};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";

/// Default loopback proxy listener port.
pub const DEFAULT_PORT: u16 = 8080;
