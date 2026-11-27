//! Client attribution shared by every exchange on one accepted connection.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A process identity that survives PID reuse within one boot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessIdentity {
    /// Operating system process ID.
    pub pid: u32,
    /// Time since boot, in kernel clock ticks.
    pub start_ticks: u64,
}

/// A process in the parent chain, read at the time of attribution.
#[derive(Debug)]
pub struct ProcessAncestor {
    /// Identity of this parent when it was observed.
    pub identity: ProcessIdentity,
    /// Name reported by the operating system.
    pub name: String,
}

/// A snapshot of the client process that owned the connection.
#[derive(Debug)]
pub struct ProcessInfo {
    /// Stable identity for filtering records and detecting exit.
    pub identity: ProcessIdentity,
    /// Name reported by the operating system.
    pub name: String,
    /// Executable path when readable.
    pub executable: Option<PathBuf>,
    /// User ID reported by the operating system.
    pub uid: u32,
    /// User name when it can be resolved.
    pub user: Option<String>,
    /// Wall-clock start time when the boot time is available.
    pub started_at: Option<SystemTime>,
    /// Command line after redaction, when readable.
    pub redacted_command: Option<String>,
    /// Parents ordered from the immediate parent towards the root.
    pub parents: Vec<ProcessAncestor>,
    /// Browser process at the root of this browser's process group, if found.
    pub browser: Option<ProcessAncestor>,
    /// Whether this process belongs to an inspection browser session.
    pub inspection_browser: bool,
}

/// Why connection attribution could not return a client process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessUnavailable {
    /// The operating system has no implementation.
    Unsupported,
    /// The socket disappeared or is outside the visible network namespace.
    SocketUnavailable,
    /// The process disappeared or its descriptors are inaccessible.
    OwnerUnavailable,
    /// The socket belongs to a different user.
    DifferentUser(u32),
    /// The OS denied access to socket or process information.
    PermissionDenied,
    /// The process lookup queue was full.
    Busy,
    /// Lookup stopped before reaching a result.
    Interrupted,
}

type Attribution = Result<ProcessInfo, ProcessUnavailable>;

#[derive(Debug)]
struct ConnectionInner {
    id: u64,
    peer: SocketAddr,
    requests: AtomicU64,
    process: OnceLock<Attribution>,
}

/// Read handle shared across the HTTP requests and CONNECT exchanges on a TCP connection.
#[derive(Clone, Debug)]
pub struct ClientConnection(Arc<ConnectionInner>);

impl ClientConnection {
    /// Begins attribution and returns its read and write handles.
    #[must_use]
    pub fn start(peer: SocketAddr) -> (Self, ProcessLookupWriter) {
        let connection = Self(Arc::new(ConnectionInner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            peer,
            requests: AtomicU64::new(0),
            process: OnceLock::new(),
        }));
        (connection.clone(), ProcessLookupWriter(connection))
    }

    /// Numeric connection identity, unique within this app process.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.0.id
    }

    /// Client address as observed by the listener.
    #[must_use]
    pub fn peer(&self) -> SocketAddr {
        self.0.peer
    }

    /// Number of exchanges started on this connection, including CONNECT.
    #[must_use]
    pub fn request_count(&self) -> u64 {
        self.0.requests.load(Ordering::Relaxed)
    }

    /// Attribution result once ready. An absent result means lookup is pending.
    #[must_use]
    pub fn process(&self) -> Option<&Attribution> {
        self.0.process.get()
    }

    pub(super) fn record_request(&self) {
        self.0.requests.fetch_add(1, Ordering::Relaxed);
    }
}

/// Publishes one attribution result. Dropping a pending writer marks it interrupted.
pub struct ProcessLookupWriter(ClientConnection);

impl ProcessLookupWriter {
    /// Finishes attribution for every exchange sharing the connection.
    pub fn finish(self, result: Attribution) {
        let _already_set = self.0.0.process.set(result);
    }
}

impl Drop for ProcessLookupWriter {
    fn drop(&mut self) {
        let _result = self
            .0
            .0
            .process
            .get_or_init(|| Err(ProcessUnavailable::Interrupted));
    }
}
