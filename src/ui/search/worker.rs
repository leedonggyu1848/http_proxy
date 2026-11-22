//! A background thread that checks exchanges against the current query, so
//! decoding and searching bodies never blocks a UI frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use http_proxy::text_search::Matcher;
use http_proxy::{Exchange, RecordId};

use crate::ui::search::SearchScope;
use crate::ui::search::document::{self, BodyCache, Checked};

/// Exchanges checked before results are sent, so progress shows while a
/// long search runs.
const BATCH: usize = 64;
/// Jobs the UI may queue before it has to retry on a later frame.
const JOB_CAPACITY: usize = 64;
/// Result batches held for the UI. A full queue pauses the search thread.
const REPORT_CAPACITY: usize = 256;
/// Decoded bodies kept before bodies of records no longer shown are dropped.
const CACHE_PRUNE_LEN: usize = 4_096;

/// One compiled query; `generation` grows each time the query changes.
#[derive(Debug)]
pub(in crate::ui) struct Query {
    pub(in crate::ui) generation: u64,
    pub(in crate::ui) matcher: Matcher,
    pub(in crate::ui) scope: SearchScope,
}

struct Job {
    query: Arc<Query>,
    records: Vec<Exchange>,
    /// Oldest record still shown; the cache may drop bodies of older ones.
    oldest: Option<RecordId>,
}

/// Results of part of one job.
pub(in crate::ui) struct Report {
    pub(in crate::ui) generation: u64,
    pub(in crate::ui) checked: Vec<Checked>,
}

pub(in crate::ui) struct SearchWorker {
    jobs: SyncSender<Job>,
    reports: Receiver<Report>,
    latest: Arc<AtomicU64>,
}

impl SearchWorker {
    /// Starts the search thread; `wake` asks the UI to read new results.
    pub(in crate::ui) fn spawn(wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (jobs, job_receiver) = mpsc::sync_channel(JOB_CAPACITY);
        let (report_sender, reports) = mpsc::sync_channel(REPORT_CAPACITY);
        let latest = Arc::new(AtomicU64::new(0));
        let thread_latest = Arc::clone(&latest);
        std::thread::Builder::new()
            .name("request-search".to_owned())
            .spawn(move || run(&job_receiver, &report_sender, &thread_latest, &wake))?;
        Ok(Self {
            jobs,
            reports,
            latest,
        })
    }

    /// Makes queued and running work for older queries stop early.
    pub(in crate::ui) fn supersede(&self, generation: u64) {
        self.latest.store(generation, Ordering::Release);
    }

    /// Queues `records` for `query`. Returns the records back when the
    /// queue is full, so the caller can try again on a later frame.
    pub(in crate::ui) fn submit(
        &self,
        query: &Arc<Query>,
        records: Vec<Exchange>,
        oldest: Option<RecordId>,
    ) -> Result<(), Vec<Exchange>> {
        let job = Job {
            query: Arc::clone(query),
            records,
            oldest,
        };
        self.jobs.try_send(job).map_err(|error| match error {
            TrySendError::Full(job) | TrySendError::Disconnected(job) => job.records,
        })
    }

    /// Returns a result batch, if one is ready.
    pub(in crate::ui) fn try_report(&self) -> Option<Report> {
        self.reports.try_recv().ok()
    }
}

fn run(jobs: &Receiver<Job>, reports: &SyncSender<Report>, latest: &AtomicU64, wake: &dyn Fn()) {
    let mut cache = BodyCache::default();
    let current = |generation: u64| latest.load(Ordering::Acquire) == generation;
    while let Ok(job) = jobs.recv() {
        let generation = job.query.generation;
        if !current(generation) {
            continue;
        }
        if let Some(oldest) = job.oldest
            && cache.len() > CACHE_PRUNE_LEN
        {
            cache.forget_before(oldest);
        }
        for batch in job.records.chunks(BATCH) {
            if !current(generation) {
                break;
            }
            let checked = batch
                .iter()
                .map(|record| {
                    document::check(record, &job.query.matcher, job.query.scope, &mut cache)
                })
                .collect();
            if reports
                .send(Report {
                    generation,
                    checked,
                })
                .is_err()
            {
                return;
            }
            wake();
        }
    }
}
