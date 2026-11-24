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

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use http::header::CONTENT_TYPE;
    use http::{HeaderValue, Method, StatusCode};

    use super::*;
    use crate::analysis::url_map::UrlMap;
    use crate::{BodyLifecycle, BodySide, ExchangeBuilder, MemoryBodySource, ResponseFixture};

    #[test]
    fn analyzes_exchanges_with_body_source_and_wake_callback() -> Result<(), Box<dyn Error>> {
        let exchange = ExchangeBuilder::http(
            Method::POST,
            "http://api.example.test/users/42?token=SECRET_Q",
        )?
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .request_body(r#"{"name":"SECRET_REQ"}"#, BodyLifecycle::Complete)
        .respond(
            ResponseFixture::new(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(r#"{"id":42}"#, BodyLifecycle::Complete),
        )?;
        let tunnel = ExchangeBuilder::connect("api.example.test:443").tunnel(
            4,
            5,
            crate::TunnelState::Closed,
        )?;

        let mut map = UrlMap::new();
        let node = map
            .observe_exchange(&exchange)
            .ok_or("expected HTTP node")?;
        let tunnel_node = map
            .observe_exchange(&tunnel)
            .ok_or("expected tunnel node")?;

        let bodies = Arc::new(
            MemoryBodySource::new()
                .with_body(exchange.id(), BodySide::Request, r#"{"name":"SECRET_REQ"}"#)
                .with_body(exchange.id(), BodySide::Response, r#"{"id":42}"#),
        );
        let (wake_tx, wake_rx) = mpsc::channel();
        let mut analysis = EndpointAnalysis::start(bodies, move || {
            let _ = wake_tx.send(());
        });

        analysis.submit(tunnel_node, &tunnel);
        analysis.submit(node, &exchange);

        let deadline = Instant::now() + Duration::from_secs(5);
        while analysis.schemas().version() == 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("timed out waiting for endpoint schema wake")?;
            wake_rx.recv_timeout(remaining)?;
            analysis.update();
        }

        assert!(analysis.schemas().version() >= 1);
        assert_eq!(analysis.schemas().merged(&[tunnel_node]).len(), 0);
        let merged = analysis.schemas().merged(&[node]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.first().map(|(m, _)| m.as_str()), Some("POST"));

        analysis.clear();
        assert_eq!(analysis.schemas().merged(&[node]).len(), 0);
        Ok(())
    }
}
