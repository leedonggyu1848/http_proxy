//! Background thread that describes finished exchanges.
//!
//! The UI thread only queues exchanges and merges finished observations.
//! This thread reads completed bodies from the [`BodySource`] and parses
//! them, so neither the UI nor the forwarding path waits for disk or parsing.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use super::endpoint::EndpointObservation;
use super::observe::observe_exchange;
use super::store::{EndpointKey, EndpointSchemas};
use http_inspector_exchange::{BodySource, Exchange};

/// Exchanges that may wait in the queue to the analysis thread.
const JOB_CAPACITY: usize = 4_096;
/// Finished observations that may wait for the UI thread.
const RESULT_CAPACITY: usize = 1_024;

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
    /// Starts the analysis thread, which reads structured bodies from
    /// `bodies`. `wake` is called after each finished exchange so the UI
    /// can merge it.
    pub fn spawn(
        bodies: Arc<dyn BodySource>,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
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

    /// Queues a finished `exchange` for description without waiting. Counts
    /// live exchanges and exchanges that cannot be queued as skipped.
    pub fn submit(&mut self, key: EndpointKey, exchange: Exchange) {
        if exchange.is_live()
            || self
                .jobs
                .try_send(Job {
                    generation: self.generation.load(Ordering::Acquire),
                    key,
                    exchange,
                })
                .is_err()
        {
            self.skipped = self.skipped.saturating_add(1);
        }
    }

    /// Discards every queued exchange, as on `Clear`.
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
    bodies: Arc<dyn BodySource>,
}

impl Worker {
    fn run(self, wake: &dyn Fn()) {
        while let Ok(Job {
            generation,
            key,
            exchange,
        }) = self.jobs.recv()
        {
            if generation != self.generation.load(Ordering::Acquire) {
                continue;
            }
            let observation = observe_exchange(&exchange, self.bodies.as_ref());
            drop(exchange);
            let finished = Finished {
                generation,
                key,
                observation,
            };
            if self.results.send(finished).is_err() {
                return;
            }
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http::{Method, StatusCode};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    use super::*;
    use crate::url_map::UrlMap;

    #[test]
    fn full_queue_counts_the_exchange_as_skipped() -> Result<(), Box<dyn Error>> {
        let (jobs, pending) = sync_channel(1);
        let (_results, results) = sync_channel(1);
        let mut analyzer = SchemaAnalyzer {
            jobs,
            results,
            generation: Arc::new(AtomicU64::new(0)),
            skipped: 0,
        };
        let exchange = ExchangeBuilder::http(Method::GET, "http://fixture.test/api")?
            .respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
        let node = UrlMap::new()
            .observe_exchange(&exchange)
            .ok_or("missing node")?;
        let key = EndpointKey {
            node,
            method: Method::GET,
        };
        analyzer.submit(key.clone(), exchange.clone());
        analyzer.submit(key, exchange);

        let mut schemas = EndpointSchemas::new();
        assert!(analyzer.drain_into(&mut schemas));
        assert_eq!(schemas.skipped_requests(), 1);
        assert!(!analyzer.drain_into(&mut schemas));
        assert!(pending.try_recv().is_ok());
        assert!(pending.try_recv().is_err());
        Ok(())
    }
}
