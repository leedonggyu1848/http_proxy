use std::path::Path;

use rustls::pki_types::CertificateDer;

use super::{CaTrustError, CaTrustStatus};

#[derive(Debug)]
pub(super) enum PlatformStore {}

impl PlatformStore {
    pub(super) const DESCRIPTION: &'static str = "Unsupported trust store";

    pub(super) const fn for_current_user() -> Result<Self, CaTrustError> {
        Err(CaTrustError::UnsupportedPlatform)
    }

    pub(super) const fn for_firefox_profile(_profile: &Path) -> Result<Self, CaTrustError> {
        Err(CaTrustError::UnsupportedPlatform)
    }

    pub(super) fn location(&self) -> &Path {
        match *self {}
    }

    pub(super) async fn status(
        &self,
        _certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        match *self {}
    }

    pub(super) async fn install(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        self.status(certificate).await
    }

    pub(super) async fn remove(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        self.status(certificate).await
    }
}
