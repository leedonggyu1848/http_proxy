//! The control channel between the command line and the daemon: a Unix
//! domain socket in a directory only the current user can open.
//!
//! The directory is `$XDG_RUNTIME_DIR/http-inspector` when that variable
//! names an absolute path, otherwise `http-inspector-<uid>` in the system
//! temporary directory. Both the directory and the socket must belong to the
//! current user and grant nothing to the group or others; anything else is
//! refused instead of being used or repaired. No TCP control port exists.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt,
};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::error::{CliError, ErrorCode};

const DIRECTORY_NAME: &str = "http-inspector";
const SOCKET_NAME: &str = "control.sock";
const LOCK_NAME: &str = "control.lock";
/// Permission bits for group and others, which must all be clear.
const SHARED_BITS: u32 = 0o077;

/// Where the control socket lives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlPath {
    directory: PathBuf,
    socket: PathBuf,
}

impl ControlPath {
    /// Chooses the control directory from the environment.
    pub(crate) fn locate(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Self {
        let runtime = lookup("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let directory = runtime.map_or_else(
            || {
                std::env::temp_dir().join(format!(
                    "{DIRECTORY_NAME}-{}",
                    rustix::process::getuid().as_raw()
                ))
            },
            |runtime| runtime.join(DIRECTORY_NAME),
        );
        Self::in_directory(directory)
    }

    pub(crate) fn in_directory(directory: PathBuf) -> Self {
        let socket = directory.join(SOCKET_NAME);
        Self { directory, socket }
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    /// Creates the directory with mode 0700, or checks an existing one.
    pub(crate) fn prepare_directory(&self) -> Result<(), CliError> {
        match DirBuilder::new().mode(0o700).create(&self.directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => self.check_directory(),
            Err(error) => Err(CliError::io(
                &format!("Could not create {}", self.directory.display()),
                &error,
            )),
        }
    }

    fn check_directory(&self) -> Result<(), CliError> {
        let metadata = fs::symlink_metadata(&self.directory)
            .map_err(|error| CliError::io("Could not read the control directory", &error))?;
        if !metadata.is_dir() {
            return Err(insecure(&self.directory, "is not a directory"));
        }
        check_private(metadata.uid(), metadata.mode(), current_uid())
            .map_err(|reason| insecure(&self.directory, reason))
    }

    /// Connects to a running daemon.
    pub(crate) fn connect(&self) -> Result<UnixStream, CliError> {
        let metadata = match fs::symlink_metadata(&self.socket) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CliError::daemon_not_running());
            }
            Err(error) => return Err(CliError::io("Could not read the control socket", &error)),
        };
        self.check_directory()?;
        if !metadata.file_type().is_socket() {
            return Err(insecure(&self.socket, "is not a socket"));
        }
        check_private(metadata.uid(), metadata.mode(), current_uid())
            .map_err(|reason| insecure(&self.socket, reason))?;
        UnixStream::connect(&self.socket).map_err(|error| match error.kind() {
            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => {
                CliError::daemon_not_running()
            }
            _ => CliError::io("Could not connect to the daemon", &error),
        })
    }

    /// Takes the startup lock. Every daemon holds it while it checks for a
    /// live daemon, removes a stale socket, and binds, and while it removes
    /// its own socket at exit, so two daemons never take the same path.
    fn lock(&self) -> Result<File, CliError> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.directory.join(LOCK_NAME))
            .map_err(|error| CliError::io("Could not open the control lock", &error))?;
        lock.lock()
            .map_err(|error| CliError::io("Could not take the control lock", &error))?;
        Ok(lock)
    }

    /// Binds the socket with mode 0600, replacing a socket that no daemon
    /// answers on. Fails with `daemon_already_running` when a daemon is
    /// listening.
    pub(crate) fn bind(&self) -> Result<Bound, CliError> {
        self.prepare_directory()?;
        let _lock = self.lock()?;
        match self.connect() {
            Ok(_) => {
                return Err(CliError::new(
                    ErrorCode::DaemonAlreadyRunning,
                    "Another HTTP Inspector daemon is already running",
                )
                .with_hint("run `http-proxy proxy status`"));
            }
            Err(error) if error.code == ErrorCode::DaemonNotRunning => {}
            Err(error) => return Err(error),
        }
        // Nobody answers on the path, and no other daemon can bind while we
        // hold the lock, so the socket file is stale.
        if fs::symlink_metadata(&self.socket).is_ok() {
            fs::remove_file(&self.socket)
                .map_err(|error| CliError::io("Could not remove a stale control socket", &error))?;
        }
        let listener = UnixListener::bind(&self.socket).map_err(|error| {
            let failure = CliError::io(
                &format!("Could not listen on {}", self.socket.display()),
                &error,
            );
            if error.kind() == io::ErrorKind::InvalidInput {
                // Unix socket paths are limited to about 100 bytes.
                failure.with_hint("set XDG_RUNTIME_DIR to a shorter directory")
            } else {
                failure
            }
        })?;
        fs::set_permissions(&self.socket, Permissions::from_mode(0o600))
            .map_err(|error| CliError::io("Could not restrict the control socket", &error))?;
        Ok(Bound { listener })
    }

    /// Removes the socket file, then closes the listener. Both happen under
    /// the startup lock while the listener still accepts, so any other
    /// daemon that checks the path meanwhile sees this one alive and backs
    /// off; once the lock is released the path is free for a new daemon,
    /// and this daemon never touches it again.
    pub(crate) fn close(&self, bound: Bound) {
        let lock = self.lock();
        let _already_gone = fs::remove_file(&self.socket).is_err();
        drop(bound.listener);
        drop(lock);
    }
}

/// A bound control socket. Release it with [`ControlPath::close`].
pub(crate) struct Bound {
    pub(crate) listener: UnixListener,
}

fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Accepts a file only when `owner` is `user` and no group or other
/// permission bit is set in `mode`.
pub(crate) const fn check_private(owner: u32, mode: u32, user: u32) -> Result<(), &'static str> {
    if owner != user {
        return Err("belongs to another user");
    }
    if mode & SHARED_BITS != 0 {
        return Err("is accessible to other users");
    }
    Ok(())
}

fn insecure(path: &Path, reason: &str) -> CliError {
    CliError::new(
        ErrorCode::InsecureControlPath,
        format!("Refusing to use {}: it {reason}", path.display()),
    )
    .with_hint("remove it, or set XDG_RUNTIME_DIR to a directory only you can open")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_private_paths_of_the_current_user() {
        assert_eq!(check_private(1000, 0o140_600, 1000), Ok(()));
        assert_eq!(check_private(1000, 0o040_700, 1000), Ok(()));
        assert_eq!(
            check_private(0, 0o140_600, 1000),
            Err("belongs to another user")
        );
        assert_eq!(
            check_private(1000, 0o140_660, 1000),
            Err("is accessible to other users")
        );
        assert_eq!(
            check_private(1000, 0o040_755, 1000),
            Err("is accessible to other users")
        );
    }

    #[test]
    fn prefers_an_absolute_runtime_directory() {
        let located = ControlPath::locate(|_| Some("/run/user/1000".into()));
        assert_eq!(
            located.socket(),
            Path::new("/run/user/1000/http-inspector/control.sock")
        );
        let relative = ControlPath::locate(|_| Some("relative".into()));
        assert!(relative.socket().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn creates_a_private_directory_and_socket() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let control = ControlPath::in_directory(root.path().join("control"));
        let first = control.bind()?;
        let directory = fs::metadata(root.path().join("control"))?;
        assert_eq!(directory.mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(control.socket())?.mode() & 0o777, 0o600);
        let _client = control.connect()?;
        // A live daemon keeps its path.
        let second = control.bind().err().map(|error| error.code);
        assert_eq!(second, Some(ErrorCode::DaemonAlreadyRunning));
        assert!(control.socket().exists());

        // Closing removes the path before the listener goes away, so a new
        // daemon can bind and nothing of the old one removes its socket.
        control.close(first);
        assert!(!control.socket().exists());
        let replacement = control.bind()?;
        let _client = control.connect()?;

        // A daemon that died without closing leaves a stale socket, which
        // the next bind replaces.
        drop(replacement.listener);
        assert!(control.socket().exists());
        let after_crash = control.bind()?;
        let _client = control.connect()?;
        control.close(after_crash);
        Ok(())
    }

    #[test]
    fn refuses_a_shared_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let shared = root.path().join("control");
        fs::create_dir(&shared)?;
        fs::set_permissions(&shared, Permissions::from_mode(0o777))?;
        let control = ControlPath::in_directory(shared);
        let error = control.bind().err().ok_or("bind must fail")?;
        assert_eq!(error.code, ErrorCode::InsecureControlPath);
        Ok(())
    }
}
