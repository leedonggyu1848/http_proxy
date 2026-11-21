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
/// How long `lock` keeps retrying a contended lock before reporting another registration.
///
/// A child process spawned by any thread of this process briefly holds a copy of every open
/// descriptor until it execs, so a lock released here can stay held for a few milliseconds.
const LOCK_WAIT: Duration = Duration::from_millis(500);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
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
        let _lock = self.lock().await?;
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
        let _lock = self.lock().await?;
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

    async fn lock(&self) -> Result<File, CaTrustError> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.database.join(".http-inspector.lock"))?;
        lock_exclusive_within(&lock, LOCK_WAIT).await?;
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

/// Takes an exclusive lock, retrying while it is contended for at most `wait`.
///
/// Retrying absorbs the short window in which a just-spawned child still holds an inherited
/// copy of a released lock; a registration that keeps the lock longer is still reported.
async fn lock_exclusive_within(lock: &File, wait: Duration) -> Result<(), CaTrustError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match FileExt::try_lock_exclusive(lock) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(LOCK_RETRY_INTERVAL).await;
            }
            Err(error) => {
                return Err(CaTrustError::Store(format!(
                    "Another CA registration may be running: {error}"
                )));
            }
        }
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

#[cfg(test)]
mod lock_tests {
    use std::error::Error;
    use std::fs::{File, OpenOptions};
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use fs2::FileExt;

    use super::{LOCK_WAIT, lock_exclusive_within};

    fn open(path: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    /// Regression for #101: children spawned by other threads inherit the released lock
    /// until they exec, which made an immediate relock fail with `EWOULDBLOCK`.
    #[tokio::test]
    async fn relocks_while_other_threads_spawn_children() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(".http-inspector.lock");
        let stop = Arc::new(AtomicBool::new(false));
        let spawners: Vec<_> = (0..4)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || -> std::io::Result<()> {
                    while !stop.load(Ordering::Relaxed) {
                        Command::new("true").status()?;
                    }
                    Ok(())
                })
            })
            .collect();
        let result = async {
            for _ in 0..300 {
                let lock = open(&path)?;
                lock_exclusive_within(&lock, LOCK_WAIT).await?;
            }
            Ok::<_, Box<dyn Error>>(())
        }
        .await;
        stop.store(true, Ordering::Relaxed);
        for spawner in spawners {
            spawner.join().map_err(|_| "spawner panicked")??;
        }
        result
    }

    #[tokio::test]
    async fn reports_a_lock_held_by_another_registration() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(".http-inspector.lock");
        let held = open(&path)?;
        FileExt::try_lock_exclusive(&held)?;
        let contender = open(&path)?;
        let error = lock_exclusive_within(&contender, Duration::from_millis(50))
            .await
            .err()
            .ok_or("a held lock must not be acquired")?;
        assert!(
            error
                .to_string()
                .contains("Another CA registration may be running")
        );
        Ok(())
    }
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
