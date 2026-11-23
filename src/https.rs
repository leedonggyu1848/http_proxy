//! HTTPS interception trust management: local CA lifecycle, leaf certificate
//! issuance, and client trust store registration.

pub(crate) mod ca;
pub(crate) mod trust;

pub use ca::{CaInfo, HttpsInspection, HttpsSetupError};
pub(crate) use ca::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
pub use trust::{CaTrustError, CaTrustStatus, CaTrustStore};
