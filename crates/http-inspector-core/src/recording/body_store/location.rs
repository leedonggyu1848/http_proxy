//! The private per-session directory that holds the body store files.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs2::FileExt;

const LOCK_FILE: &str = "lock";
const DATA_FILE: &str = "data.bin";
const SESSION_PREFIX: &str = "session-";
/// Prefix of a session that is still being created and locked. Only after
/// its lock is held is it renamed to [`SESSION_PREFIX`], so cleanup by
/// another instance never sees an unlocked live session.
const PENDING_PREFIX: &str = "pending-";
/// An unlocked directory younger than this is skipped, so a session another
/// instance is creating right now is not removed.
const UNLOCKED_GRACE: Duration = Duration::from_mins(1);

/// A session directory that exists while this app session runs.
///
/// The directory holds an exclusively locked file, so other instances can
/// tell a live session from one left behind by a crash. Dropping the value
/// removes the directory.
#[derive(Debug)]
pub(super) struct SessionDirectory {
    path: PathBuf,
    _lock: File,
}

impl SessionDirectory {
    /// Removes sessions left behind by earlier runs, then creates a new
    /// private session directory inside `root`.
    pub(super) fn create(root: &Path) -> io::Result<Self> {
        create_private_root(root)?;
        remove_stale_sessions(root);
        let pending = tempfile::Builder::new()
            .prefix(PENDING_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root)?
            .keep();
        let created = lock_new_session(&pending).and_then(|lock| {
            let suffix = pending
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(PENDING_PREFIX))
                .ok_or_else(|| io::Error::other("unexpected session directory name"))?;
            let path = root.join(format!("{SESSION_PREFIX}{suffix}"));
            fs::rename(&pending, &path)?;
            Ok((path, lock))
        });
        let (path, lock) = match created {
            Ok(created) => created,
            Err(error) => {
                let _cleanup = fs::remove_dir_all(&pending);
                return Err(error);
            }
        };
        Ok(Self { path, _lock: lock })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Creates the data file, readable and writable only by this user.
    pub(super) fn create_data_file(&self) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.data_path())
    }

    pub(super) fn data_path(&self) -> PathBuf {
        self.path.join(DATA_FILE)
    }
}

impl Drop for SessionDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            tracing::warn!(%error, path = %self.path.display(), "failed to remove body store session");
        }
    }
}

fn create_private_root(root: &Path) -> io::Result<()> {
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)?;
    }
    match DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))
        }
        Err(error) => Err(error),
    }
}

fn lock_new_session(path: &Path) -> io::Result<File> {
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path.join(LOCK_FILE))?;
    lock.try_lock_exclusive()?;
    Ok(lock)
}

/// Removes session directories whose lock no running instance holds.
/// Failures are logged and skipped so one bad entry cannot block startup.
fn remove_stale_sessions(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
        let stale = if name.starts_with(SESSION_PREFIX) {
            is_stale(&path)
        } else if name.starts_with(PENDING_PREFIX) {
            // A pending session is unlocked for a moment while it is created.
            is_old(&path) && is_stale(&path)
        } else {
            false
        };
        if is_directory
            && stale
            && let Err(error) = fs::remove_dir_all(&path)
        {
            tracing::warn!(%error, path = %path.display(), "failed to remove a stale body store session");
        }
    }
}

fn is_stale(session: &Path) -> bool {
    match File::open(session.join(LOCK_FILE)) {
        // `try_lock_exclusive` fails while another process holds the lock.
        Ok(lock) => lock.try_lock_exclusive().is_ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => is_old(session),
        Err(_) => false,
    }
}

fn is_old(directory: &Path) -> bool {
    fs::metadata(directory)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= UNLOCKED_GRACE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> io::Result<u32> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }

    #[test]
    fn creates_private_files_and_removes_them_on_drop() -> io::Result<()> {
        let base = tempfile::tempdir()?;
        let root = base.path().join("cache").join("bodies");
        let session = SessionDirectory::create(&root)?;
        let data = session.create_data_file()?;
        drop(data);
        assert_eq!(mode(&root)?, 0o700);
        assert_eq!(mode(session.path())?, 0o700);
        assert_eq!(mode(&session.data_path())?, 0o600);
        let path = session.path().to_path_buf();
        drop(session);
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn tightens_an_existing_root() -> io::Result<()> {
        let base = tempfile::tempdir()?;
        let root = base.path().join("bodies");
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755))?;
        let _session = SessionDirectory::create(&root)?;
        assert_eq!(mode(&root)?, 0o700);
        Ok(())
    }

    #[test]
    fn removes_unlocked_sessions_but_keeps_live_ones() -> io::Result<()> {
        let base = tempfile::tempdir()?;
        let root = base.path().join("bodies");
        let live = SessionDirectory::create(&root)?;
        let stale = root.join("session-stale");
        fs::create_dir(&stale)?;
        File::create(stale.join(LOCK_FILE))?;
        fs::write(stale.join(DATA_FILE), b"old bytes")?;
        let fresh_unlocked = root.join("session-starting");
        fs::create_dir(&fresh_unlocked)?;
        let pending = root.join("pending-starting");
        fs::create_dir(&pending)?;
        File::create(pending.join(LOCK_FILE))?;
        let unrelated = root.join("notes");
        fs::create_dir(&unrelated)?;

        let next = SessionDirectory::create(&root)?;
        assert!(!stale.exists());
        assert!(live.path().exists());
        assert!(next.path().exists());
        assert!(fresh_unlocked.exists(), "a session being created is kept");
        assert!(pending.exists(), "a young pending session is kept");
        assert!(
            next.path()
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SESSION_PREFIX)),
            "a created session is renamed once locked"
        );
        assert!(unrelated.exists(), "only session directories are removed");
        Ok(())
    }
}
