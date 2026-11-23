//! Per-endpoint request and response structure inferred from captured traffic.
//!
//! An endpoint is one URL map path node and one request method. For each
//! endpoint this module aggregates query parameter names, custom header
//! names, authentication scheme names, and the field structure of request
//! and response bodies grouped by `Content-Type` (and status code for
//! responses). ID folding in the Map view merges endpoint aggregates the
//! same way it merges path nodes.
//!
//! Privacy invariant: only names, types and counts are stored. Query values,
//! header values, cookies, credentials and body field values are classified
//! where they are read ([`observe_exchange`]) and never kept.
//!
//! Bodies are read through the [`BodySource`](crate::BodySource) port, so
//! analysis does not depend on how or where bodies are stored, and parsed
//! on a background thread ([`SchemaAnalyzer`]),
//! bounded by [`MAX_ANALYZED_BODY_BYTES`], [`MAX_SHAPE_DEPTH`] and
//! [`MAX_SHAPE_FIELDS`]. Bodies that were not captured completely are counted
//! as excluded instead of contributing a partial structure.

mod analyzer;
mod body;
mod endpoint;
mod multipart;
mod observe;
mod shape;
mod store;
mod value;

pub use analyzer::SchemaAnalyzer;
pub use body::{BodyAnalysis, BodyParse, BodySchema, ExclusionReason, PartStats};
pub use endpoint::{
    BodyObservation, EndpointObservation, EndpointSchema, MAX_BODY_GROUPS, MAX_HEADER_NAMES,
    MAX_QUERY_PARAMS, NO_CONTENT_TYPE, ParamStats, ResponseKey, ResponseObservation,
};
pub use multipart::{Boundary, MAX_PARTS, PartObservation, parse_parts};
pub use observe::{MAX_ANALYZED_BODY_BYTES, observe_exchange};
pub use shape::{FieldShape, ID_KEY, MAX_SHAPE_DEPTH, MAX_SHAPE_FIELDS};
pub use store::{EndpointKey, EndpointSchemas, MAX_ENDPOINTS, method_rank};
pub use value::{StringFormat, TypeCounts, ValueKind, ValueShape};
