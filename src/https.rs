//! HTTPS interception trust management: local CA lifecycle, leaf certificate
//! issuance, and client trust store registration.

mod ca;
mod issuer;
mod trust;

pub use ca::{CaInfo, HttpsSetupError};
pub use issuer::HttpsInspection;
pub(crate) use issuer::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
pub use trust::{CaTrustError, CaTrustStatus, CaTrustStore};
