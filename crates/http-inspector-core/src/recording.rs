//! Session traffic recording: full-session journal logs and disk-backed body storage.

use std::sync::{Mutex, MutexGuard, PoisonError};

pub mod body_store;
pub mod journal;

pub use body_store::{
    BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES, BodyLayout, BodyStore, BulkProgress,
    BulkSave, BulkSummary, ContentRange, PreviewBodies, RangePlan, SaveBody, SaveFormat, SavedBody,
    StoreGap, StoredBody,
};
pub use journal::{FullLog, SESSION_BODY_LIMIT_BYTES};

/// Acquires `mutex`, recovering the inner value if a previous holder panicked.
///
/// Use only for locks whose guarded data maintains its invariants across a
/// panic during mutation.
fn lock_recovering<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
fn poison<T>(mutex: &Mutex<T>) {
    let _panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock();
        std::panic::resume_unwind(Box::new(()));
    }));
    assert!(mutex.is_poisoned());
}
