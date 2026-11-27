//! A PEM bundle of the system roots followed by the local CA certificate.
//!
//! Programs that read `SSL_CERT_FILE` and similar variables replace their
//! whole trust store with the named file. A bundle that keeps the system
//! roots and adds the local CA lets them verify both HTTPS decrypted by the
//! proxy and HTTPS that bypasses it.
//!
//! Only the public certificate of [`CaInfo`] is read. The CA identity file,
//! which holds the private key, is never opened, named, or copied here.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::https::CaInfo;

/// File name of the bundle inside its private directory.
pub const BUNDLE_FILE: &str = "combined-ca.pem";

/// System CA bundle files, most specific first: Debian, Ubuntu, Arch, and
/// Gentoo; Fedora and RHEL; openSUSE; RHEL's extracted trust; Alpine,
/// macOS, and the BSDs.
const SYSTEM_BUNDLES: [&str; 6] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/openssl/cert.pem",
];

/// Largest system bundle read; real bundles are well under 1 MiB.
const MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;
const PEM_CERTIFICATE: &str = "-----BEGIN CERTIFICATE-----";
const PEM_PRIVATE_KEY: &str = "PRIVATE KEY-----";

/// Where the system part of a bundle came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemRoots {
    /// The PEM file at this path, copied as is.
    File(PathBuf),
    /// No readable system bundle was found; the bundle holds only the local
    /// CA, so HTTPS that bypasses the proxy fails certificate checks.
    Missing,
}

/// Reason a CA bundle could not be written.
#[derive(Debug, thiserror::Error)]
pub enum CaBundleError {
    #[error("{} is not a PEM certificate", .0.display())]
    InvalidCertificate(PathBuf),
    #[error("Could not write the CA bundle: {0}")]
    Io(#[from] io::Error),
}

/// A written bundle. A temporary bundle removes its private directory on
/// drop; a shared bundle stays until [`remove_shared`] removes it.
#[derive(Debug)]
pub struct CaBundle {
    path: PathBuf,
    system_roots: SystemRoots,
    _directory: Option<TempDir>,
}

impl CaBundle {
    /// Writes a bundle in a new mode-0700 directory under `parent`, as a
    /// mode-0600 file. Dropping the bundle removes the directory.
    pub fn create_temporary(parent: &Path, ca: &CaInfo) -> Result<Self, CaBundleError> {
        let contents = Contents::collect(ca, |name| std::env::var_os(name))?;
        let directory = tempfile::Builder::new()
            .prefix("http-inspector-shell-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let path = directory.path().join(BUNDLE_FILE);
        write_private(&path, &contents.pem)?;
        Ok(Self {
            path,
            system_roots: contents.system_roots,
            _directory: Some(directory),
        })
    }

    /// Writes or replaces [`BUNDLE_FILE`] in `directory`, which the caller
    /// keeps private to the current user. The file has mode 0600 and is
    /// replaced atomically, so readers never see half a bundle.
    pub fn write_shared(directory: &Path, ca: &CaInfo) -> Result<Self, CaBundleError> {
        let contents = Contents::collect(ca, |name| std::env::var_os(name))?;
        let path = directory.join(BUNDLE_FILE);
        let pending = tempfile::Builder::new()
            .prefix(".combined-ca-")
            .permissions(fs::Permissions::from_mode(0o600))
            .tempfile_in(directory)?;
        pending.as_file().write_all(contents.pem.as_bytes())?;
        pending.as_file().sync_all()?;
        pending.persist(&path).map_err(|error| error.error)?;
        Ok(Self {
            path,
            system_roots: contents.system_roots,
            _directory: None,
        })
    }

    /// The bundle file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the system roots in the bundle came from.
    #[must_use]
    pub const fn system_roots(&self) -> &SystemRoots {
        &self.system_roots
    }
}

/// Removes a bundle written by [`CaBundle::write_shared`], if any.
pub fn remove_shared(directory: &Path) {
    let _already_gone = fs::remove_file(directory.join(BUNDLE_FILE)).is_err();
}

/// The bundle text and where its system part came from.
struct Contents {
    pem: String,
    system_roots: SystemRoots,
}

impl Contents {
    fn collect(
        ca: &CaInfo,
        lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, CaBundleError> {
        let local = read_certificate(ca.certificate_path())?;
        let (system_roots, system) = system_bundle(lookup);
        let mut pem = system;
        if !pem.is_empty() && !pem.ends_with('\n') {
            pem.push('\n');
        }
        pem.push_str("# HTTP Inspector local CA\n");
        pem.push_str(&local);
        if !pem.ends_with('\n') {
            pem.push('\n');
        }
        Ok(Self { pem, system_roots })
    }
}

/// Reads the local CA certificate and refuses anything holding a key.
fn read_certificate(path: &Path) -> Result<String, CaBundleError> {
    let text = read_limited(path)?;
    if !text.contains(PEM_CERTIFICATE) || text.contains(PEM_PRIVATE_KEY) {
        return Err(CaBundleError::InvalidCertificate(path.to_owned()));
    }
    Ok(text)
}

/// The first usable system bundle: an inherited absolute `SSL_CERT_FILE`,
/// then the distribution paths.
fn system_bundle(mut lookup: impl FnMut(&str) -> Option<OsString>) -> (SystemRoots, String) {
    let inherited = lookup("SSL_CERT_FILE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    inherited
        .into_iter()
        .chain(SYSTEM_BUNDLES.iter().map(PathBuf::from))
        .find_map(|path| {
            let text = read_limited(&path).ok()?;
            (text.contains(PEM_CERTIFICATE) && !text.contains(PEM_PRIVATE_KEY))
                .then_some((SystemRoots::File(path), text))
        })
        .unwrap_or((SystemRoots::Missing, String::new()))
}

fn read_limited(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(MAX_BUNDLE_BYTES)
        .read_to_string(&mut text)?;
    Ok(text)
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn mode(path: &Path) -> io::Result<u32> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }

    #[test]
    fn temporary_bundle_is_private_holds_only_public_certificates_and_is_removed() -> TestResult {
        let root = tempfile::tempdir()?;
        let ca_directory = root.path().join("ca");
        let ca = CaInfo::load_or_create(&ca_directory)?;
        let bundle = CaBundle::create_temporary(root.path(), &ca)?;
        let path = bundle.path().to_owned();
        let directory = path.parent().ok_or("no parent")?.to_owned();
        assert_eq!(mode(&directory)?, 0o700);
        assert_eq!(mode(&path)?, 0o600);
        let text = fs::read_to_string(&path)?;
        assert!(text.contains(&fs::read_to_string(ca.certificate_path())?));
        assert!(!text.contains(PEM_PRIVATE_KEY));
        assert!(!text.contains("ca-identity"));
        // The local CA is the last certificate, after the system roots.
        assert!(
            text.trim_end()
                .ends_with(fs::read_to_string(ca.certificate_path())?.trim_end())
        );
        drop(bundle);
        assert!(!directory.exists());
        assert!(ca.certificate_path().exists());
        Ok(())
    }

    #[test]
    fn system_roots_come_first_and_missing_roots_leave_only_the_local_ca() -> TestResult {
        let root = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&root.path().join("ca"))?;
        let system = root.path().join("system.pem");
        fs::write(
            &system,
            "-----BEGIN CERTIFICATE-----\nU1lTVEVN\n-----END CERTIFICATE-----",
        )?;
        let inherited = system.clone().into_os_string();
        let contents = Contents::collect(&ca, |name| {
            (name == "SSL_CERT_FILE").then(|| inherited.clone())
        })?;
        assert_eq!(contents.system_roots, SystemRoots::File(system));
        assert!(
            contents
                .pem
                .starts_with("-----BEGIN CERTIFICATE-----\nU1lTVEVN")
        );
        assert!(
            contents
                .pem
                .contains("-----END CERTIFICATE-----\n# HTTP Inspector local CA\n")
        );
        let (roots, _) = system_bundle(|_| Some(OsString::from("relative.pem")));
        assert!(matches!(roots, SystemRoots::File(_) | SystemRoots::Missing));
        let key = root.path().join("key.pem");
        fs::write(
            &key,
            "-----BEGIN CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\n",
        )?;
        assert!(matches!(
            read_certificate(&key),
            Err(CaBundleError::InvalidCertificate(_))
        ));
        Ok(())
    }

    #[test]
    fn shared_bundle_is_replaced_in_place_and_removed_on_request() -> TestResult {
        let root = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&root.path().join("ca"))?;
        let shared = root.path().join("control");
        fs::create_dir(&shared)?;
        let first = CaBundle::write_shared(&shared, &ca)?;
        let second = CaBundle::write_shared(&shared, &ca)?;
        assert_eq!(first.path(), second.path());
        assert_eq!(mode(second.path())?, 0o600);
        assert_eq!(fs::read_dir(&shared)?.count(), 1);
        drop(second);
        assert!(first.path().exists());
        remove_shared(&shared);
        assert!(!first.path().exists());
        Ok(())
    }
}
