//! Per-host leaf certificate issuance and TLS endpoints for HTTPS inspection.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use http::uri::Authority;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::ca::{CaInfo, CertificateAuthority, HttpsSetupError};

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

pub(crate) struct TlsInspector {
    issuer: Issuer<'static, KeyPair>,
    connector: TlsConnector,
}

pub(crate) struct Destination {
    host: String,
    port: u16,
    server_name: ServerName<'static>,
}

pub(crate) struct PreparedInspection {
    acceptor: TlsAcceptor,
}

#[derive(Debug, Error)]
pub(crate) enum DestinationError {
    #[error("CONNECT authority has no host")]
    MissingHost,
    #[error("CONNECT authority has an invalid TLS server name: {0}")]
    InvalidServerName(String),
}

#[derive(Debug, Error)]
pub(crate) enum PrepareError {
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
pub(crate) enum UpstreamTlsError {
    #[error("upstream TLS verification or handshake failed: {0}")]
    Handshake(std::io::Error),
}

impl TlsInspector {
    pub(crate) fn open(settings: &HttpsInspection) -> Result<(Arc<Self>, CaInfo), HttpsSetupError> {
        let (issuer, info) =
            CertificateAuthority::load_or_create(&settings.ca_directory)?.into_parts();
        let connector = build_upstream_connector(settings.additional_upstream_root.as_deref())?;
        Ok((Arc::new(Self { issuer, connector }), info))
    }

    pub(crate) fn prepare(
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

    pub(crate) async fn handshake(
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
    pub(crate) fn from_authority(authority: &Authority) -> Result<Self, DestinationError> {
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

    pub(crate) fn socket_target(&self) -> String {
        if self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub(crate) fn absolute_target(&self, path_and_query: &str) -> String {
        let authority = if self.port == 443 {
            self.host.clone()
        } else if self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        };
        format!("https://{authority}{path_and_query}")
    }

    pub(crate) fn matches_authority(&self, authority: &Authority) -> bool {
        authority.host().eq_ignore_ascii_case(&self.host)
            && authority.port_u16().unwrap_or(443) == self.port
    }
}

impl PreparedInspection {
    pub(crate) async fn accept<IO>(&self, io: IO) -> Result<ServerTlsStream<IO>, std::io::Error>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
    {
        self.acceptor.accept(io).await
    }
}

pub(crate) fn build_upstream_connector(
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
    use std::error::Error;

    use super::*;

    #[test]
    fn destination_parses_and_formats_dns_and_ip_authorities() -> Result<(), Box<dyn Error>> {
        let dns = Destination::from_authority(&Authority::from_static("Example.COM"))?;
        assert_eq!(dns.socket_target(), "example.com:443");
        assert_eq!(
            dns.absolute_target("/path?a=1"),
            "https://example.com/path?a=1"
        );
        assert!(dns.matches_authority(&Authority::from_static("EXAMPLE.com:443")));
        assert!(!dns.matches_authority(&Authority::from_static("example.com:8443")));

        let custom_port = Destination::from_authority(&Authority::from_static("127.0.0.1:8443"))?;
        assert_eq!(custom_port.socket_target(), "127.0.0.1:8443");
        assert_eq!(custom_port.absolute_target("/"), "https://127.0.0.1:8443/");
        assert!(custom_port.matches_authority(&Authority::from_static("127.0.0.1:8443")));
        assert!(!custom_port.matches_authority(&Authority::from_static("127.0.0.1")));
        Ok(())
    }

    #[test]
    fn prepares_leaf_certificate_for_dns_and_ip_destinations() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let ca_dir = root.path().join("ca");
        let ca = CaInfo::load_or_create(&ca_dir)?;
        let settings = HttpsInspection::new(ca_dir)
            .with_additional_upstream_root(ca.certificate_path().to_owned());
        let (inspector, info) = TlsInspector::open(&settings)?;
        assert_eq!(info, ca);

        for authority in ["example.com:443", "127.0.0.1:8443"] {
            let destination = Destination::from_authority(&authority.parse()?)?;
            let _prepared = inspector.prepare(&destination)?;
        }
        Ok(())
    }
}
