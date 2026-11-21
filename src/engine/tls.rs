use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use http::uri::Authority;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const IDENTITY_FILE: &str = "ca-identity.pem";
const CERTIFICATE_FILE: &str = "root-ca.pem";
const LOCK_FILE: &str = ".ca.lock";
const ALPN_HTTP_1_1: &[u8] = b"http/1.1";

/// Configuration for opt-in HTTPS inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpsInspection {
    ca_directory: PathBuf,
    additional_upstream_root: Option<PathBuf>,
}

impl HttpsInspection {
    /// Creates HTTPS inspection settings with a persistent CA directory.
    #[must_use]
    pub const fn new(ca_directory: PathBuf) -> Self {
        Self {
            ca_directory,
            additional_upstream_root: None,
        }
    }

    /// Adds a PEM root certificate for private upstream servers.
    #[must_use]
    pub fn with_additional_upstream_root(mut self, path: PathBuf) -> Self {
        self.additional_upstream_root = Some(path);
        self
    }
}

/// Public metadata for the local inspection CA.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaInfo {
    certificate_path: PathBuf,
    fingerprint_sha256: String,
}

impl CaInfo {
    /// Prepares the persistent CA without starting a proxy.
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

pub(in crate::engine) struct TlsInspector {
    issuer: Issuer<'static, KeyPair>,
    connector: TlsConnector,
}

pub(in crate::engine) struct Destination {
    host: String,
    port: u16,
    server_name: ServerName<'static>,
}

pub(in crate::engine) struct PreparedInspection {
    acceptor: TlsAcceptor,
}

#[derive(Debug, Error)]
pub(in crate::engine) enum DestinationError {
    #[error("CONNECT authority has no host")]
    MissingHost,
    #[error("CONNECT authority has an invalid TLS server name: {0}")]
    InvalidServerName(String),
}

#[derive(Debug, Error)]
pub(in crate::engine) enum PrepareError {
    #[error("failed to create a leaf key: {0}")]
    Key(rcgen::Error),
    #[error("failed to create leaf parameters: {0}")]
    Parameters(rcgen::Error),
    #[error("failed to sign the leaf certificate: {0}")]
    Certificate(rcgen::Error),
    #[error("failed to configure the downstream TLS server: {0}")]
    ServerConfig(rustls::Error),
}

#[derive(Debug, Error)]
pub(in crate::engine) enum UpstreamTlsError {
    #[error("upstream TLS verification or handshake failed: {0}")]
    Handshake(std::io::Error),
}

impl TlsInspector {
    pub(in crate::engine) fn open(
        settings: &HttpsInspection,
    ) -> Result<(Arc<Self>, CaInfo), HttpsSetupError> {
        let authority = CertificateAuthority::load_or_create(&settings.ca_directory)?;
        let connector = build_upstream_connector(settings.additional_upstream_root.as_deref())?;
        let info = authority.info.clone();
        Ok((
            Arc::new(Self {
                issuer: authority.issuer,
                connector,
            }),
            info,
        ))
    }

    pub(in crate::engine) fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<PreparedInspection, PrepareError> {
        let key = KeyPair::generate().map_err(PrepareError::Key)?;
        let mut params = CertificateParams::new(vec![destination.host.clone()])
            .map_err(PrepareError::Parameters)?;
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::minutes(5);
        params.not_after = now + time::Duration::days(7);
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, destination.host.clone());
        params.distinguished_name = name;
        let certificate = params
            .signed_by(&key, &self.issuer)
            .map_err(PrepareError::Certificate)?;
        let private_key = PrivatePkcs8KeyDer::from(key.serialize_der()).into();
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.der().clone()], private_key)
            .map_err(PrepareError::ServerConfig)?;
        config.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
        Ok(PreparedInspection {
            acceptor: TlsAcceptor::from(Arc::new(config)),
        })
    }

    pub(in crate::engine) async fn handshake(
        &self,
        destination: &Destination,
        stream: TcpStream,
    ) -> Result<ClientTlsStream<TcpStream>, UpstreamTlsError> {
        self.connector
            .connect(destination.server_name.clone(), stream)
            .await
            .map_err(UpstreamTlsError::Handshake)
    }
}

impl Destination {
    pub(in crate::engine) fn from_authority(
        authority: &Authority,
    ) -> Result<Self, DestinationError> {
        let host = authority.host();
        if host.is_empty() {
            return Err(DestinationError::MissingHost);
        }
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|_| DestinationError::InvalidServerName(host.to_owned()))?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port: authority.port_u16().unwrap_or(443),
            server_name,
        })
    }

    pub(in crate::engine) fn socket_target(&self) -> String {
        if self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub(in crate::engine) fn absolute_target(&self, path_and_query: &str) -> String {
        let authority = if self.port == 443 {
            self.host.clone()
        } else if self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        };
        format!("https://{authority}{path_and_query}")
    }

    pub(in crate::engine) fn matches_authority(&self, authority: &Authority) -> bool {
        authority.host().eq_ignore_ascii_case(&self.host)
            && authority.port_u16().unwrap_or(443) == self.port
    }
}

impl PreparedInspection {
    pub(in crate::engine) async fn accept<IO>(
        &self,
        io: IO,
    ) -> Result<ServerTlsStream<IO>, std::io::Error>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
    {
        self.acceptor.accept(io).await
    }
}

struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    info: CaInfo,
}

impl CertificateAuthority {
    fn load_or_create(directory: &Path) -> Result<Self, HttpsSetupError> {
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
        let info = CaInfo {
            certificate_path,
            fingerprint_sha256: fingerprint(&certificate),
        };
        Ok(Self { issuer, info })
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

pub(in crate::engine) fn fingerprint(certificate: &CertificateDer<'_>) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn build_upstream_connector(
    additional_root: Option<&Path>,
) -> Result<TlsConnector, HttpsSetupError> {
    let native = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (valid, _) = roots.add_parsable_certificates(native.certs);
    if valid == 0 && additional_root.is_none() {
        return Err(HttpsSetupError::NoUpstreamRoots);
    }

    if let Some(path) = additional_root {
        let certificates = CertificateDer::pem_file_iter(path)
            .map_err(|_| HttpsSetupError::InvalidAdditionalRoot(path.to_owned()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| HttpsSetupError::InvalidAdditionalRoot(path.to_owned()))?;
        if certificates.is_empty() {
            return Err(HttpsSetupError::InvalidAdditionalRoot(path.to_owned()));
        }
        for certificate in certificates {
            roots
                .add(certificate)
                .map_err(|_| HttpsSetupError::InvalidAdditionalRoot(path.to_owned()))?;
        }
    }

    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
