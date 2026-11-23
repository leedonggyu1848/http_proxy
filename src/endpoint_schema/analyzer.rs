//! Background thread that describes finished exchanges.
//!
//! The UI thread only queues exchanges and merges finished observations.
//! Waiting for bodies to finish, reading them from the body store and
//! parsing them all happen on this thread, so neither the UI nor the
//! forwarding path waits for disk or parsing.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::thread;
use std::time::Duration;

use super::endpoint::EndpointObservation;
use super::observe::{CapturedBodies, observe_exchange};
use super::store::{EndpointKey, EndpointSchemas};
use crate::Exchange;

/// Exchanges that may wait in the queue to the analysis thread.
const JOB_CAPACITY: usize = 4_096;
/// Finished observations that may wait for the UI thread.
const RESULT_CAPACITY: usize = 1_024;
/// Exchanges still in progress that the analysis thread keeps watching;
/// beyond this the oldest is described as it is, with unfinished bodies
/// counted as truncated.
const MAX_WATCHED: usize = 4_096;
/// How often unfinished exchanges are checked.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

struct Job {
    generation: u64,
    key: EndpointKey,
    exchange: Exchange,
}

struct Finished {
    generation: u64,
    key: EndpointKey,
    observation: EndpointObservation,
}

/// Handle to the schema analysis thread. Dropping it stops the thread
/// after its current exchange.
#[derive(Debug)]
pub struct SchemaAnalyzer {
    jobs: SyncSender<Job>,
    results: Receiver<Finished>,
    generation: Arc<AtomicU64>,
    skipped: u64,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Finished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Finished")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl SchemaAnalyzer {
    /// Starts the analysis thread. `wake` is called after each finished
    /// exchange so the UI can merge it.
    pub fn spawn(bodies: CapturedBodies, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (jobs, job_receiver) = sync_channel(JOB_CAPACITY);
        let (result_sender, results) = sync_channel(RESULT_CAPACITY);
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        let _detached = thread::Builder::new()
            .name("endpoint-schema".to_owned())
            .spawn(move || {
                Worker {
                    jobs: job_receiver,
                    results: result_sender,
                    generation: worker_generation,
                    bodies,
                    watched: Vec::new(),
                }
                .run(&wake);
            })?;
        Ok(Self {
            jobs,
            results,
            generation,
            skipped: 0,
        })
    }

    /// Queues `exchange` for description without waiting. When the queue is
    /// full the exchange is counted as skipped instead.
    pub fn submit(&mut self, key: EndpointKey, exchange: Exchange) {
        let job = Job {
            generation: self.generation.load(Ordering::Acquire),
            key,
            exchange,
        };
        match self.jobs.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.skipped = self.skipped.saturating_add(1);
            }
        }
    }

    /// Discards every queued and unfinished exchange, as on `Clear`.
    pub fn reset(&mut self) {
        let _previous = self.generation.fetch_add(1, Ordering::AcqRel);
        self.skipped = 0;
        while self.results.try_recv().is_ok() {}
    }

    /// Merges finished observations into `schemas` without waiting.
    /// Returns `true` when anything changed.
    pub fn drain_into(&mut self, schemas: &mut EndpointSchemas) -> bool {
        let current = self.generation.load(Ordering::Acquire);
        let mut changed = false;
        while let Ok(finished) = self.results.try_recv() {
            if finished.generation == current {
                schemas.record(&finished.key, &finished.observation);
                changed = true;
            }
        }
        if self.skipped > 0 {
            schemas.note_skipped(std::mem::take(&mut self.skipped));
            changed = true;
        }
        changed
    }
}

struct Worker {
    jobs: Receiver<Job>,
    results: SyncSender<Finished>,
    generation: Arc<AtomicU64>,
    bodies: CapturedBodies,
    watched: Vec<Job>,
}

impl Worker {
    fn run(mut self, wake: &dyn Fn()) {
        while self.receive() {
            let current = self.generation.load(Ordering::Acquire);
            self.watched.retain(|job| job.generation == current);
            if !self.describe_finished(wake) {
                return;
            }
        }
    }

    /// Waits for new jobs, or for the next poll while exchanges are being
    /// watched. Returns `false` once the handle is dropped.
    fn receive(&mut self) -> bool {
        let first = if self.watched.is_empty() {
            match self.jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => return false,
            }
        } else {
            match self.jobs.recv_timeout(POLL_INTERVAL) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        };
        self.watched.extend(first);
        loop {
            match self.jobs.try_recv() {
                Ok(job) => self.watched.push(job),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// Describes every finished exchange, and the oldest unfinished ones
    /// beyond [`MAX_WATCHED`]. Returns `false` once the handle is dropped.
    fn describe_finished(&mut self, wake: &dyn Fn()) -> bool {
        let overflow = self.watched.len().saturating_sub(MAX_WATCHED);
        let mut ready = Vec::new();
        let mut index = 0;
        self.watched.retain(|job| {
            let take = index < overflow || !job.exchange.is_live();
            index += 1;
            if take {
                ready.push((job.generation, job.key.clone(), job.exchange.clone()));
            }
            !take
        });
        for (generation, key, exchange) in ready {
            if generation != self.generation.load(Ordering::Acquire) {
                continue;
            }
            let observation = observe_exchange(&exchange, &self.bodies);
            let finished = Finished {
                generation,
                key,
                observation,
            };
            if self.results.send(finished).is_err() {
                return false;
            }
            wake();
        }
        true
    }
}
