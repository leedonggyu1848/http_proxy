//! HTTPS interception trust management: local CA lifecycle, leaf certificate
//! issuance, and client trust store registration.

mod ca;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod issuer;
mod trust;

pub use ca::{CaInfo, HttpsSetupError};
pub use issuer::HttpsInspection;
pub(crate) use issuer::{Destination, PreparedInspection, TlsInspector, UpstreamTlsError};
pub use trust::{CaTrustError, CaTrustStatus, CaTrustStore};
