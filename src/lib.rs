#![forbid(unsafe_code)]

mod browser;
#[allow(
    clippy::redundant_pub_crate,
    reason = "Items are re-exported at the crate root for sibling modules."
)]
mod capture;
mod domain;
mod journal;
mod proxy;
#[allow(
    clippy::redundant_pub_crate,
    reason = "Items are re-exported at the crate root for sibling modules."
)]
mod tls;
mod trust;

pub(crate) use capture::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, ObservedBody, TARGET_BYTES, bounded_text, capture_headers,
    response_body_preview,
};
pub use capture::{IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES, is_png_media_type};

pub use browser::{Browser, BrowserEvent, BrowserSession};
pub use domain::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureSource, CapturedHeader,
    CapturedRequest, CapturedResponse, Exchange, FailureStage, HeaderPreview, Outcome, RecordId,
    SessionId, Timing, TunnelPreview, TunnelSnapshot, TunnelState,
};
pub use journal::{FullLog, SESSION_BODY_LIMIT_BYTES};
pub use proxy::{EngineState, ProxyConfig, ProxyHandle, StartError, StopError};
pub use tls::{CaInfo, HttpsInspection, HttpsSetupError};
pub(crate) use tls::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
pub use trust::{CaTrustError, CaTrustStatus, CaTrustStore};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";
