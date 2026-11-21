#![forbid(unsafe_code)]

mod engine;

pub use engine::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureSource, CapturedHeader,
    CapturedRequest, CapturedResponse, Exchange, FailureStage, HeaderPreview, ImageFormat, Outcome,
    RecordId, SessionId, Timing, TunnelPreview, TunnelSnapshot, TunnelState,
};
pub use engine::{Browser, BrowserEvent, BrowserSession};
pub use engine::{CaInfo, HttpsInspection, HttpsSetupError};
pub use engine::{CaTrustError, CaTrustStatus, CaTrustStore};
pub use engine::{
    EngineState, ProxyConfig, ProxyHandle, REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField,
    ReplayTask, StartError, StopError,
};
pub use engine::{FullLog, SESSION_BODY_LIMIT_BYTES};
pub use engine::{
    IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES, is_previewable_image_media_type,
};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";

/// Default loopback proxy listener port.
pub const DEFAULT_PORT: u16 = 8080;
