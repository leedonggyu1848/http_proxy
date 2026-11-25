//! Endpoint aggregates for every URL map node, kept until `Clear`.

use std::collections::{BTreeMap, HashMap};

use http::Method;

use super::endpoint::{EndpointObservation, EndpointSchema};
use crate::url_map::NodeId;

/// Most endpoints (node and method pairs) aggregated in one session.
pub const MAX_ENDPOINTS: usize = 20_000;

/// Identifies one endpoint: a raw URL map node and a request method.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EndpointKey {
    /// Raw node the request path landed on.
    pub node: NodeId,
    /// Request method.
    pub method: Method,
}

/// Cumulative endpoint aggregates. Evicting requests from the request list
/// does not remove anything; only [`EndpointSchemas::clear`] does.
#[derive(Debug, Default)]
pub struct EndpointSchemas {
    by_node: HashMap<NodeId, BTreeMap<String, EndpointSchema>>,
    endpoints: usize,
    omitted_endpoints: u64,
    skipped_requests: u64,
    version: u64,
}

impl EndpointSchemas {
    /// Creates an empty set of aggregates.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one observed exchange to the endpoint `key`.
    pub fn record(&mut self, key: &EndpointKey, observation: &EndpointObservation) {
        let method = key.method.as_str();
        if let Some(schema) = self
            .by_node
            .get_mut(&key.node)
            .and_then(|methods| methods.get_mut(method))
        {
            schema.record(observation);
        } else if self.endpoints < MAX_ENDPOINTS {
            let mut schema = EndpointSchema::default();
            schema.record(observation);
            let _ = self
                .by_node
                .entry(key.node)
                .or_default()
                .insert(method.to_owned(), schema);
            self.endpoints += 1;
        } else {
            self.omitted_endpoints = self.omitted_endpoints.saturating_add(1);
        }
        self.bump_version();
    }

    /// Counts requests that were never analyzed, such as when the analysis
    /// queue was full.
    pub const fn note_skipped(&mut self, count: u64) {
        if count > 0 {
            self.skipped_requests = self.skipped_requests.saturating_add(count);
            self.bump_version();
        }
    }

    /// Requests that were never analyzed since the last clear.
    #[must_use]
    pub const fn skipped_requests(&self) -> u64 {
        self.skipped_requests
    }

    /// Requests not aggregated because [`MAX_ENDPOINTS`] was reached.
    #[must_use]
    pub const fn omitted_endpoints(&self) -> u64 {
        self.omitted_endpoints
    }

    /// Removes every aggregate.
    pub fn clear(&mut self) {
        self.by_node.clear();
        self.endpoints = 0;
        self.omitted_endpoints = 0;
        self.skipped_requests = 0;
        self.bump_version();
    }

    /// Monotonic counter incremented on every change.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Merges the aggregates of `nodes` (for example every raw node folded
    /// into one `{id}` row) per method, ordered by [`method_rank`].
    #[must_use]
    pub fn merged(&self, nodes: &[NodeId]) -> Vec<(String, EndpointSchema)> {
        let mut merged: BTreeMap<String, EndpointSchema> = BTreeMap::new();
        for node in nodes {
            let Some(methods) = self.by_node.get(node) else {
                continue;
            };
            for (method, schema) in methods {
                merged.entry(method.clone()).or_default().merge(schema);
            }
        }
        let mut ordered: Vec<(String, EndpointSchema)> = merged.into_iter().collect();
        ordered.sort_by(|left, right| {
            method_rank(&left.0)
                .cmp(&method_rank(&right.0))
                .then_with(|| left.0.cmp(&right.0))
        });
        ordered
    }

    /// Requests aggregated for `method` across `nodes`.
    #[must_use]
    pub fn request_count(&self, nodes: &[NodeId], method: &Method) -> u64 {
        nodes
            .iter()
            .filter_map(|node| self.by_node.get(node)?.get(method.as_str()))
            .fold(0, |sum, schema| sum.saturating_add(schema.requests()))
    }

    const fn bump_version(&mut self) {
        self.version = self.version.saturating_add(1);
    }
}

/// Display order of methods: common API verbs first, others alphabetically after.
#[must_use]
pub fn method_rank(method: &str) -> usize {
    const ORDER: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    ORDER
        .iter()
        .position(|known| *known == method)
        .unwrap_or(ORDER.len())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http::StatusCode;
    use http_inspector_exchange::{ExchangeBuilder, MemoryBodySource, ResponseFixture};

    use super::*;
    use crate::endpoint_schema::observe_exchange;
    use crate::url_map::UrlMap;

    #[test]
    fn endpoint_cap_does_not_create_empty_node_entries() -> Result<(), Box<dyn Error>> {
        let mut map = UrlMap::new();
        let mut schemas = EndpointSchemas::new();
        let bodies = MemoryBodySource::new();
        let first = ExchangeBuilder::http(Method::GET, "http://fixture.test/0")?
            .respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
        let observation = observe_exchange(&first, &bodies);

        for index in 0..MAX_ENDPOINTS {
            let exchange =
                ExchangeBuilder::http(Method::GET, format!("http://fixture.test/{index}"))?
                    .respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
            let node = map
                .observe_exchange(&exchange)
                .ok_or("missing URL map node")?;
            schemas.record(
                &EndpointKey {
                    node,
                    method: Method::GET,
                },
                &observation,
            );
        }

        let omitted = ExchangeBuilder::http(Method::GET, "http://fixture.test/overflow")?
            .respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
        let omitted_node = map
            .observe_exchange(&omitted)
            .ok_or("missing URL map node")?;
        schemas.record(
            &EndpointKey {
                node: omitted_node,
                method: Method::GET,
            },
            &observation,
        );
        assert!(!schemas.by_node.contains_key(&omitted_node));
        assert_eq!(schemas.by_node.len(), MAX_ENDPOINTS);

        let first_node = map.observe_exchange(&first).ok_or("missing URL map node")?;
        schemas.record(
            &EndpointKey {
                node: first_node,
                method: Method::POST,
            },
            &observation,
        );
        schemas.record(
            &EndpointKey {
                node: first_node,
                method: Method::GET,
            },
            &observation,
        );
        assert_eq!(schemas.omitted_endpoints(), 2);
        assert_eq!(schemas.request_count(&[first_node], &Method::GET), 2);
        assert_eq!(schemas.request_count(&[first_node], &Method::POST), 0);
        Ok(())
    }
}
