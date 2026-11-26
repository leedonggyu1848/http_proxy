//! The control channel between the command line and the daemon: a Unix
//! domain socket in a directory only the current user can open.
//!
//! The directory is `$XDG_RUNTIME_DIR/http-inspector` when that variable
//! names an absolute path, otherwise `http-inspector-<uid>` in the system
//! temporary directory. Both the directory and the socket must belong to the
//! current user and grant nothing to the group or others; anything else is
//! refused instead of being used or repaired. No TCP control port exists.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::error::{CliError, ErrorCode};

const DIRECTORY_NAME: &str = "http-inspector";
const SOCKET_NAME: &str = "control.sock";
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

    /// Binds the socket with mode 0600, replacing a socket that no daemon
    /// answers on. Fails when a daemon is already listening.
    pub(crate) fn bind(&self) -> Result<UnixListener, CliError> {
        self.prepare_directory()?;
        match self.connect() {
            Ok(_) => {
                return Err(CliError::new(
                    ErrorCode::ProxyStartFailed,
                    "Another HTTP Inspector daemon is already running",
                )
                .with_hint("run `http-proxy proxy status`"));
            }
            Err(error) if error.code == ErrorCode::DaemonNotRunning => {}
            Err(error) => return Err(error),
        }
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
        Ok(listener)
    }

    /// Removes the socket file when the daemon stops.
    pub(crate) fn remove_socket(&self) {
        let _already_gone = fs::remove_file(&self.socket).is_err();
    }
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
        let listener = control.bind()?;
        let directory = fs::metadata(root.path().join("control"))?;
        assert_eq!(directory.mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(control.socket())?.mode() & 0o777, 0o600);
        let _client = control.connect()?;
        drop(listener);
        // A socket nobody listens on is stale and replaced.
        let _listener = control.bind()?;
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
