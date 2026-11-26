//! Local inspection CA creation, persistence, validation, and metadata.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fs2::FileExt;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use thiserror::Error;

const IDENTITY_FILE: &str = "ca-identity.pem";
const CERTIFICATE_FILE: &str = "root-ca.pem";
const LOCK_FILE: &str = ".ca.lock";

/// Public metadata for the local inspection CA.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaInfo {
    certificate_path: PathBuf,
    fingerprint_sha256: String,
    valid_from: SystemTime,
    valid_until: SystemTime,
}

impl CaInfo {
    /// Prepares the persistent CA without starting a proxy.
    ///
    /// Production callers resolve `directory` through
    /// [`crate::settings::paths::ca_directory`].
    pub fn load_or_create(directory: &Path) -> Result<Self, HttpsSetupError> {
        CertificateAuthority::load_or_create(directory).map(|ca| ca.info)
    }

    /// Returns the certificate-only PEM path that clients can trust.
    #[must_use]
    pub fn certificate_path(&self) -> &Path {
        &self.certificate_path
    }

    /// Returns the colon-separated SHA-256 certificate fingerprint.
    #[must_use]
    pub fn fingerprint_sha256(&self) -> &str {
        &self.fingerprint_sha256
    }

    /// Returns the start of the certificate's validity period.
    #[must_use]
    pub const fn valid_from(&self) -> SystemTime {
        self.valid_from
    }

    /// Returns the end of the certificate's validity period.
    #[must_use]
    pub const fn valid_until(&self) -> SystemTime {
        self.valid_until
    }
}

/// Error returned while HTTPS inspection state is prepared.
#[derive(Debug, Error)]
pub enum HttpsSetupError {
    #[error("failed to {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("CA directory is not a private regular directory: {0}")]
    UnsafeDirectory(PathBuf),
    #[error("CA identity is not a private regular file: {0}")]
    UnsafeIdentity(PathBuf),
    #[error("CA identity is incomplete; both files must exist: {0}")]
    IncompleteIdentity(PathBuf),
    #[error("CA identity is invalid: {0}")]
    InvalidIdentity(String),
    #[error("no trusted upstream root certificates were loaded")]
    NoUpstreamRoots,
    #[error("additional upstream root is invalid: {0}")]
    InvalidAdditionalRoot(PathBuf),
}

pub(super) struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    info: CaInfo,
}

impl CertificateAuthority {
    pub(super) fn load_or_create(directory: &Path) -> Result<Self, HttpsSetupError> {
        prepare_directory(directory)?;
        let lock_path = directory.join(LOCK_FILE);
        let lock = open_private_file(&lock_path, false)?;
        FileExt::lock_exclusive(&lock).map_err(|source| HttpsSetupError::Io {
            operation: "lock",
            path: lock_path,
            source,
        })?;

        let identity_path = directory.join(IDENTITY_FILE);
        let certificate_path = directory.join(CERTIFICATE_FILE);
        let identity_exists = identity_path.exists();
        let certificate_exists = certificate_path.exists();
        match (identity_exists, certificate_exists) {
            (false, false) => create_identity(&identity_path, &certificate_path)?,
            (true, true) => {}
            _ => return Err(HttpsSetupError::IncompleteIdentity(directory.to_owned())),
        }

        validate_private_file(&identity_path)?;
        let bundle = read_file(&identity_path)?;
        let public = read_file(&certificate_path)?;
        let (issuer, certificate) = parse_identity(&bundle, &public)?;
        let (valid_from, valid_until) = validity(&certificate)?;
        let info = CaInfo {
            certificate_path,
            fingerprint_sha256: fingerprint(&certificate),
            valid_from,
            valid_until,
        };
        Ok(Self { issuer, info })
    }

    pub(super) fn into_parts(self) -> (Issuer<'static, KeyPair>, CaInfo) {
        (self.issuer, self.info)
    }
}

fn prepare_directory(path: &Path) -> Result<(), HttpsSetupError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|source| HttpsSetupError::Io {
            operation: "inspect",
            path: path.to_owned(),
            source,
        })?;
        if !metadata.file_type().is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(HttpsSetupError::UnsafeDirectory(path.to_owned()));
        }
        return Ok(());
    }

    fs::create_dir_all(path).map_err(|source| HttpsSetupError::Io {
        operation: "create directory",
        path: path.to_owned(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        HttpsSetupError::Io {
            operation: "set directory permissions on",
            path: path.to_owned(),
            source,
        }
    })
}

fn open_private_file(path: &Path, truncate: bool) -> Result<File, HttpsSetupError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).mode(0o600);
    if truncate {
        options.truncate(true);
    }
    options.open(path).map_err(|source| HttpsSetupError::Io {
        operation: "open",
        path: path.to_owned(),
        source,
    })
}

fn validate_private_file(path: &Path) -> Result<(), HttpsSetupError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| HttpsSetupError::Io {
        operation: "inspect",
        path: path.to_owned(),
        source,
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(HttpsSetupError::UnsafeIdentity(path.to_owned()));
    }
    Ok(())
}

fn create_identity(identity_path: &Path, certificate_path: &Path) -> Result<(), HttpsSetupError> {
    let key =
        KeyPair::generate().map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let mut params = CertificateParams::default();
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(1_825);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.use_authority_key_identifier_extension = true;
    let mut name = DistinguishedName::new();
    // NSS groups certificates by subject; give each installation its own subject.
    let identifier = format!("{:X}", Sha256::digest(key.public_key_raw()));
    name.push(
        DnType::CommonName,
        format!("HTTP Inspector Local CA {identifier:.16}"),
    );
    params.distinguished_name = name;
    let certificate = params
        .self_signed(&key)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let certificate_pem = certificate.pem();
    let bundle = format!("{certificate_pem}{}", key.serialize_pem());
    atomic_write(identity_path, bundle.as_bytes(), 0o600)?;
    atomic_write(certificate_path, certificate_pem.as_bytes(), 0o644)
}

fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<(), HttpsSetupError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| HttpsSetupError::InvalidIdentity("invalid CA file name".to_owned()))?;
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(|source| HttpsSetupError::Io {
            operation: "create temporary file",
            path: temporary.clone(),
            source,
        })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| HttpsSetupError::Io {
            operation: "write",
            path: temporary.clone(),
            source,
        })?;
    fs::rename(&temporary, path).map_err(|source| HttpsSetupError::Io {
        operation: "rename",
        path: temporary,
        source,
    })
}

fn read_file(path: &Path) -> Result<Vec<u8>, HttpsSetupError> {
    fs::read(path).map_err(|source| HttpsSetupError::Io {
        operation: "read",
        path: path.to_owned(),
        source,
    })
}

/// Reads the validity period of a certificate.
fn validity(certificate: &CertificateDer<'_>) -> Result<(SystemTime, SystemTime), HttpsSetupError> {
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let at = |time: x509_parser::time::ASN1Time| {
        let seconds = time.timestamp();
        u64::try_from(seconds).map_or(SystemTime::UNIX_EPOCH, |seconds| {
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
        })
    };
    let validity = parsed.validity();
    Ok((at(validity.not_before), at(validity.not_after)))
}

fn parse_identity(
    bundle: &[u8],
    public: &[u8],
) -> Result<(Issuer<'static, KeyPair>, CertificateDer<'static>), HttpsSetupError> {
    let certificate = CertificateDer::from_pem_slice(bundle)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let private_key = PrivateKeyDer::from_pem_slice(bundle)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let public_certificate = CertificateDer::from_pem_slice(public)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    if certificate != public_certificate {
        return Err(HttpsSetupError::InvalidIdentity(
            "public certificate does not match the CA identity".to_owned(),
        ));
    }

    let key = KeyPair::try_from(&private_key)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let validation_key = private_key.clone_key();
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], validation_key)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    let issuer = Issuer::from_ca_cert_der(&certificate, key)
        .map_err(|error| HttpsSetupError::InvalidIdentity(error.to_string()))?;
    Ok((issuer, certificate))
}

pub(super) fn fingerprint(certificate: &CertificateDer<'_>) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;

    use super::*;
    use crate::settings::paths;

    #[test]
    fn ca_identity_survives_restart_with_private_permissions() -> Result<(), HttpsSetupError> {
        let directory = tempfile::tempdir().map_err(|source| HttpsSetupError::Io {
            operation: "create test directory",
            path: PathBuf::from("temporary directory"),
            source,
        })?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).map_err(
            |source| HttpsSetupError::Io {
                operation: "set test directory permissions on",
                path: directory.path().to_owned(),
                source,
            },
        )?;

        let first = CertificateAuthority::load_or_create(directory.path())?;
        let second = CertificateAuthority::load_or_create(directory.path())?;
        assert_eq!(
            first.info.fingerprint_sha256,
            second.info.fingerprint_sha256
        );
        let metadata = fs::metadata(directory.path().join(IDENTITY_FILE)).map_err(|source| {
            HttpsSetupError::Io {
                operation: "inspect",
                path: directory.path().join(IDENTITY_FILE),
                source,
            }
        })?;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn loads_existing_ca_from_settings_ca_directory_unchanged() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().to_path_buf();
        let data_home = root.path().join("xdg-data");
        let ca_dir = paths::ca_directory(|name| match name {
            "HOME" => Some(OsString::from(&home)),
            "XDG_DATA_HOME" => Some(OsString::from(&data_home)),
            _ => None,
        })
        .ok_or("CA directory must resolve from absolute HOME/XDG_DATA_HOME")?;

        let first = CaInfo::load_or_create(&ca_dir)?;
        let public_pem_before = fs::read(first.certificate_path())?;
        let second = CaInfo::load_or_create(&ca_dir)?;
        let public_pem_after = fs::read(second.certificate_path())?;

        assert_eq!(first, second);
        assert_eq!(first.certificate_path(), ca_dir.join(CERTIFICATE_FILE));
        let now = std::time::SystemTime::now();
        assert!(first.valid_from() < now && now < first.valid_until());
        let lifetime = first
            .valid_until()
            .duration_since(first.valid_from())
            .map_err(|_| "validity must end after it starts")?;
        // Created one day back and valid for 1,825 days ahead.
        assert_eq!(lifetime.as_secs() / 86_400, 1_826);
        assert_eq!(public_pem_before, public_pem_after);
        assert!(
            public_pem_after
                .windows(b"-----BEGIN CERTIFICATE-----".len())
                .any(|window| window == b"-----BEGIN CERTIFICATE-----")
        );
        assert!(
            !public_pem_after
                .windows(b"PRIVATE KEY".len())
                .any(|window| window == b"PRIVATE KEY")
        );
        assert_eq!(fs::metadata(&ca_dir)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(ca_dir.join(IDENTITY_FILE))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(ca_dir.join(CERTIFICATE_FILE))?
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        Ok(())
    }

    #[test]
    fn rejects_incomplete_ca_identity_without_overwriting() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let ca_dir = root.path().join("ca");
        let first = CaInfo::load_or_create(&ca_dir)?;
        fs::remove_file(first.certificate_path())?;

        assert!(matches!(
            CaInfo::load_or_create(&ca_dir),
            Err(HttpsSetupError::IncompleteIdentity(_))
        ));
        assert!(!first.certificate_path().exists());
        Ok(())
    }
}
