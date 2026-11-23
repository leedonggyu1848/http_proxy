//! Health of the proxy engine thread as seen by the owning handle.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock_recovering<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a started proxy is still accepting connections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineState {
    /// The listener is accepting and forwarding connections.
    Running,
    /// The engine stopped on its own and no longer accepts connections.
    Failed {
        /// Human-readable cause.
        message: String,
    },
}

/// Failure slot shared between the engine thread and its handle.
#[derive(Clone, Debug, Default)]
pub(super) struct EngineStatus {
    failure: Arc<Mutex<Option<String>>>,
}

impl EngineStatus {
    /// Records why the engine stopped. The first cause is kept.
    pub(super) fn fail(&self, message: String) {
        let mut failure = lock_recovering(&self.failure);
        if failure.is_none() {
            *failure = Some(message);
        }
    }

    /// Returns the state, treating an engine thread that ended without a
    /// recorded cause as an unexpected stop.
    pub(super) fn state(&self, thread_finished: bool) -> EngineState {
        let failure = lock_recovering(&self.failure);
        match (failure.as_ref(), thread_finished) {
            (Some(message), _) => EngineState::Failed {
                message: message.clone(),
            },
            (None, true) => EngineState::Failed {
                message: "the proxy engine stopped unexpectedly".to_owned(),
            },
            (None, false) => EngineState::Running,
        }
    }
}

/// What the accept loop does after `accept` fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AcceptAction {
    /// Keep the listener: the error concerns one connection or a temporary
    /// resource shortage such as exhausted file descriptors.
    Retry,
    /// The listener itself is unusable.
    Stop,
}

pub(super) fn accept_action(error: &io::Error) -> AcceptAction {
    match error.kind() {
        io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported => AcceptAction::Stop,
        _ => AcceptAction::Retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_running_until_the_engine_fails() {
        let status = EngineStatus::default();
        assert_eq!(status.state(false), EngineState::Running);
        status.fail("listener closed".to_owned());
        status.fail("second cause".to_owned());
        assert_eq!(
            status.state(false),
            EngineState::Failed {
                message: "listener closed".to_owned()
            }
        );
    }

    #[test]
    fn treats_a_silently_finished_thread_as_failed() {
        let status = EngineStatus::default();
        assert!(matches!(status.state(true), EngineState::Failed { .. }));
    }

    #[test]
    fn retries_connection_and_resource_errors() {
        // EMFILE: too many open files.
        let exhausted = io::Error::from_raw_os_error(24);
        assert_eq!(accept_action(&exhausted), AcceptAction::Retry);
        let aborted = io::Error::from(io::ErrorKind::ConnectionAborted);
        assert_eq!(accept_action(&aborted), AcceptAction::Retry);
        let invalid = io::Error::from(io::ErrorKind::InvalidInput);
        assert_eq!(accept_action(&invalid), AcceptAction::Stop);
    }
}
