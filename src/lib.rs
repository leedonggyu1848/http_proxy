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
};

pub use browser::{Browser, BrowserEvent, BrowserSession};
pub use domain::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, CaptureOutcome, CaptureRecord,
    CapturedHeader, FailureStage, HeaderPreview, RecordId,
};
pub use journal::FullLog;
pub use proxy::{ProxyConfig, ProxyHandle, StartError, StopError};
pub use tls::{CaInfo, HttpsInspection, HttpsSetupError};
pub(crate) use tls::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
pub use trust::{CaTrustError, CaTrustStatus, CaTrustStore};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";
