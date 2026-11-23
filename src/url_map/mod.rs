//! Real-time URL map (site map) and domain relationship graph.
//!
//! This module owns canonical URL normalization, shared dynamic ID segment
//! classification/folding (also reused by endpoint schema inference in #69),
//! Public Suffix List third-party classification, incremental tree aggregation,
//! force-directed graph layout, and text/JSON/Mermaid/DOT exporters.
//!
//! Privacy invariant: query parameter values are stripped at URL parse time and
//! never retained in the tree, relationship edges, or exported files.

mod edges;
mod export;
mod id_fold;
mod layout;
mod normalize;
mod site;
mod stats;
mod tree;
mod view;

pub use edges::{
    DomainEdgeKey, DomainEdgeStats, DomainSummary, EdgeKind, RelationshipGraph, UrlEdgeKey,
};
pub use export::{
    export_graph_dot, export_graph_mermaid, export_tree_json, export_tree_text, format_bytes,
};
pub use id_fold::{ID_FOLD_THRESHOLD, IdKind, PathSegmentPattern, classify_id_segment};
pub use layout::{DEFAULT_MAX_GRAPH_DOMAINS, ForceLayout, GraphEdge, GraphNode, STEPS_PER_FRAME};
pub use normalize::{
    NormalizedUrl, OriginKey, parse_exchange_target, parse_origin_header, parse_url,
    percent_decode_display, resolve_location,
};
pub use site::{PartyKind, is_third_party, registrable_domain};
pub use stats::{MethodSet, NodeStats, StatusBucket, StatusCounts};
pub use tree::{MAX_NODES, MapNode, NodeId, ObservedInput, UrlMap};
pub use view::{
    ContentTypeFilter, PartyFilter, RowExpansion, RowKind, SortDirection, StatusFilter, TreeFilter,
    TreeSortField, ViewNode, ViewNodeKey, VisibleRow, build_view_roots, find_view_node,
    flatten_visible,
};
