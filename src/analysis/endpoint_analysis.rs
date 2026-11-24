//! Owns endpoint schema aggregation for the Map view: queues captured
//! exchanges to the background analyzer and keeps the merged results.

use std::sync::Arc;

use crate::analysis::endpoint_schema::{EndpointKey, EndpointSchemas, SchemaAnalyzer};
use crate::analysis::url_map::NodeId;
use crate::{BodySource, CaptureSource, Exchange};

/// Coordinates background endpoint schema analysis and retains merged
/// per-endpoint aggregates.
#[derive(Debug)]
pub struct EndpointAnalysis {
    schemas: EndpointSchemas,
    analyzer: Option<SchemaAnalyzer>,
}

impl EndpointAnalysis {
    /// Starts the analysis thread, reading structured bodies from `bodies` and
    /// calling `wake` after each finished exchange. Without a thread the Map
    /// shows no endpoint structure but keeps working.
    #[must_use]
    pub fn start(bodies: Arc<dyn BodySource>, wake: impl Fn() + Send + 'static) -> Self {
        let analyzer = SchemaAnalyzer::spawn(bodies, wake);
        if let Err(error) = &analyzer {
            tracing::warn!("endpoint schema analysis is unavailable: {error}");
        }
        Self {
            schemas: EndpointSchemas::new(),
            analyzer: analyzer.ok(),
        }
    }

    /// Creates an inactive analyzer that does not spawn a background thread.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            schemas: EndpointSchemas::new(),
            analyzer: None,
        }
    }

    /// Queues `exchange`, which the URL map placed on `node`. `CONNECT`
    /// tunnels have no path or body structure and are skipped.
    pub fn submit(&mut self, node: NodeId, exchange: &Exchange) {
        if exchange.source() == CaptureSource::Connect {
            return;
        }
        if let Some(analyzer) = self.analyzer.as_mut() {
            let key = EndpointKey {
                node,
                method: exchange.request().method().clone(),
            };
            analyzer.submit(key, exchange.clone());
        }
    }

    /// Merges finished observations without waiting.
    pub fn update(&mut self) {
        if let Some(analyzer) = self.analyzer.as_mut() {
            let _changed = analyzer.drain_into(&mut self.schemas);
        }
    }

    /// Drops every aggregate and pending exchange, as on `Clear`.
    pub fn clear(&mut self) {
        if let Some(analyzer) = self.analyzer.as_mut() {
            analyzer.reset();
        }
        self.schemas.clear();
    }

    /// Returns the merged endpoint schema aggregates.
    #[must_use]
    pub const fn schemas(&self) -> &EndpointSchemas {
        &self.schemas
    }
}
