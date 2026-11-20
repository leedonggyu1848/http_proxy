//! Registration of the public inspection CA in client trust stores.

use std::path::Path;

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use thiserror::Error;

use crate::CaInfo;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
use linux::PlatformStore;
#[cfg(target_os = "macos")]
use macos::PlatformStore;

/// Registration state of this installation's CA in the selected client store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaTrustStatus {
    /// No entry managed by HTTP Inspector exists for this CA.
    NotInstalled,
    /// The exact CA is present and trusted to issue HTTPS server certificates.
    Trusted,
    /// The exact CA is present, but is not trusted for HTTPS server certificates.
    Untrusted,
}

/// Failure to inspect or update a client trust store.
#[derive(Debug, Error)]
pub enum CaTrustError {
    #[error("CA registration is not supported on this platform")]
    UnsupportedPlatform,
    #[error("HOME must name an absolute directory to locate the browser trust store")]
    MissingHome,
    #[error("{0}")]
    Store(String),
    #[error("failed to access the trust store: {0}")]
    Io(#[from] std::io::Error),
    #[error("public CA certificate is invalid or has changed; refresh the CA information")]
    CertificateChanged,
}

/// Platform-specific client trust registration, separate from TLS interception.
///
/// Linux supports the current user's Chrome/Chromium NSS shared DB and
/// explicitly selected Firefox profiles. macOS uses the login keychain for
/// Chrome/Safari and NSS for Firefox profiles. Neither changes the system store.
#[derive(Debug)]
pub struct CaTrustStore {
    platform: PlatformStore,
}

impl CaTrustStore {
    /// Human-readable registration target for this platform.
    pub const DESCRIPTION: &'static str = PlatformStore::DESCRIPTION;

    /// Selects the supported client trust store for the current user.
    pub fn for_current_user() -> Result<Self, CaTrustError> {
        Ok(Self {
            platform: PlatformStore::for_current_user()?,
        })
    }

    /// Selects a dedicated Firefox profile's NSS database.
    /// The caller must keep Firefox closed while changing this database.
    pub fn for_firefox_profile(profile: &Path) -> Result<Self, CaTrustError> {
        Ok(Self {
            platform: PlatformStore::for_firefox_profile(profile)?,
        })
    }

    /// Location of the selected trust store.
    #[must_use]
    pub fn location(&self) -> &Path {
        self.platform.location()
    }

    /// Checks the certificate bytes and HTTPS trust of the managed entry.
    pub async fn status(&self, ca: &CaInfo) -> Result<CaTrustStatus, CaTrustError> {
        self.platform.status(&public_certificate(ca)?).await
    }

    /// Registers only the public CA, then verifies the resulting trust state.
    /// Repeated registration converges to the same trusted entry.
    pub async fn install(&self, ca: &CaInfo) -> Result<CaTrustStatus, CaTrustError> {
        self.platform.install(&public_certificate(ca)?).await
    }

    /// Removes only the managed entry after matching its certificate bytes.
    pub async fn remove(&self, ca: &CaInfo) -> Result<CaTrustStatus, CaTrustError> {
        self.platform.remove(&public_certificate(ca)?).await
    }
}

fn public_certificate(ca: &CaInfo) -> Result<CertificateDer<'static>, CaTrustError> {
    let pem = std::fs::read(ca.certificate_path())?;
    // Never forward a file containing a key to an external registration tool.
    let certificate_count = pem
        .windows(b"-----BEGIN CERTIFICATE-----".len())
        .filter(|window| *window == b"-----BEGIN CERTIFICATE-----")
        .count();
    if certificate_count != 1 || pem.windows(11).any(|window| window == b"PRIVATE KEY") {
        return Err(CaTrustError::CertificateChanged);
    }
    let der = CertificateDer::from_pem_slice(&pem).map_err(|_| CaTrustError::CertificateChanged)?;
    if crate::tls::fingerprint(&der) != ca.fingerprint_sha256() {
        return Err(CaTrustError::CertificateChanged);
    }
    Ok(der)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use unsupported::PlatformStore;
