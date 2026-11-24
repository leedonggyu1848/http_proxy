//! Shared URL canonicalization, dynamic ID segment classification/folding, and
//! Public Suffix List site classification reused by [`crate::url_map`]
//! and [`crate::endpoint_schema`].
//!
//! Privacy invariant: query parameter values are stripped at URL parse time and
//! never retained in [`NormalizedUrl`].

mod id_fold;
mod normalize;
mod site;

pub use id_fold::{ID_FOLD_THRESHOLD, IdKind, PathSegmentPattern, classify_id_segment};
pub use normalize::{
    NormalizedUrl, OriginKey, parse_exchange_target, parse_origin_header, parse_url,
    percent_decode_display, resolve_location, visit_query_values,
};
pub use site::{PartyKind, is_third_party, registrable_domain};
