//! Capture event pipeline that hands captured exchanges to every analyzer.
//!
//! The UI feeds one [`AnalysisPipeline`] with capture events and only reads
//! analysis results back, so adding an analyzer or changing what it receives
//! does not touch the UI.
//!
//! Analysis runs in two explicit stages:
//!
//! 1. **Placement.** The [`UrlMap`] places each new record on a path node.
//!    Every later stage depends on this, so it always runs first and its
//!    [`NodeId`] travels with the event.
//! 2. **Analyzers.** Endpoint schema analysis, then every analyzer added with
//!    [`AnalysisPipeline::register`], in registration order, receive the same
//!    [`CaptureEvent`] through the [`Analyzer`] trait.
//!
//! Background analyzers report results through [`Analyzer::poll`], which
//! [`AnalysisPipeline::update`] calls once per frame, and request a repaint
//! through the pipeline's shared [`Wake`] callback.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use crate::endpoint_analysis::EndpointAnalysis;
use crate::endpoint_schema::EndpointSchemas;
use crate::url_map::{NodeId, UrlMap};
use http_inspector_exchange::{BodySource, Exchange};

/// Most in-flight records watched for [`CaptureEvent::RecordSettled`].
///
/// A record that is still in flight when it is added while this many are
/// already watched never receives a settled event; it is counted in
/// [`AnalysisPipeline::unwatched_records`] instead. Records that already
/// finished when added are never affected. This is the same rule the URL map
/// applies to its live tracking (`MAX_PENDING_EXCHANGES`, also 4,096).
pub const MAX_WATCHED_RECORDS: usize = 4_096;

/// Callback shared by background analyzers to ask the UI to repaint after a
/// result arrives.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// One change in the captured session, delivered to every [`Analyzer`].
#[derive(Clone, Copy, Debug)]
pub enum CaptureEvent<'a> {
    /// A newly captured record, delivered once. It may still be in flight.
    RecordAdded {
        /// The captured record.
        exchange: &'a Exchange,
        /// URL map node the placement stage put the record on, or `None` when
        /// its target could not be placed.
        node: Option<NodeId>,
    },
    /// A record whose response and bodies stopped changing, delivered at most
    /// once after its [`CaptureEvent::RecordAdded`].
    ///
    /// A record that finished before it was added gets this event right
    /// after `RecordAdded`. A record still in flight gets it only if the
    /// pipeline could watch it: when [`MAX_WATCHED_RECORDS`] in-flight records
    /// are already watched, it gets none and is counted in
    /// [`AnalysisPipeline::unwatched_records`]. A record evicted or cleared
    /// while still in flight gets no settled event.
    RecordSettled {
        /// The settled record.
        exchange: &'a Exchange,
        /// URL map node the record was placed on, as in `RecordAdded`.
        node: Option<NodeId>,
    },
    /// The session was cleared; every aggregate must be dropped.
    SessionCleared,
}

/// Analysis that consumes capture events and keeps its own results.
///
/// Implementations are read back with [`AnalysisPipeline::analyzer`].
///
/// Every record is delivered as [`CaptureEvent::RecordAdded`], but not every
/// record is delivered as [`CaptureEvent::RecordSettled`] (see its docs and
/// [`MAX_WATCHED_RECORDS`]). An analyzer that needs every record should start
/// from `RecordAdded`, and one that counts settled records can report
/// [`AnalysisPipeline::unwatched_records`] as possibly missing.
pub trait Analyzer: Any {
    /// Applies one capture event.
    fn handle(&mut self, event: &CaptureEvent<'_>);

    /// Collects results finished in the background without waiting.
    /// Returns `true` when any result changed.
    fn poll(&mut self) -> bool {
        false
    }
}

impl Analyzer for EndpointAnalysis {
    fn handle(&mut self, event: &CaptureEvent<'_>) {
        match *event {
            CaptureEvent::RecordSettled {
                exchange,
                node: Some(node),
            } => self.submit(node, exchange),
            CaptureEvent::RecordAdded { .. } | CaptureEvent::RecordSettled { node: None, .. } => {}
            CaptureEvent::SessionCleared => self.clear(),
        }
    }

    fn poll(&mut self) -> bool {
        self.update()
    }
}

/// A record still in flight, kept until it settles.
#[derive(Debug)]
struct WatchedRecord {
    exchange: Exchange,
    node: Option<NodeId>,
}

/// Delivers capture events to the URL map and every analyzer in order.
pub struct AnalysisPipeline {
    url_map: UrlMap,
    endpoints: EndpointAnalysis,
    analyzers: Vec<Box<dyn Analyzer>>,
    watched: Vec<WatchedRecord>,
    unwatched: u64,
    wake: Wake,
}

impl fmt::Debug for AnalysisPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalysisPipeline")
            .field("url_map_nodes", &self.url_map.node_count())
            .field("endpoints", &self.endpoints)
            .field("analyzers", &self.analyzers.len())
            .field("watched", &self.watched.len())
            .field("unwatched", &self.unwatched)
            .finish_non_exhaustive()
    }
}

impl AnalysisPipeline {
    /// Starts every built-in analyzer. Endpoint schema analysis reads
    /// structured bodies from `bodies` on a background thread; `wake` is
    /// called whenever a background result is ready to be collected by
    /// [`AnalysisPipeline::update`].
    #[must_use]
    pub fn start(bodies: Arc<dyn BodySource>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let wake: Wake = Arc::new(wake);
        let endpoint_wake = Arc::clone(&wake);
        let endpoints = EndpointAnalysis::start(bodies, move || endpoint_wake());
        Self::with_parts(UrlMap::new(), endpoints, wake)
    }

    /// Creates a pipeline without background analysis threads. The URL map
    /// still works; endpoint schemas stay empty.
    #[must_use]
    pub fn disabled() -> Self {
        Self::seeded(UrlMap::new())
    }

    /// Creates a pipeline without background analysis over an already
    /// populated `url_map`, as for synthetic workloads in tests and
    /// benchmarks.
    #[must_use]
    pub fn seeded(url_map: UrlMap) -> Self {
        Self::with_parts(url_map, EndpointAnalysis::disabled(), Arc::new(|| {}))
    }

    fn with_parts(url_map: UrlMap, endpoints: EndpointAnalysis, wake: Wake) -> Self {
        Self {
            url_map,
            endpoints,
            analyzers: Vec::new(),
            watched: Vec::new(),
            unwatched: 0,
            wake,
        }
    }

    /// Returns the repaint callback to hand to a background analyzer before
    /// registering it.
    #[must_use]
    pub fn waker(&self) -> Wake {
        Arc::clone(&self.wake)
    }

    /// Adds `analyzer` after every analyzer already registered. It receives
    /// only events delivered from now on.
    pub fn register(&mut self, analyzer: Box<dyn Analyzer>) {
        self.analyzers.push(analyzer);
    }

    /// Delivers a newly captured record: the URL map places it first, then
    /// every analyzer receives it with that placement. A record that already
    /// finished is also delivered as settled right away. A record still in
    /// flight is watched for settlement unless [`MAX_WATCHED_RECORDS`] records
    /// are already watched, in which case it is counted as unwatched.
    pub fn record_added(&mut self, exchange: &Exchange) {
        let node = self.url_map.observe_exchange(exchange);
        self.dispatch(&CaptureEvent::RecordAdded { exchange, node });
        if !exchange.is_live() {
            self.dispatch(&CaptureEvent::RecordSettled { exchange, node });
        } else if self.watched.len() < MAX_WATCHED_RECORDS {
            self.watched.push(WatchedRecord {
                exchange: exchange.clone(),
                node,
            });
        } else {
            self.unwatched = self.unwatched.saturating_add(1);
            self.endpoints.note_unwatched(node, exchange);
        }
    }

    /// Stops watching an evicted record. If it has already settled, delivers
    /// its final state before releasing it.
    pub fn record_evicted(&mut self, exchange: &Exchange) {
        let Some(index) = self
            .watched
            .iter()
            .position(|record| record.exchange.id() == exchange.id())
        else {
            return;
        };
        let record = self.watched.remove(index);
        if record.exchange.is_live() {
            self.endpoints.note_unwatched(record.node, &record.exchange);
        } else {
            self.dispatch(&CaptureEvent::RecordSettled {
                exchange: &record.exchange,
                node: record.node,
            });
        }
    }

    /// Drops every aggregate in the URL map and every analyzer, as on `Clear`.
    pub fn session_cleared(&mut self) {
        self.watched.clear();
        self.unwatched = 0;
        self.url_map.clear();
        self.dispatch(&CaptureEvent::SessionCleared);
    }

    /// Applies live progress without waiting: updates the URL map with
    /// streamed responses, delivers records that settled since the last call,
    /// and collects background results. Returns `true` when the URL map
    /// changed, any settled event was delivered, or any analyzer reported
    /// changed results; a delivered event does not mean an analyzer's results
    /// changed.
    pub fn update(&mut self) -> bool {
        let mut changed = self.url_map.sync_live();
        let watched = std::mem::take(&mut self.watched);
        for record in watched {
            if record.exchange.is_live() {
                self.watched.push(record);
            } else {
                self.dispatch(&CaptureEvent::RecordSettled {
                    exchange: &record.exchange,
                    node: record.node,
                });
                changed = true;
            }
        }
        changed |= self.endpoints.poll();
        for analyzer in &mut self.analyzers {
            changed |= analyzer.poll();
        }
        changed
    }

    /// In-flight records added since the last clear that were not watched
    /// because [`MAX_WATCHED_RECORDS`] were already watched, so they never
    /// receive [`CaptureEvent::RecordSettled`].
    #[must_use]
    pub const fn unwatched_records(&self) -> u64 {
        self.unwatched
    }

    /// Returns the URL map built by the placement stage.
    #[must_use]
    pub const fn url_map(&self) -> &UrlMap {
        &self.url_map
    }

    #[must_use]
    pub const fn endpoint_schemas(&self) -> &EndpointSchemas {
        self.endpoints.schemas()
    }

    /// Returns the first registered analyzer of type `A`.
    #[must_use]
    pub fn analyzer<A: Analyzer>(&self) -> Option<&A> {
        self.analyzers
            .iter()
            .find_map(|analyzer| (analyzer.as_ref() as &dyn Any).downcast_ref::<A>())
    }

    /// Delivers `event` to the analyzer stage in order: endpoint schemas
    /// first, then registered analyzers in registration order.
    fn dispatch(&mut self, event: &CaptureEvent<'_>) {
        self.endpoints.handle(event);
        for analyzer in &mut self.analyzers {
            analyzer.handle(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use http::header::CONTENT_TYPE;
    use http::{HeaderValue, Method, StatusCode};

    use super::*;
    use http_inspector_exchange::{
        BodyLifecycle, BodySide, CaptureSource, CapturedRequest, ExchangeWriter, HeaderPreview,
        MemoryBodySource, Outcome, RecordId, SessionId,
    };
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    /// Test analyzer that counts records per event and is registered without
    /// touching the UI.
    #[derive(Debug, Default)]
    struct CountingAnalyzer {
        added: Vec<(RecordId, Option<NodeId>)>,
        settled: Vec<RecordId>,
        clears: usize,
        polls: usize,
    }

    impl Analyzer for CountingAnalyzer {
        fn handle(&mut self, event: &CaptureEvent<'_>) {
            match *event {
                CaptureEvent::RecordAdded { exchange, node } => {
                    self.added.push((exchange.id(), node));
                }
                CaptureEvent::RecordSettled { exchange, .. } => self.settled.push(exchange.id()),
                CaptureEvent::SessionCleared => {
                    self.added.clear();
                    self.settled.clear();
                    self.clears += 1;
                }
            }
        }

        fn poll(&mut self) -> bool {
            self.polls += 1;
            false
        }
    }

    fn finished_exchange(path: &str) -> Result<Exchange, Box<dyn Error>> {
        Ok(
            ExchangeBuilder::http(Method::GET, format!("http://api.example.test{path}"))?.respond(
                ResponseFixture::new(StatusCode::OK).body("ok", BodyLifecycle::Complete),
            )?,
        )
    }

    fn in_flight_writer() -> ExchangeWriter {
        ExchangeWriter::start(
            RecordId::next(),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(
                Method::GET,
                "http://api.example.test/slow".to_owned(),
                HeaderPreview::default(),
                Vec::new(),
            ),
        )
    }

    #[test]
    fn registered_analyzer_receives_events_with_url_map_placement() -> Result<(), Box<dyn Error>> {
        let mut pipeline = AnalysisPipeline::disabled();
        pipeline.register(Box::new(CountingAnalyzer::default()));

        let exchange = finished_exchange("/users/42")?;
        pipeline.record_added(&exchange);
        let _changed = pipeline.update();

        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("analyzer not registered")?;
        let placed = pipeline
            .url_map()
            .matching_node_ids(&crate::url_map::ViewNodeKey {
                origin: crate::url::OriginKey::new("http", "api.example.test", None),
                path: ["users", "42"]
                    .into_iter()
                    .map(|segment| crate::url::PathSegmentPattern::Literal(segment.to_owned()))
                    .collect(),
            });
        assert_eq!(placed.len(), 1);
        assert_eq!(
            counter.added,
            vec![(exchange.id(), placed.first().copied())]
        );
        assert_eq!(counter.settled, vec![exchange.id()]);
        assert_eq!(counter.polls, 1);
        Ok(())
    }

    #[test]
    fn in_flight_record_is_delivered_as_settled_once_it_finishes() {
        let mut pipeline = AnalysisPipeline::disabled();
        pipeline.register(Box::new(CountingAnalyzer::default()));
        let writer = in_flight_writer();
        let id = writer.exchange().id();

        pipeline.record_added(writer.exchange());
        let _changed = pipeline.update();
        let settled = |pipeline: &AnalysisPipeline| {
            pipeline
                .analyzer::<CountingAnalyzer>()
                .map(|counter| counter.settled.clone())
        };
        assert_eq!(settled(&pipeline), Some(Vec::new()));

        assert!(writer.settle(Outcome::Cancelled));
        assert!(pipeline.update());
        assert_eq!(settled(&pipeline), Some(vec![id]));
        assert!(!pipeline.update());
        assert_eq!(settled(&pipeline), Some(vec![id]));
    }

    #[test]
    fn eviction_releases_live_requests_and_delivers_already_settled_requests()
    -> Result<(), Box<dyn Error>> {
        let mut pipeline = AnalysisPipeline::start(Arc::new(MemoryBodySource::new()), || {});
        pipeline.register(Box::new(CountingAnalyzer::default()));
        let live = in_flight_writer();
        let settled = in_flight_writer();
        pipeline.record_added(live.exchange());
        pipeline.record_added(settled.exchange());

        assert!(settled.settle(Outcome::Cancelled));
        pipeline.record_evicted(live.exchange());
        pipeline.record_evicted(settled.exchange());

        assert!(pipeline.watched.is_empty());
        assert_eq!(pipeline.endpoint_schemas().skipped_requests(), 1);
        assert!(live.settle(Outcome::Cancelled));
        let _changed = pipeline.update();
        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("missing analyzer")?;
        assert_eq!(counter.settled, vec![settled.exchange().id()]);
        Ok(())
    }

    #[test]
    fn records_beyond_the_watch_limit_get_no_settled_event() -> Result<(), Box<dyn Error>> {
        let mut pipeline = AnalysisPipeline::disabled();
        pipeline.register(Box::new(CountingAnalyzer::default()));
        let watched: Vec<ExchangeWriter> = (0..MAX_WATCHED_RECORDS)
            .map(|_| in_flight_writer())
            .collect();
        for writer in &watched {
            pipeline.record_added(writer.exchange());
        }
        assert_eq!(pipeline.unwatched_records(), 0);

        let overflow = in_flight_writer();
        pipeline.record_added(overflow.exchange());
        assert_eq!(pipeline.unwatched_records(), 1);

        let finished = finished_exchange("/done")?;
        pipeline.record_added(&finished);
        let settled = |pipeline: &AnalysisPipeline| {
            pipeline
                .analyzer::<CountingAnalyzer>()
                .map(|counter| counter.settled.clone())
        };
        assert_eq!(settled(&pipeline), Some(vec![finished.id()]));

        assert!(overflow.settle(Outcome::Cancelled));
        let _changed = pipeline.update();
        assert_eq!(settled(&pipeline), Some(vec![finished.id()]));
        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("analyzer not registered")?;
        assert_eq!(counter.added.len(), MAX_WATCHED_RECORDS + 2);

        pipeline.session_cleared();
        assert_eq!(pipeline.unwatched_records(), 0);
        Ok(())
    }

    #[test]
    fn analyzers_run_in_registration_order() -> Result<(), Box<dyn Error>> {
        #[derive(Debug)]
        struct Ordered {
            label: &'static str,
            log: Arc<std::sync::Mutex<Vec<&'static str>>>,
        }
        impl Analyzer for Ordered {
            fn handle(&mut self, _event: &CaptureEvent<'_>) {
                if let Ok(mut log) = self.log.lock() {
                    log.push(self.label);
                }
            }
        }

        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut pipeline = AnalysisPipeline::disabled();
        for label in ["first", "second"] {
            pipeline.register(Box::new(Ordered {
                label,
                log: Arc::clone(&log),
            }));
        }
        pipeline.record_added(&finished_exchange("/")?);
        let order = log.lock().map_err(|_| "poisoned")?.clone();
        assert_eq!(order, ["first", "second", "first", "second"]);
        Ok(())
    }

    #[test]
    fn clear_resets_url_map_endpoint_schemas_and_every_analyzer() -> Result<(), Box<dyn Error>> {
        let exchange = ExchangeBuilder::http(Method::POST, "http://api.example.test/users")?
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .request_body(r#"{"name":"SECRET"}"#, BodyLifecycle::Complete)
            .respond(ResponseFixture::new(StatusCode::CREATED))?;
        let bodies = Arc::new(MemoryBodySource::new().with_body(
            exchange.id(),
            BodySide::Request,
            r#"{"name":"SECRET"}"#,
        ));
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut pipeline = AnalysisPipeline::start(bodies, move || {
            let _ = wake_tx.send(());
        });
        pipeline.register(Box::new(CountingAnalyzer::default()));
        let writer = in_flight_writer();

        pipeline.record_added(&exchange);
        pipeline.record_added(writer.exchange());
        let deadline = Instant::now() + Duration::from_secs(5);
        while pipeline.endpoint_schemas().version() == 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("timed out waiting for endpoint schema wake")?;
            wake_rx.recv_timeout(remaining)?;
            let _changed = pipeline.update();
        }
        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("analyzer not registered")?;
        assert_eq!(counter.added.len(), 2);
        let node = counter
            .added
            .first()
            .and_then(|(_, node)| *node)
            .ok_or("record not placed")?;
        assert_eq!(pipeline.endpoint_schemas().merged(&[node]).len(), 1);
        assert!(pipeline.url_map().node_count() > 0);

        pipeline.session_cleared();

        assert_eq!(pipeline.url_map().node_count(), 0);
        assert_eq!(pipeline.endpoint_schemas().merged(&[node]).len(), 0);
        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("analyzer not registered")?;
        assert_eq!(counter.clears, 1);
        assert_eq!(counter.added.len(), 0);
        assert_eq!(counter.settled.len(), 0);

        // The in-flight record belonged to the cleared session, so finishing
        // it later delivers nothing.
        assert!(writer.settle(Outcome::Cancelled));
        let _changed = pipeline.update();
        let counter = pipeline
            .analyzer::<CountingAnalyzer>()
            .ok_or("analyzer not registered")?;
        assert_eq!(counter.settled.len(), 0);
        Ok(())
    }
}
