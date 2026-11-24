//! Traffic analysis over captured HTTP exchanges: URL normalization and ID
//! folding, real-time URL map, per-endpoint schema inference, text search, and
//! record filtering.
//!
//! Privacy invariant: query parameter values, header values, cookies,
//! credentials, and body field values are never retained in URL map or
//! endpoint schema aggregates. Analysis reads structured bodies only through
//! the [`BodySource`](crate::BodySource) port and does not depend on storage,
//! interception, HTTPS setup, or UI rendering.

pub mod endpoint_analysis;
pub mod endpoint_schema;
pub mod filter;
pub mod search;
pub mod url;
pub mod url_map;
