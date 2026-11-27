//! Dedicated private browser sessions using the inspector's proxy, and
//! terminal windows that run an inspection shell.

mod discovery;
mod launch;
mod terminal;

pub use terminal::{Terminal, TerminalLaunch, TerminalLaunchError};

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use tokio::sync::oneshot;

/// An installed, supported browser executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Browser {
    name: &'static str,
    executable: PathBuf,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Firefox { snap: bool },
    Chrome,
}

use crate::https::{CaTrustError, HttpsSetupError};

/// Reason an inspection browser session failed to start or exited with an error.
#[derive(Debug, thiserror::Error)]
pub enum BrowserLaunchError {
    #[error("Browser inspection requires a listening loopback proxy")]
    InvalidProxyAddress,
    #[error("Firefox Snap requires an absolute HOME")]
    MissingSnapHome,
    #[error("{browser} exited before opening a window ({status})")]
    ExitedBeforeOpen {
        browser: &'static str,
        status: std::process::ExitStatus,
    },
    #[error("{browser} exited with {status}")]
    ExitedWithStatus {
        browser: &'static str,
        status: std::process::ExitStatus,
    },
    #[error(transparent)]
    CaSetup(#[from] HttpsSetupError),
    #[error(transparent)]
    CaTrust(#[from] CaTrustError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Progress from the background browser launcher.
pub enum BrowserEvent {
    /// A separate browser process is running.
    Opened,
    /// The browser exited or preparation failed.
    Finished(Result<(), BrowserLaunchError>),
}

/// Owns a browser worker and closes its browser on drop.
pub struct BrowserSession {
    events: Receiver<BrowserEvent>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Browser {
    /// Finds Firefox and Chrome/Chromium on PATH and in macOS application bundles.
    #[must_use]
    pub fn installed() -> Vec<Self> {
        discovery::installed()
    }

    /// The browser name to display in the chooser.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Whether HTTPS inspection uses the current user's shared trust store.
    #[must_use]
    pub const fn uses_shared_ca_store(&self) -> bool {
        matches!(self.kind, Kind::Chrome)
    }

    /// Opens a new private profile in a background worker.
    /// CA registration is performed only when `ca_directory` is supplied.
    pub fn open(
        &self,
        proxy: SocketAddr,
        ca_directory: Option<PathBuf>,
    ) -> Result<BrowserSession, std::io::Error> {
        launch::open(self.clone(), proxy, ca_directory)
    }
}

impl BrowserSession {
    /// Receives the next progress event without blocking the UI.
    #[must_use]
    pub fn try_receive(&mut self) -> Option<BrowserEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _already_finished = stop.send(()).is_err();
        }
        if let Some(worker) = self.worker.take() {
            let _worker_panicked = worker.join().is_err();
        }
    }
}
