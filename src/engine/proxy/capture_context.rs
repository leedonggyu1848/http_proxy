use super::handle::Engine;
use crate::engine::capture::BodyLog;
use crate::engine::capture::{ERROR_BYTES, TARGET_BYTES, bounded_text, capture_headers};
use crate::engine::domain::{
    CaptureSource, CapturedRequest, Exchange, FailureStage, Outcome, RecordId, TunnelSnapshot,
};
use crate::engine::journal::BodySide;
use crate::engine::journal::FullLog;
use http::{HeaderMap, Method};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(1);

/// Callback run after an exchange was queued, for example to wake a UI.
pub(super) type EventNotifier = Box<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(super) struct EventSink {
    pub(super) sender: mpsc::Sender<Exchange>,
    pub(super) dropped: Arc<AtomicU64>,
    pub(super) notifier: Arc<OnceLock<EventNotifier>>,
    pub(super) log: Option<FullLog>,
}

impl EventSink {
    fn send(&self, exchange: Exchange) {
        match self.sender.try_send(exchange) {
            Ok(()) => {
                if let Some(notify) = self.notifier.get() {
                    notify();
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

/// Engine-side owner of one exchange. Dropping it before the exchange
/// settles records why the request never finished.
pub(super) struct CaptureContext {
    pub(super) exchange: Exchange,
    pub(super) log: Option<FullLog>,
    stopped: Arc<AtomicBool>,
}

impl CaptureContext {
    /// Publishes a new in-flight exchange from request metadata.
    pub(super) fn start(
        method: &Method,
        headers: &HeaderMap,
        replay_of: Option<RecordId>,
        source: CaptureSource,
        target: &str,
        engine: &Engine,
    ) -> Self {
        let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        let log = engine.events.log.clone();
        if let Some(log) = log.as_ref() {
            log.request_head(id, method, target, headers);
        }
        let exchange = Exchange::new(
            RecordId::new(id),
            engine.session,
            source,
            CapturedRequest::new(
                method.clone(),
                bounded_text(target, TARGET_BYTES),
                capture_headers(headers),
            ),
        );
        if let Some(original) = replay_of {
            exchange.set_replay_of(original);
            if let Some(log) = log.as_ref() {
                log.replay_of(id, original);
            }
        }
        engine.events.send(exchange.clone());
        Self {
            exchange,
            log,
            stopped: Arc::clone(&engine.stopped),
        }
    }

    pub(super) fn id(&self) -> u64 {
        self.exchange.id().value()
    }

    pub(super) fn body_log(&self, side: BodySide) -> Option<BodyLog> {
        self.log
            .as_ref()
            .map(|log| BodyLog::new(log.clone(), self.id(), side))
    }

    /// Records a failure and returns the bounded message shown to the client.
    pub(super) fn fail(&self, stage: FailureStage, message: &str) -> String {
        if let Some(log) = self.log.as_ref() {
            log.failure(self.id(), stage, message);
        }
        let message = bounded_text(message, ERROR_BYTES);
        self.settle(Outcome::Failed {
            stage,
            message: message.clone(),
        });
        message
    }

    pub(super) fn settle(&self, outcome: Outcome) {
        let logged_outcome = self.log.as_ref().map(|log| (log, outcome.clone()));
        if self.exchange.settle(outcome)
            && let Some((log, outcome)) = logged_outcome
        {
            log.settled(self.id(), &outcome, self.exchange.elapsed());
        }
    }

    pub(super) fn tunnel_ended(&self, tunnel: &TunnelSnapshot) {
        if let Some(log) = self.log.as_ref() {
            log.tunnel_ended(self.id(), tunnel);
        }
    }
}

impl Drop for CaptureContext {
    fn drop(&mut self) {
        if self.exchange.outcome().is_in_flight() {
            // The engine sets `stopped` before shutting its runtime down, so
            // any request dropped after that was cut off by the stop rather
            // than abandoned by its client.
            if self.stopped.load(Ordering::Acquire) {
                self.settle(Outcome::Incomplete);
            } else {
                self.settle(Outcome::Cancelled);
            }
        }
    }
}
