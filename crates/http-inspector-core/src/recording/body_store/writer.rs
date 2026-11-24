//! The thread that appends queued body chunks to the data file.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use super::Shared;
use super::index::{BodyKey, StoreGap};
use crate::recording::lock_recovering;

/// Commands processed before the index is updated, so a busy writer still
/// publishes progress regularly.
const BATCH_COMMANDS: usize = 256;
/// Bytes written between free disk space checks.
const DISK_CHECK_BYTES: u64 = 16 * 1024 * 1024;
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;
/// How often an idle writer checks whether low disk space has recovered.
const IDLE_DISK_CHECK: Duration = Duration::from_secs(5);

pub(super) enum Command {
    /// Body bytes copied from the forwarding path.
    Chunk { key: BodyKey, bytes: Vec<u8> },
    /// Answered once every earlier chunk is written and indexed.
    Barrier(SyncSender<()>),
}

struct Writer<'a> {
    output: BufWriter<File>,
    shared: &'a Shared,
    directory: &'a Path,
    offset: u64,
    /// Ranges written to the buffer but not yet flushed and indexed.
    pending: Vec<(BodyKey, u64, u64)>,
    since_disk_check: u64,
}

/// Writes chunks until every sender is dropped.
pub(super) fn run(receiver: &Receiver<Command>, file: File, shared: &Shared, directory: &Path) {
    let mut writer = Writer {
        output: BufWriter::with_capacity(WRITE_BUFFER_BYTES, file),
        shared,
        directory,
        offset: 0,
        pending: Vec::new(),
        since_disk_check: 0,
    };
    writer.check_disk();
    loop {
        let first = match receiver.recv_timeout(IDLE_DISK_CHECK) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => {
                if writer.shared.disk_low.load(Ordering::Acquire) {
                    writer.check_disk();
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        writer.handle(first);
        for command in receiver.try_iter().take(BATCH_COMMANDS) {
            writer.handle(command);
        }
        writer.commit();
    }
    writer.commit();
}

impl Writer<'_> {
    fn handle(&mut self, command: Command) {
        match command {
            Command::Chunk { key, bytes } => self.write(key, &bytes),
            Command::Barrier(reply) => {
                self.commit();
                let _requester_gone = reply.send(());
            }
        }
    }

    fn write(&mut self, key: BodyKey, bytes: &[u8]) {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.shared.queued_bytes.fetch_sub(length, Ordering::AcqRel);
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        if self.shared.failed.load(Ordering::Acquire) {
            self.shared.mark_gap(key, StoreGap::WriteFailed);
            return;
        }
        match self.output.write_all(bytes) {
            Ok(()) => {
                self.pending.push((key, self.offset, length));
                self.offset = self.offset.saturating_add(length);
                self.since_disk_check = self.since_disk_check.saturating_add(length);
                if self.since_disk_check >= DISK_CHECK_BYTES {
                    self.check_disk();
                }
            }
            Err(error) => {
                self.fail(&error);
                self.shared.mark_gap(key, StoreGap::WriteFailed);
            }
        }
    }

    /// Flushes written chunks and adds their ranges to the index.
    fn commit(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        if let Err(error) = self.output.flush() {
            self.fail(&error);
            return;
        }
        let mut index = lock_recovering(&self.shared.index);
        for (key, offset, length) in self.pending.drain(..) {
            index.append(key, offset, length);
        }
    }

    /// Stops storing after a write error and marks unindexed bodies as incomplete.
    fn fail(&mut self, error: &io::Error) {
        self.shared.record_write_error(error);
        for (key, _, _) in std::mem::take(&mut self.pending) {
            self.shared.mark_gap(key, StoreGap::WriteFailed);
        }
    }

    fn check_disk(&mut self) {
        self.since_disk_check = 0;
        let low = fs2::available_space(self.directory)
            .is_ok_and(|available| available < self.shared.min_free_bytes);
        self.shared.disk_low.store(low, Ordering::Release);
    }
}
