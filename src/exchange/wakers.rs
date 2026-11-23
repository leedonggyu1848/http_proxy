//! Tasks waiting for a one-time event, tracked without an async runtime.

use std::sync::Mutex;
use std::task::Waker;

use super::lock_recovering;

/// Wakers registered by tasks that wait for one event.
#[derive(Debug, Default)]
pub(super) struct WakerSet {
    wakers: Mutex<Vec<Waker>>,
}

impl WakerSet {
    /// Registers a task to wake. A task that polls again is kept only once.
    pub(super) fn register(&self, waker: &Waker) {
        let mut wakers = lock_recovering(&self.wakers);
        if !wakers.iter().any(|registered| registered.will_wake(waker)) {
            wakers.push(waker.clone());
        }
    }

    /// Wakes and forgets every registered task.
    pub(super) fn wake_all(&self) {
        let wakers = std::mem::take(&mut *lock_recovering(&self.wakers));
        for waker in wakers {
            waker.wake();
        }
    }
}
