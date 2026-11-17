#![forbid(unsafe_code)]

#[allow(clippy::redundant_pub_crate)]
mod capture;
mod domain;
mod proxy;
#[allow(clippy::redundant_pub_crate)]
mod tls;

pub(crate) use capture::{
    BODY_PREVIEW_BYTES, ERROR_BYTES, ObservedBody, TARGET_BYTES, bounded_text, capture_headers,
};

pub use domain::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureOutcome, CaptureRecord,
    CapturedHeader, FailureStage, HeaderPreview, RecordId,
};
pub use proxy::{ProxyConfig, ProxyHandle, StartError, StopError};
pub use tls::{CaInfo, HttpsInspection, HttpsSetupError};
pub(crate) use tls::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";
