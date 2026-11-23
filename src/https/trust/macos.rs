use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, pem::PemObject};
use tokio::process::Command;

use super::{CaTrustError, CaTrustStatus, linux};
use crate::settings::paths;

#[derive(Debug)]
pub(super) enum PlatformStore {
    Login(PathBuf),
    Firefox(linux::PlatformStore),
}

impl PlatformStore {
    pub(super) const DESCRIPTION: &'static str = "Chrome/Safari (login keychain)";

    pub(super) fn for_current_user() -> Result<Self, CaTrustError> {
        let home = paths::home_directory(|name| std::env::var_os(name))
            .ok_or(CaTrustError::MissingHome)?;
        Ok(Self::Login(
            home.join("Library/Keychains/login.keychain-db"),
        ))
    }

    pub(super) fn for_firefox_profile(profile: &Path) -> Result<Self, CaTrustError> {
        Ok(Self::Firefox(linux::PlatformStore::for_firefox_profile(
            profile,
        )?))
    }

    pub(super) fn location(&self) -> &Path {
        match self {
            Self::Login(path) => path,
            Self::Firefox(store) => store.location(),
        }
    }

    pub(super) async fn status(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        if let Self::Firefox(store) = self {
            return store.status(certificate).await;
        }
        let output = security(&[
            Path::new("find-certificate"),
            Path::new("-a"),
            Path::new("-p"),
            self.location(),
        ])
        .await?;
        require_success(&output)?;
        let mut present = false;
        for stored in CertificateDer::pem_slice_iter(&output.stdout) {
            let stored = stored.map_err(|error| CaTrustError::Store(error.to_string()))?;
            present |= stored.as_ref() == certificate.as_ref();
        }
        if !present {
            return Ok(CaTrustStatus::NotInstalled);
        }
        let file = public_file(certificate)?;
        let output = security(&[
            Path::new("verify-cert"),
            Path::new("-c"),
            file.path(),
            Path::new("-p"),
            Path::new("ssl"),
            Path::new("-l"),
            Path::new("-L"),
            Path::new("-k"),
            self.location(),
        ])
        .await?;
        Ok(if output.status.success() {
            CaTrustStatus::Trusted
        } else {
            CaTrustStatus::Untrusted
        })
    }

    pub(super) async fn install(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        if let Self::Firefox(store) = self {
            return store.install(certificate).await;
        }
        if self.status(certificate).await? == CaTrustStatus::Trusted {
            return Ok(CaTrustStatus::Trusted);
        }
        let file = public_file(certificate)?;
        let output = security(&[
            Path::new("add-trusted-cert"),
            Path::new("-r"),
            Path::new("trustRoot"),
            Path::new("-p"),
            Path::new("ssl"),
            Path::new("-k"),
            self.location(),
            file.path(),
        ])
        .await?;
        require_success(&output)?;
        self.status(certificate).await
    }

    pub(super) async fn remove(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        if let Self::Firefox(store) = self {
            return store.remove(certificate).await;
        }
        if self.status(certificate).await? == CaTrustStatus::NotInstalled {
            return Ok(CaTrustStatus::NotInstalled);
        }
        let fingerprint = crate::https::ca::fingerprint(certificate).replace(':', "");
        let output = security(&[
            Path::new("delete-certificate"),
            Path::new("-t"),
            Path::new("-Z"),
            Path::new(&fingerprint),
            self.location(),
        ])
        .await?;
        require_success(&output)?;
        self.status(certificate).await
    }
}

fn public_file(certificate: &CertificateDer<'_>) -> Result<tempfile::NamedTempFile, CaTrustError> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(certificate.as_ref())?;
    file.flush()?;
    Ok(file)
}

async fn security(args: &[&Path]) -> Result<Output, CaTrustError> {
    tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("/usr/bin/security")
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        CaTrustError::Store(
            "Keychain authorization timed out; approve the macOS dialog and retry".to_owned(),
        )
    })?
    .map_err(CaTrustError::Io)
}

fn require_success(output: &Output) -> Result<(), CaTrustError> {
    if output.status.success() {
        return Ok(());
    }
    Err(CaTrustError::Store(format!(
        "Keychain operation failed or authorization was cancelled ({}): {}. Approve the macOS authorization dialog to register or remove the CA",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::https::ca::CaInfo;

    #[tokio::test]
    async fn new_ca_is_absent_from_login_keychain() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&directory.path().join("ca"))?;
        let certificate = super::super::public_certificate(&ca)?;
        let store = PlatformStore::for_current_user()?;
        assert_eq!(
            store.status(&certificate).await?,
            CaTrustStatus::NotInstalled
        );
        let file = public_file(&certificate)?;
        assert_eq!(std::fs::read(file.path())?, certificate.as_ref());
        Ok(())
    }
}
