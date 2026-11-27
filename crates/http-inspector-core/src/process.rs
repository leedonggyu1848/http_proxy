//! Local client process attribution, with OS-specific lookup behind this boundary.

use http_inspector_exchange::ProcessIdentity;

#[cfg(target_os = "linux")]
#[expect(
    clippy::redundant_pub_crate,
    reason = "platform implementation has crate-visible re-exports"
)]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{Lookup, register_browser};

#[cfg(not(target_os = "linux"))]
pub(crate) use unsupported::{Lookup, register_browser};

/// Whether a captured process is still running with the same PID and start time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessStatus {
    /// The original process is still running.
    Running,
    /// The original process exited.
    Exited,
    /// The process state cannot be read.
    Unknown,
}

/// Checks a captured identity without sending a signal to the process.
#[cfg(target_os = "linux")]
#[must_use]
pub fn process_status(identity: ProcessIdentity) -> ProcessStatus {
    linux::status(identity)
}

/// Returns unknown on platforms without process lookup.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub const fn process_status(_identity: ProcessIdentity) -> ProcessStatus {
    ProcessStatus::Unknown
}

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::redundant_pub_crate,
    reason = "platform implementation has crate-visible re-exports"
)]
mod unsupported {
    use std::net::SocketAddr;

    use http_inspector_exchange::{ProcessInfo, ProcessUnavailable};

    #[derive(Default)]
    pub(crate) struct Lookup {}

    impl Lookup {
        #[expect(
            clippy::unused_self,
            reason = "matches the stateful Linux lookup interface"
        )]
        pub(crate) const fn find(
            &self,
            _peer: SocketAddr,
            _proxy: SocketAddr,
        ) -> Result<ProcessInfo, ProcessUnavailable> {
            Err(ProcessUnavailable::Unsupported)
        }
    }

    pub(crate) struct BrowserRegistration;

    pub(crate) const fn register_browser(
        _pid: Option<u32>,
        _proxy: SocketAddr,
    ) -> Option<BrowserRegistration> {
        None
    }
}
