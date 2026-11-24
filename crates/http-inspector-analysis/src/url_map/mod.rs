//! Real-time URL map (site map) and domain relationship graph.
//!
//! This module owns incremental tree aggregation, domain and URL relationship
//! edges, node statistics, force-directed graph layout, and view projection.
//! Shared canonical URL normalization, dynamic ID classification/folding, and
//! Public Suffix List site classification live in [`crate::url`].
//!
//! Privacy invariant: query parameter values are stripped at URL parse time and
//! never retained in the tree, relationship edges, or exported files.

mod edges;
mod layout;
mod stats;
mod tree;
mod view;

pub use edges::{
    DomainEdgeKey, DomainEdgeStats, DomainSummary, EdgeKind, RelationshipGraph, UrlEdgeKey,
};
pub use layout::{DEFAULT_MAX_GRAPH_DOMAINS, ForceLayout, GraphEdge, GraphNode, STEPS_PER_FRAME};
pub use stats::{MethodSet, NodeStats, StatusBucket, StatusCounts};
pub use tree::{MAX_NODES, MapNode, NodeId, ObservedInput, UrlMap};
pub use view::{
    ContentTypeFilter, PartyFilter, RequestScope, RowExpansion, RowKind, SortDirection,
    StatusFilter, TreeFilter, TreeSortField, ViewNode, ViewNodeKey, VisibleRow, build_view_roots,
    find_view_node, flatten_visible,
};
