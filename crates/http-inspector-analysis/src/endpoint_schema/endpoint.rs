//! Per-endpoint (path node + method) aggregate of request and response structure.

use std::collections::BTreeMap;

use super::body::{BodyAnalysis, BodySchema};
use super::value::{TypeCounts, ValueShape, bump};
use crate::url::IdKind;
use http_inspector_exchange::AuthScheme;

/// Most query parameter names kept per endpoint.
pub const MAX_QUERY_PARAMS: usize = 64;
/// Most custom header names kept per endpoint.
pub const MAX_HEADER_NAMES: usize = 64;
/// Most body groups (`Content-Type`, or status and `Content-Type`) kept per endpoint.
pub const MAX_BODY_GROUPS: usize = 32;
/// Group name for a body sent without `Content-Type`.
pub const NO_CONTENT_TYPE: &str = "(no Content-Type)";

/// One body with the `Content-Type` that groups it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyObservation {
    /// `Content-Type` essence, or `None` when the header was missing or invalid.
    pub content_type: Option<String>,
    /// Structure of the body.
    pub analysis: BodyAnalysis,
}

impl BodyObservation {
    fn group(&self) -> String {
        self.content_type
            .clone()
            .unwrap_or_else(|| NO_CONTENT_TYPE.to_owned())
    }
}

/// Response part of one observed exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseObservation {
    /// A response arrived with this status code and body.
    Received {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: BodyObservation,
    },
    /// No response arrived (failed, cancelled or incomplete exchange).
    Missing,
}

/// Structure observed in one exchange, without any header, query, cookie,
/// credential or body value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointObservation {
    /// ID-like path segments by position (0 = first segment after the origin).
    pub path_ids: Vec<(usize, IdKind)>,
    /// Query parameter names (raw, as in the URL map) with their value shape.
    pub query: Vec<(String, ValueShape)>,
    /// Lowercase names of non-standard request headers.
    pub headers: Vec<String>,
    /// Authentication schemes recognized on the request.
    pub auth: Vec<AuthScheme>,
    /// Request body.
    pub request_body: BodyObservation,
    /// Response.
    pub response: ResponseObservation,
}

/// Presence ratio and value types of one query parameter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParamStats {
    seen: u64,
    types: TypeCounts,
}

impl ParamStats {
    /// Requests that carried this parameter.
    #[must_use]
    pub const fn seen(&self) -> u64 {
        self.seen
    }

    /// Observed value types.
    #[must_use]
    pub const fn types(&self) -> &TypeCounts {
        &self.types
    }

    fn merge(&mut self, other: &Self) {
        self.seen = self.seen.saturating_add(other.seen);
        self.types.merge(&other.types);
    }
}

/// Groups response bodies by status code and `Content-Type`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResponseKey {
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type` essence or [`NO_CONTENT_TYPE`].
    pub content_type: String,
}

/// Accumulated structure of every request to one path and method.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndpointSchema {
    requests: u64,
    path_ids: BTreeMap<usize, BTreeMap<IdKind, u64>>,
    query: BTreeMap<String, ParamStats>,
    omitted_query: u64,
    headers: BTreeMap<String, u64>,
    omitted_headers: u64,
    auth: BTreeMap<AuthScheme, u64>,
    requests_without_body: u64,
    request_bodies: BTreeMap<String, BodySchema>,
    responses: BTreeMap<ResponseKey, BodySchema>,
    missing_responses: u64,
    omitted_groups: u64,
}

impl EndpointSchema {
    /// Adds one observed exchange.
    pub fn record(&mut self, observation: &EndpointObservation) {
        self.requests = self.requests.saturating_add(1);
        for &(position, kind) in &observation.path_ids {
            bump(self.path_ids.entry(position).or_default(), kind, 1);
        }
        self.record_query(&observation.query);
        self.record_headers(&observation.headers);
        for &scheme in &observation.auth {
            bump(&mut self.auth, scheme, 1);
        }
        self.record_request_body(&observation.request_body);
        match &observation.response {
            ResponseObservation::Received { status, body } => {
                let key = ResponseKey {
                    status: *status,
                    content_type: body.group(),
                };
                let mut group = BodySchema::default();
                group.record(&body.analysis);
                self.merge_response_group(&key, &group);
            }
            ResponseObservation::Missing => {
                self.missing_responses = self.missing_responses.saturating_add(1);
            }
        }
    }

    /// Adds every observation of `other`, as when ID folding joins two paths.
    pub fn merge(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        for (&position, kinds) in &other.path_ids {
            let target = self.path_ids.entry(position).or_default();
            for (&kind, &count) in kinds {
                bump(target, kind, count);
            }
        }
        for (name, stats) in &other.query {
            self.merge_query(name, stats);
        }
        self.omitted_query = self.omitted_query.saturating_add(other.omitted_query);
        for (name, &count) in &other.headers {
            self.merge_header(name, count);
        }
        self.omitted_headers = self.omitted_headers.saturating_add(other.omitted_headers);
        for (&scheme, &count) in &other.auth {
            bump(&mut self.auth, scheme, count);
        }
        self.requests_without_body = self
            .requests_without_body
            .saturating_add(other.requests_without_body);
        for (content_type, group) in &other.request_bodies {
            self.merge_request_group(content_type, group);
        }
        for (key, group) in &other.responses {
            self.merge_response_group(key, group);
        }
        self.missing_responses = self
            .missing_responses
            .saturating_add(other.missing_responses);
        self.omitted_groups = self.omitted_groups.saturating_add(other.omitted_groups);
    }

    /// Requests observed.
    #[must_use]
    pub const fn requests(&self) -> u64 {
        self.requests
    }

    /// ID formats seen at each path position.
    #[must_use]
    pub const fn path_ids(&self) -> &BTreeMap<usize, BTreeMap<IdKind, u64>> {
        &self.path_ids
    }

    /// Query parameters by raw name.
    #[must_use]
    pub const fn query(&self) -> &BTreeMap<String, ParamStats> {
        &self.query
    }

    /// Query names not kept because of [`MAX_QUERY_PARAMS`].
    #[must_use]
    pub const fn omitted_query(&self) -> u64 {
        self.omitted_query
    }

    /// Custom request header names with the number of requests carrying them.
    #[must_use]
    pub const fn headers(&self) -> &BTreeMap<String, u64> {
        &self.headers
    }

    /// Header names not kept because of [`MAX_HEADER_NAMES`].
    #[must_use]
    pub const fn omitted_headers(&self) -> u64 {
        self.omitted_headers
    }

    /// Authentication schemes with the number of requests using them.
    #[must_use]
    pub const fn auth(&self) -> &BTreeMap<AuthScheme, u64> {
        &self.auth
    }

    /// Requests sent without a body and without `Content-Type`.
    #[must_use]
    pub const fn requests_without_body(&self) -> u64 {
        self.requests_without_body
    }

    /// Request bodies by `Content-Type`.
    #[must_use]
    pub const fn request_bodies(&self) -> &BTreeMap<String, BodySchema> {
        &self.request_bodies
    }

    /// Response bodies by status and `Content-Type`.
    #[must_use]
    pub const fn responses(&self) -> &BTreeMap<ResponseKey, BodySchema> {
        &self.responses
    }

    /// Requests that received no response.
    #[must_use]
    pub const fn missing_responses(&self) -> u64 {
        self.missing_responses
    }

    /// Body groups not kept because of [`MAX_BODY_GROUPS`].
    #[must_use]
    pub const fn omitted_groups(&self) -> u64 {
        self.omitted_groups
    }

    /// Bodies left out of structure aggregation on both sides.
    #[must_use]
    pub fn excluded_bodies(&self) -> u64 {
        self.request_bodies
            .values()
            .chain(self.responses.values())
            .fold(0, |sum, group| sum.saturating_add(group.excluded_total()))
    }

    /// Bodies whose structure was aggregated on both sides.
    #[must_use]
    pub fn structured_bodies(&self) -> u64 {
        self.request_bodies
            .values()
            .chain(self.responses.values())
            .fold(0, |sum, group| {
                sum.saturating_add(group.structured_samples())
            })
    }

    fn record_query(&mut self, query: &[(String, ValueShape)]) {
        let mut per_request: BTreeMap<&str, ParamStats> = BTreeMap::new();
        for (name, shape) in query {
            let stats = per_request.entry(name.as_str()).or_default();
            stats.seen = 1;
            stats.types.record(*shape);
        }
        for (name, stats) in per_request {
            self.merge_query(name, &stats);
        }
    }

    fn merge_query(&mut self, name: &str, stats: &ParamStats) {
        if let Some(existing) = self.query.get_mut(name) {
            existing.merge(stats);
        } else if self.query.len() < MAX_QUERY_PARAMS {
            let _ = self.query.insert(name.to_owned(), stats.clone());
        } else {
            self.omitted_query = self.omitted_query.saturating_add(1);
        }
    }

    fn record_headers(&mut self, headers: &[String]) {
        let mut unique: Vec<&String> = headers.iter().collect();
        unique.sort_unstable();
        unique.dedup();
        for name in unique {
            self.merge_header(name, 1);
        }
    }

    fn merge_header(&mut self, name: &str, count: u64) {
        if let Some(existing) = self.headers.get_mut(name) {
            *existing = existing.saturating_add(count);
        } else if self.headers.len() < MAX_HEADER_NAMES {
            let _ = self.headers.insert(name.to_owned(), count);
        } else {
            self.omitted_headers = self.omitted_headers.saturating_add(1);
        }
    }

    fn record_request_body(&mut self, body: &BodyObservation) {
        if body.content_type.is_none() && body.analysis == BodyAnalysis::Empty {
            self.requests_without_body = self.requests_without_body.saturating_add(1);
            return;
        }
        let mut group = BodySchema::default();
        group.record(&body.analysis);
        self.merge_request_group(&body.group(), &group);
    }

    fn merge_request_group(&mut self, content_type: &str, group: &BodySchema) {
        if let Some(existing) = self.request_bodies.get_mut(content_type) {
            existing.merge(group);
        } else if self.request_bodies.len() < MAX_BODY_GROUPS {
            let _ = self
                .request_bodies
                .insert(content_type.to_owned(), group.clone());
        } else {
            self.omitted_groups = self.omitted_groups.saturating_add(1);
        }
    }

    fn merge_response_group(&mut self, key: &ResponseKey, group: &BodySchema) {
        if let Some(existing) = self.responses.get_mut(key) {
            existing.merge(group);
        } else if self.responses.len() < MAX_BODY_GROUPS {
            let _ = self.responses.insert(key.clone(), group.clone());
        } else {
            self.omitted_groups = self.omitted_groups.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint_schema::body::BodyParse;
    use crate::endpoint_schema::value::ValueKind;
    use http_inspector_exchange::ReadBodyOutcome;

    fn observation(status: u16, response: &str) -> EndpointObservation {
        EndpointObservation {
            path_ids: vec![(1, IdKind::Integer)],
            query: vec![
                ("page".to_owned(), ValueShape::of_text("2")),
                ("page".to_owned(), ValueShape::of_text("3")),
            ],
            headers: vec!["x-trace".to_owned(), "x-trace".to_owned()],
            auth: vec![AuthScheme::Bearer],
            request_body: BodyObservation {
                content_type: None,
                analysis: BodyAnalysis::Empty,
            },
            response: ResponseObservation::Received {
                status,
                body: BodyObservation {
                    content_type: Some("application/json".to_owned()),
                    analysis: BodyAnalysis::describe(
                        &BodyParse::Json,
                        ReadBodyOutcome::Complete(response.as_bytes().to_vec()),
                    ),
                },
            },
        }
    }

    #[test]
    fn records_presence_once_per_request_and_groups_by_status() {
        let mut schema = EndpointSchema::default();
        schema.record(&observation(200, r#"{"id":1}"#));
        schema.record(&observation(404, r#"{"error":"missing"}"#));
        assert_eq!(schema.requests(), 2);
        assert_eq!(schema.query().get("page").map(ParamStats::seen), Some(2));
        assert_eq!(
            schema
                .query()
                .get("page")
                .map(|param| param.types().count(ValueKind::Integer)),
            Some(4)
        );
        assert_eq!(schema.headers().get("x-trace"), Some(&2));
        assert_eq!(schema.auth().get(&AuthScheme::Bearer), Some(&2));
        assert_eq!(schema.requests_without_body(), 2);
        assert!(schema.request_bodies().is_empty());
        assert_eq!(schema.responses().len(), 2);
        assert_eq!(
            schema
                .path_ids()
                .get(&1)
                .and_then(|kinds| kinds.get(&IdKind::Integer)),
            Some(&2)
        );

        let mut folded = schema.clone();
        folded.merge(&schema);
        assert_eq!(folded.requests(), 4);
        assert_eq!(folded.structured_bodies(), 4);
        assert_eq!(folded.responses().len(), 2);
    }
}
