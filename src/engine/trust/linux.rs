use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use fs2::FileExt;
use rustls::pki_types::CertificateDer;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::{CaTrustError, CaTrustStatus};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "linux")]
const TOOL_HELP: &str = "Install NSS certutil: Ubuntu/Debian: sudo apt install libnss3-tools; Fedora: sudo dnf install nss-tools";

#[cfg(target_os = "macos")]
const TOOL_HELP: &str = "Install NSS certutil: brew install nss";

#[derive(Debug)]
pub(super) struct PlatformStore {
    database: PathBuf,
}

impl PlatformStore {
    #[cfg(target_os = "linux")]
    pub(super) const DESCRIPTION: &'static str = "Chrome/Chromium (current user)";

    #[cfg(target_os = "linux")]
    pub(super) fn for_current_user() -> Result<Self, CaTrustError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(CaTrustError::MissingHome)?;
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        Ok(Self::for_user(&home, data_home.as_deref()))
    }

    #[cfg(target_os = "linux")]
    fn for_user(home: &Path, data_home: Option<&Path>) -> Self {
        let legacy = home.join(".pki/nssdb");
        let database = if legacy.is_dir() {
            legacy
        } else {
            data_home
                .map_or_else(|| home.join(".local/share"), Path::to_owned)
                .join("pki/nssdb")
        };
        Self { database }
    }

    pub(super) fn for_firefox_profile(profile: &Path) -> Result<Self, CaTrustError> {
        if !profile.is_absolute() || !profile.is_dir() {
            return Err(CaTrustError::Store(
                "Firefox profile must be an existing absolute directory".to_owned(),
            ));
        }
        Ok(Self {
            database: profile.to_owned(),
        })
    }

    pub(super) fn location(&self) -> &Path {
        &self.database
    }

    pub(super) async fn status(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        if !self.has_database()? {
            return Ok(CaTrustStatus::NotInstalled);
        }
        let nickname = nickname(certificate);
        let entries = self.run(&["-L"], None).await?;
        let entries = String::from_utf8_lossy(&entries);
        let Some(trust) = entries.lines().find_map(|line| {
            line.strip_prefix(&nickname)
                .filter(|suffix| suffix.starts_with(char::is_whitespace))
                .and_then(|suffix| suffix.split_whitespace().next())
        }) else {
            return Ok(CaTrustStatus::NotInstalled);
        };
        let stored = self.run(&["-L", "-n", &nickname, "-r"], None).await?;
        if stored != certificate.as_ref() {
            return Err(CaTrustError::Store(
                "The managed CA entry contains a different certificate; refusing to change it"
                    .to_owned(),
            ));
        }
        Ok(
            if trust.split(',').next().is_some_and(|ssl| ssl.contains('C')) {
                CaTrustStatus::Trusted
            } else {
                CaTrustStatus::Untrusted
            },
        )
    }

    pub(super) async fn install(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        fs::create_dir_all(&self.database)?;
        let _lock = self.lock()?;
        if !self.has_database()? {
            self.run(&["-N", "--empty-password"], None).await?;
        }
        let nickname = nickname(certificate);
        match self.status(certificate).await? {
            CaTrustStatus::Trusted => return Ok(CaTrustStatus::Trusted),
            CaTrustStatus::Untrusted => {
                self.run(&["-M", "-n", &nickname, "-t", "C,,"], None)
                    .await?;
            }
            CaTrustStatus::NotInstalled => {
                self.run(
                    &["-A", "-n", &nickname, "-t", "C,,"],
                    Some(certificate.as_ref()),
                )
                .await?;
            }
        }
        let status = self.status(certificate).await?;
        if status != CaTrustStatus::Trusted {
            return Err(CaTrustError::Store(
                "The CA was imported but HTTPS trust could not be verified".to_owned(),
            ));
        }
        Ok(status)
    }

    pub(super) async fn remove(
        &self,
        certificate: &CertificateDer<'_>,
    ) -> Result<CaTrustStatus, CaTrustError> {
        if !self.has_database()? {
            return Ok(CaTrustStatus::NotInstalled);
        }
        let _lock = self.lock()?;
        if self.status(certificate).await? != CaTrustStatus::NotInstalled {
            self.run(&["-D", "-n", &nickname(certificate)], None)
                .await?;
        }
        let status = self.status(certificate).await?;
        if status != CaTrustStatus::NotInstalled {
            return Err(CaTrustError::Store(
                "The CA is still present in the browser trust store".to_owned(),
            ));
        }
        Ok(status)
    }

    fn has_database(&self) -> Result<bool, CaTrustError> {
        if self.database.join("cert9.db").try_exists()? {
            return Ok(true);
        }
        if self.database.join("key4.db").try_exists()?
            || self.database.join("cert8.db").try_exists()?
        {
            return Err(CaTrustError::Store(
                "The browser certificate database is incomplete or uses a legacy format; repair the browser's certificate database or import manually".to_owned(),
            ));
        }
        Ok(false)
    }

    fn lock(&self) -> Result<File, CaTrustError> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.database.join(".http-inspector.lock"))?;
        FileExt::try_lock_exclusive(&lock).map_err(|error| {
            CaTrustError::Store(format!("Another CA registration may be running: {error}"))
        })?;
        Ok(lock)
    }

    async fn run(&self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, CaTrustError> {
        let mut database = OsString::from("sql:");
        database.push(&self.database);
        let mut command = Command::new(certutil());
        command
            .args(args)
            .arg("-d")
            .arg(database)
            .env("LC_ALL", "C")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let operation = async {
            let mut child = command.spawn().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CaTrustError::Store(TOOL_HELP.to_owned())
                } else {
                    CaTrustError::Io(error)
                }
            })?;
            if let Some(input) = input {
                let mut stdin = child.stdin.take().ok_or_else(|| {
                    CaTrustError::Store("Could not send the public CA to certutil".to_owned())
                })?;
                stdin.write_all(input).await?;
            }
            let output = child.wait_with_output().await?;
            if !output.status.success() {
                return Err(CaTrustError::Store(format!(
                    "certutil failed ({}): {}. Close the browser and retry if the database is locked",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(output.stdout)
        };
        tokio::time::timeout(COMMAND_TIMEOUT, operation)
            .await
            .map_err(|_| {
                CaTrustError::Store(
                    "certutil timed out after 10 seconds; close the browser and retry".to_owned(),
                )
            })?
    }
}

fn nickname(certificate: &CertificateDer<'_>) -> String {
    format!(
        "HTTP Inspector {}",
        crate::engine::tls::fingerprint(certificate).replace(':', "")
    )
}

fn certutil() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    for prefix in ["/opt/homebrew/opt/nss/bin", "/usr/local/opt/nss/bin"] {
        let tool = Path::new(prefix).join("certutil");
        if tool.is_file() {
            return tool;
        }
    }
    "certutil".into()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::error::Error;

    use std::fs;

    use super::{CaTrustError, CaTrustStatus, PlatformStore, nickname};
    use crate::engine::tls::CaInfo;
    use crate::engine::trust::{CaTrustStore, public_certificate};

    #[test]
    fn selects_chromium_database_with_legacy_precedence() -> Result<(), Box<dyn Error>> {
        let home = tempfile::tempdir()?;
        assert_eq!(
            PlatformStore::for_user(home.path(), None).location(),
            home.path().join(".local/share/pki/nssdb")
        );
        let data_home = home.path().join("custom-data");
        assert_eq!(
            PlatformStore::for_user(home.path(), Some(&data_home)).location(),
            data_home.join("pki/nssdb")
        );
        fs::create_dir_all(home.path().join(".pki/nssdb"))?;
        assert_eq!(
            PlatformStore::for_user(home.path(), Some(&data_home)).location(),
            home.path().join(".pki/nssdb")
        );
        Ok(())
    }

    #[tokio::test]
    async fn registers_repairs_and_removes_only_this_installations_ca() -> Result<(), Box<dyn Error>>
    {
        let home = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&home.path().join("ca"))?;
        let other_ca = CaInfo::load_or_create(&home.path().join("other-ca"))?;
        let store = CaTrustStore {
            platform: PlatformStore::for_user(home.path(), None),
        };
        assert_eq!(store.status(&ca).await?, CaTrustStatus::NotInstalled);
        assert_eq!(store.remove(&ca).await?, CaTrustStatus::NotInstalled);
        assert!(!store.location().exists());
        assert_eq!(store.install(&other_ca).await?, CaTrustStatus::Trusted);
        assert_eq!(store.install(&ca).await?, CaTrustStatus::Trusted);
        assert_eq!(store.install(&ca).await?, CaTrustStatus::Trusted);

        let certificate = public_certificate(&ca)?;
        let nickname = nickname(&certificate);
        store
            .platform
            .run(&["-M", "-n", &nickname, "-t", ",,"], None)
            .await?;
        assert_eq!(store.status(&ca).await?, CaTrustStatus::Untrusted);
        assert_eq!(store.install(&ca).await?, CaTrustStatus::Trusted);
        store
            .platform
            .run(&["-V", "-n", &nickname, "-u", "L"], None)
            .await?;
        assert_eq!(store.remove(&ca).await?, CaTrustStatus::NotInstalled);
        assert_eq!(store.remove(&ca).await?, CaTrustStatus::NotInstalled);
        assert_eq!(store.status(&other_ca).await?, CaTrustStatus::Trusted);
        Ok(())
    }

    #[tokio::test]
    async fn refuses_to_replace_or_delete_a_conflicting_entry() -> Result<(), Box<dyn Error>> {
        let home = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&home.path().join("ca"))?;
        let other_ca = CaInfo::load_or_create(&home.path().join("other-ca"))?;
        let store = CaTrustStore {
            platform: PlatformStore::for_user(home.path(), None),
        };
        let certificate = public_certificate(&ca)?;
        let other_certificate = public_certificate(&other_ca)?;
        fs::create_dir_all(store.location())?;
        store
            .platform
            .run(&["-N", "--empty-password"], None)
            .await?;
        let nickname = nickname(&certificate);
        store
            .platform
            .run(
                &["-A", "-n", &nickname, "-t", "C,,"],
                Some(other_certificate.as_ref()),
            )
            .await?;
        assert!(store.status(&ca).await.is_err());
        assert!(store.install(&ca).await.is_err());
        assert!(store.remove(&ca).await.is_err());
        let stored = store
            .platform
            .run(&["-L", "-n", &nickname, "-r"], None)
            .await?;
        assert_eq!(stored, other_certificate.as_ref());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_changed_or_key_bearing_public_certificate() -> Result<(), Box<dyn Error>> {
        let home = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&home.path().join("ca"))?;
        let other_ca = CaInfo::load_or_create(&home.path().join("other-ca"))?;
        let original = fs::read_to_string(ca.certificate_path())?;
        fs::copy(other_ca.certificate_path(), ca.certificate_path())?;
        let store = CaTrustStore {
            platform: PlatformStore::for_user(home.path(), None),
        };
        assert!(matches!(
            store.install(&ca).await,
            Err(CaTrustError::CertificateChanged)
        ));
        assert!(!store.location().exists());
        fs::write(
            ca.certificate_path(),
            format!("{original}\n-----BEGIN PRIVATE KEY-----\n"),
        )?;
        assert!(matches!(
            store.install(&ca).await,
            Err(CaTrustError::CertificateChanged)
        ));
        assert!(!store.location().exists());
        Ok(())
    }

    #[tokio::test]
    async fn refuses_to_initialize_an_incomplete_database() -> Result<(), Box<dyn Error>> {
        let home = tempfile::tempdir()?;
        let ca = CaInfo::load_or_create(&home.path().join("ca"))?;
        let store = CaTrustStore {
            platform: PlatformStore::for_user(home.path(), None),
        };
        fs::create_dir_all(store.location())?;
        fs::write(store.location().join("key4.db"), b"existing key database")?;
        assert!(store.install(&ca).await.is_err());
        assert_eq!(
            fs::read(store.location().join("key4.db"))?,
            b"existing key database"
        );
        assert!(!store.location().join("cert9.db").exists());
        Ok(())
    }
}
