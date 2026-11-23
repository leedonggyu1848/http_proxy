//! Manual request replay: draft validation and execution dispatch.

mod draft;

pub use crate::interception::ReplayTask;
use crate::interception::{ProxyHandle, execute_replay};
pub use draft::{REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField};

impl ProxyHandle {
    /// Sends a user-approved draft on the running proxy, recording a new exchange.
    pub fn replay(&self, draft: &ReplayDraft) -> Result<ReplayTask, String> {
        let request = draft.request(self.local_addr())?;
        execute_replay(self, request)
    }
}
