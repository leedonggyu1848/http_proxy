//! Owns endpoint schema aggregation for the Map view: queues captured
//! exchanges to the background analyzer and keeps the merged results.

use eframe::egui;
use std::sync::Arc;

use http_proxy::endpoint_schema::{EndpointKey, EndpointSchemas, SchemaAnalyzer};
use http_proxy::url_map::NodeId;
use http_proxy::{BodySource, BodyStore, CaptureSource, Exchange, PreviewBodies};

pub(in crate::ui) struct EndpointAnalysis {
    schemas: EndpointSchemas,
    analyzer: Option<SchemaAnalyzer>,
}

impl EndpointAnalysis {
    /// Starts the analysis thread, reading bodies from `store` when the
    /// preview did not keep them. Without a thread the Map shows no
    /// endpoint structure but keeps working.
    pub(in crate::ui) fn start(store: Option<&BodyStore>, context: &egui::Context) -> Self {
        let context = context.clone();
        let bodies: Arc<dyn BodySource> = match store {
            Some(store) => Arc::new(store.clone()),
            None => Arc::new(PreviewBodies),
        };
        let analyzer = SchemaAnalyzer::spawn(bodies, move || {
            context.request_repaint();
        });
        if let Err(error) = &analyzer {
            tracing::warn!("endpoint schema analysis is unavailable: {error}");
        }
        Self {
            schemas: EndpointSchemas::new(),
            analyzer: analyzer.ok(),
        }
    }

    /// Queues `exchange`, which the URL map placed on `node`. `CONNECT`
    /// tunnels have no path or body structure and are skipped.
    pub(in crate::ui) fn submit(&mut self, node: NodeId, exchange: &Exchange) {
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
    pub(in crate::ui) fn update(&mut self) {
        if let Some(analyzer) = self.analyzer.as_mut() {
            let _changed = analyzer.drain_into(&mut self.schemas);
        }
    }

    /// Drops every aggregate and pending exchange, as on `Clear`.
    pub(in crate::ui) fn clear(&mut self) {
        if let Some(analyzer) = self.analyzer.as_mut() {
            analyzer.reset();
        }
        self.schemas.clear();
    }

    pub(in crate::ui) const fn schemas(&self) -> &EndpointSchemas {
        &self.schemas
    }

    #[cfg(test)]
    pub(in crate::ui) fn disabled() -> Self {
        Self {
            schemas: EndpointSchemas::new(),
            analyzer: None,
        }
    }
}
