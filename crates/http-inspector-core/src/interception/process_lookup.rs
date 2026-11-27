use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use http_inspector_exchange::{ClientConnection, ProcessUnavailable};
use tokio::sync::Semaphore;

use crate::process::Lookup;
use crate::recording::FullLog;

// Keep /proc scans off forwarding threads and bound work even when
// many short connections close before their lookup gets a worker.
const WORKERS: usize = 4;
const OUTSTANDING: usize = 256;

pub(super) struct ConnectionLookup {
    lookup: Arc<Lookup>,
    workers: Arc<Semaphore>,
    outstanding: Arc<Semaphore>,
}

impl ConnectionLookup {
    pub(super) fn new() -> Self {
        Self {
            lookup: Arc::new(Lookup::default()),
            workers: Arc::new(Semaphore::new(WORKERS)),
            outstanding: Arc::new(Semaphore::new(OUTSTANDING)),
        }
    }

    pub(super) fn start(
        &self,
        peer: SocketAddr,
        proxy: SocketAddr,
        log: Option<&FullLog>,
    ) -> ClientConnection {
        let started = Instant::now();
        let (connection, writer) = ClientConnection::start(peer);
        let Ok(queued) = Arc::clone(&self.outstanding).try_acquire_owned() else {
            writer.finish(Err(ProcessUnavailable::Busy));
            return connection;
        };
        let workers = Arc::clone(&self.workers);
        let lookup = Arc::clone(&self.lookup);
        let log = log.cloned();
        let id = connection.id();
        let run = move |worker| {
            let (_worker, _queued) = (worker, queued);
            let worker_started = Instant::now();
            let result = lookup.find(peer, proxy);
            tracing::trace!(
                found = result.is_ok(),
                lookup_us = worker_started.elapsed().as_micros(),
                total_us = started.elapsed().as_micros(),
                "client process lookup"
            );
            if let (Some(log), Ok(info)) = (log.as_ref(), &result) {
                log.connection_process(id, info);
            }
            writer.finish(result);
        };
        if let Ok(worker) = Arc::clone(&workers).try_acquire_owned() {
            tokio::task::spawn_blocking(move || run(worker));
        } else {
            tokio::spawn(async move {
                if let Ok(worker) = workers.acquire_owned().await {
                    tokio::task::spawn_blocking(move || run(worker));
                }
            });
        }
        connection
    }
}
