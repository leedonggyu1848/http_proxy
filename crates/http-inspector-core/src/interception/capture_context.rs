use super::capture::{BodyLog, BodySinks};
use super::capture::{ERROR_BYTES, bounded_text, captured_request};
use super::handle::Engine;
use crate::recording::body_store::{BodyRecorder, BodyStore};
use crate::recording::journal::FullLog;
use http::Request;
#[cfg(test)]
use http::{HeaderMap, Method};
use http_inspector_exchange::BodySide;
use http_inspector_exchange::{
    CaptureSource, ClientConnection, Exchange, ExchangeWriter, FailureStage, Outcome, RecordId,
    TunnelSnapshot, TunnelWriter,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

/// Callback run after an exchange was queued, for example to wake a UI.
pub(super) type EventNotifier = Box<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(super) struct EventSink {
    pub(super) sender: mpsc::Sender<Exchange>,
    pub(super) dropped: Arc<AtomicU64>,
    pub(super) notifier: Arc<OnceLock<EventNotifier>>,
    pub(super) log: Option<FullLog>,
    pub(super) body_store: Option<BodyStore>,
}

impl EventSink {
    fn notify(&self) {
        if let Some(notify) = self.notifier.get() {
            notify();
        }
    }

    fn send(&self, exchange: Exchange) {
        match self.sender.try_send(exchange) {
            Ok(()) => self.notify(),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

/// Engine-side owner of one exchange and its only writer. Dropping it
/// before the exchange settles records why the request never finished.
pub(super) struct CaptureContext {
    pub(super) writer: ExchangeWriter,
    pub(super) log: Option<FullLog>,
    body_store: Option<BodyStore>,
    stopped: Arc<AtomicBool>,
}

impl CaptureContext {
    /// Publishes a new in-flight exchange from request metadata.
    pub(super) fn start<B>(
        request: &Request<B>,
        source: CaptureSource,
        target: &str,
        engine: &Engine,
    ) -> Self {
        let record = RecordId::next();
        let id = record.value();
        let method = request.method();
        let headers = request.headers();
        let log = engine.events.log.clone();
        let connection = request.extensions().get::<ClientConnection>();
        if let Some(log) = log.as_ref() {
            log.request_head(
                id,
                method,
                target,
                headers,
                connection.map(ClientConnection::id),
            );
        }
        let writer = ExchangeWriter::start(
            record,
            engine.session,
            source,
            captured_request(method, target, headers),
        );
        if let Some(connection) = connection {
            writer.attach_connection(connection.clone());
        }
        if let Some(original) = request.extensions().get::<RecordId>().copied() {
            writer.set_replay_of(original);
            if let Some(log) = log.as_ref() {
                log.replay_of(id, original);
            }
        }
        engine.events.send(writer.exchange().clone());
        Self {
            writer,
            log,
            body_store: engine.events.body_store.clone(),
            stopped: Arc::clone(&engine.stopped),
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(log: Option<FullLog>, stopped: Arc<AtomicBool>) -> Self {
        let writer = ExchangeWriter::start(
            RecordId::next(),
            http_inspector_exchange::SessionId::next(),
            CaptureSource::Connect,
            captured_request(&Method::CONNECT, "example.invalid:443", &HeaderMap::new()),
        );
        Self {
            writer,
            log,
            body_store: None,
            stopped,
        }
    }

    pub(super) fn id(&self) -> u64 {
        self.writer.exchange().id().value()
    }

    /// Reports whether the proxy engine has already begun shutting down.
    pub(super) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    /// Returns the full-log and body-store writers for one body.
    pub(super) fn body_sinks(&self, side: BodySide) -> BodySinks {
        BodySinks {
            log: self
                .log
                .as_ref()
                .map(|log| BodyLog::new(log.clone(), self.id(), side)),
            store: self
                .body_store
                .as_ref()
                .map(|store| BodyRecorder::new(store.clone(), self.id(), side)),
        }
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
        if self.writer.settle(outcome)
            && let Some((log, outcome)) = logged_outcome
        {
            log.settled(self.id(), &outcome, self.writer.exchange().elapsed());
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
        if self.writer.exchange().outcome().is_in_flight() {
            // The engine sets `stopped` before shutting its runtime down, so
            // any request dropped after that was cut off by the stop rather
            // than abandoned by its client.
            if self.is_stopped() {
                self.settle(Outcome::Incomplete);
            } else {
                self.settle(Outcome::Cancelled);
            }
        }
    }
}

/// Owns an established opaque `CONNECT` tunnel's writer and capture context.
///
/// Constructing this guard before `tokio::spawn` guarantees that dropping the
/// relay task at any point — including before its first poll — marks an
/// unfinished tunnel `Incomplete` and writes its final snapshot to the session
/// journal once.
pub(super) struct TunnelCapture {
    tunnel: TunnelWriter,
    capture: CaptureContext,
}

impl TunnelCapture {
    /// Creates a tunnel writer, settles the exchange as `Outcome::Tunnel`, and
    /// returns the guard that owns both until the tunnel ends.
    pub(super) fn start(capture: CaptureContext) -> Self {
        let tunnel = TunnelWriter::new();
        capture.settle(Outcome::Tunnel(tunnel.preview()));
        Self { tunnel, capture }
    }

    pub(super) const fn writer(&self) -> &TunnelWriter {
        &self.tunnel
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.capture.is_stopped()
    }

    #[cfg(test)]
    pub(super) fn preview(&self) -> http_inspector_exchange::TunnelPreview {
        self.tunnel.preview()
    }
}

impl Drop for TunnelCapture {
    fn drop(&mut self) {
        self.tunnel.abandon();
        self.capture.tunnel_ended(&self.tunnel.preview().snapshot());
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::{CaptureContext, TunnelCapture};
    use crate::recording::journal::FullLog;
    use http_inspector_exchange::{Outcome, TunnelState};

    fn export_log(log: &FullLog) -> io::Result<String> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("capture.log");
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        std::fs::read_to_string(path)
    }

    #[test]
    fn dropping_unsettled_capture_distinguishes_stop_from_client_cancel() {
        let running = CaptureContext::for_test(None, Arc::new(AtomicBool::new(false)));
        let cancelled = running.writer.exchange().clone();
        drop(running);
        assert!(matches!(cancelled.outcome(), Outcome::Cancelled));

        let stopped = CaptureContext::for_test(None, Arc::new(AtomicBool::new(true)));
        let incomplete = stopped.writer.exchange().clone();
        drop(stopped);
        assert!(matches!(incomplete.outcome(), Outcome::Incomplete));
    }

    #[test]
    fn dropping_tunnel_capture_abandons_open_tunnel_and_logs_end_once() -> io::Result<()> {
        let log = FullLog::new()?;
        let capture = CaptureContext::for_test(Some(log.clone()), Arc::new(AtomicBool::new(false)));
        let id = capture.id();
        let tunnel = TunnelCapture::start(capture);
        let preview = tunnel.preview();
        assert_eq!(preview.snapshot().state(), &TunnelState::Open);

        drop(tunnel);
        assert_eq!(preview.snapshot().state(), &TunnelState::Incomplete);

        let journal = export_log(&log)?;
        let ended_lines: Vec<_> = journal
            .lines()
            .filter(|line| line.starts_with(&format!("[{id}] Tunnel ended ")))
            .collect();
        assert_eq!(ended_lines.len(), 1);
        assert!(
            ended_lines
                .first()
                .is_some_and(|line| line.contains("Tunnel ended Incomplete"))
        );
        Ok(())
    }

    #[test]
    fn dropping_closed_tunnel_capture_keeps_closed_state_and_logs_once() -> io::Result<()> {
        let log = FullLog::new()?;
        let capture = CaptureContext::for_test(Some(log.clone()), Arc::new(AtomicBool::new(false)));
        let id = capture.id();
        let tunnel = TunnelCapture::start(capture);
        let preview = tunnel.preview();
        tunnel.writer().close();
        drop(tunnel);

        assert_eq!(preview.snapshot().state(), &TunnelState::Closed);
        let journal = export_log(&log)?;
        let ended_lines: Vec<_> = journal
            .lines()
            .filter(|line| line.starts_with(&format!("[{id}] Tunnel ended ")))
            .collect();
        assert_eq!(ended_lines.len(), 1);
        assert!(
            ended_lines
                .first()
                .is_some_and(|line| line.contains("Tunnel ended Closed"))
        );
        Ok(())
    }
}
