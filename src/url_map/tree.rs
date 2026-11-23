//! Real-time URL map tree with O(depth) insertion, bounded node capacity, and
//! live settlement tracking for in-flight exchanges.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use http::Method;

use crate::url_map::edges::{EdgeKind, RelationshipGraph};
use crate::url_map::normalize::{
    NormalizedUrl, OriginKey, parse_exchange_target, parse_origin_header, parse_url,
    resolve_location,
};
use crate::url_map::site::is_third_party;
use crate::url_map::stats::{NodeStats, StatusBucket};
use crate::{Exchange, Outcome};

/// Default upper bound on total tree nodes kept in memory.
///
/// Once reached, new deeper paths are folded into their deepest existing
/// ancestor node instead of allocating additional nodes.
pub const MAX_NODES: usize = 50_000;

/// Maximum number of concurrent in-flight exchanges tracked for live updates.
const MAX_PENDING_EXCHANGES: usize = 4_096;

/// Arena handle for a node inside [`UrlMap`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(usize);

impl NodeId {
    /// Returns the internal arena index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// One node in the raw normalized URL tree (`origin` root or path segment).
#[derive(Clone, Debug)]
pub struct MapNode {
    /// Stable node identifier within the current map session.
    pub id: NodeId,
    /// Parent node, or `None` for an origin root.
    pub parent: Option<NodeId>,
    /// Origin this node belongs to.
    pub origin: OriginKey,
    /// Raw normalized path segment, or empty string for an origin root.
    pub segment: String,
    /// Depth from origin root (`0` for origin root, `1..` for path segments).
    pub depth: usize,
    /// Child lookup table keyed by raw segment for O(1) per-level insertion.
    pub children: HashMap<String, NodeId>,
    /// Statistics for requests landing directly on this node.
    pub direct: NodeStats,
    /// Rolled-up statistics for this node and all of its descendants.
    pub subtree: NodeStats,
    /// Monotonic instant when this node was first inserted (for brief UI highlight).
    pub created_at: Instant,
}

/// Self-contained observation record for deterministic tests and benchmarks.
#[derive(Clone, Debug)]
pub struct ObservedInput {
    /// Normalized target URL (query values already stripped).
    pub target: NormalizedUrl,
    /// HTTP request method.
    pub method: Method,
    /// Observed status bucket.
    pub status: StatusBucket,
    /// Observed response Content-Type essence, if any.
    pub content_type: Option<String>,
    /// Observed transferred bytes.
    pub bytes: u64,
    /// Observed duration, if completed.
    pub duration: Option<Duration>,
    /// Wall-clock timestamp of request arrival.
    pub started_at: SystemTime,
    /// Parsed `Referer` URL, if present.
    pub referer: Option<NormalizedUrl>,
    /// Parsed `Origin` header, if present.
    pub origin_header: Option<OriginKey>,
    /// Resolved `3xx` `Location` URL, if present.
    pub redirect_location: Option<NormalizedUrl>,
}

#[derive(Clone, Debug)]
struct PendingExchange {
    exchange: Exchange,
    node_id: NodeId,
    target: NormalizedUrl,
    status_bucket: StatusBucket,
    settled_recorded: bool,
    recorded_bytes: u64,
    error_recorded: bool,
}

/// Real-time URL tree and domain relationship graph built from captured traffic.
#[derive(Debug)]
pub struct UrlMap {
    nodes: Vec<MapNode>,
    origins: BTreeMap<OriginKey, NodeId>,
    graph: RelationshipGraph,
    max_nodes: usize,
    node_limit_reached: bool,
    version: u64,
    pending: Vec<PendingExchange>,
}

impl Default for UrlMap {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlMap {
    /// Creates an empty URL map with the default [`MAX_NODES`] node limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_nodes(MAX_NODES)
    }

    /// Creates an empty URL map with an explicit `max_nodes` upper bound.
    #[must_use]
    pub fn with_max_nodes(max_nodes: usize) -> Self {
        Self {
            nodes: Vec::new(),
            origins: BTreeMap::new(),
            graph: RelationshipGraph::default(),
            max_nodes: max_nodes.max(1),
            node_limit_reached: false,
            version: 0,
            pending: Vec::new(),
        }
    }

    /// Resets all nodes, aggregates, edges, and pending trackers.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.origins.clear();
        self.graph.clear();
        self.node_limit_reached = false;
        self.pending.clear();
        self.bump_version();
    }

    /// Monotonic counter incremented whenever nodes, stats, or edges change.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Total number of nodes currently allocated in the tree.
    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Reports whether the node cap was hit at least once since the last clear.
    #[must_use]
    pub const fn node_limit_reached(&self) -> bool {
        self.node_limit_reached
    }

    /// Returns the configured node cap.
    #[must_use]
    pub const fn max_nodes(&self) -> usize {
        self.max_nodes
    }

    /// Returns a node by its [`NodeId`].
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&MapNode> {
        self.nodes.get(id.0)
    }

    /// Returns all origin roots in canonical sorted order.
    #[must_use]
    pub const fn origins(&self) -> &BTreeMap<OriginKey, NodeId> {
        &self.origins
    }

    /// Returns the collected domain/URL relationship graph.
    #[must_use]
    pub const fn graph(&self) -> &RelationshipGraph {
        &self.graph
    }

    /// Returns the cumulative request count matching `key`'s origin and path
    /// prefix (including folded `{id}` patterns) without rebuilding the view.
    #[must_use]
    pub fn matching_request_count(&self, key: &crate::url_map::view::ViewNodeKey) -> u64 {
        let Some(&origin_id) = self.origins.get(&key.origin) else {
            return 0;
        };
        let mut frontier = vec![origin_id];
        for pattern in &key.path {
            let mut next = Vec::new();
            for node_id in frontier {
                let Some(node) = self.nodes.get(node_id.0) else {
                    continue;
                };
                match pattern {
                    crate::url_map::id_fold::PathSegmentPattern::Literal(seg) => {
                        if let Some(&child_id) = node.children.get(seg) {
                            next.push(child_id);
                        }
                    }
                    crate::url_map::id_fold::PathSegmentPattern::FoldedId => {
                        for (seg, &child_id) in &node.children {
                            if crate::url_map::id_fold::classify_id_segment(seg).is_some() {
                                next.push(child_id);
                            }
                        }
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                return 0;
            }
        }
        frontier
            .into_iter()
            .filter_map(|id| self.nodes.get(id.0))
            .fold(0_u64, |acc, node| {
                acc.saturating_add(node.subtree.request_count)
            })
    }

    /// Inserts a newly received [`Exchange`] into the map and registers it for
    /// live settlement/streaming updates until it finishes.
    pub fn observe_exchange(&mut self, exchange: &Exchange) -> Option<NodeId> {
        let target = parse_exchange_target(exchange.request().target(), exchange.source())?;
        let referer = exchange
            .request()
            .headers()
            .get("referer")
            .next()
            .and_then(parse_url);
        let origin_header = exchange
            .request()
            .headers()
            .get("origin")
            .next()
            .and_then(parse_origin_header);

        let third_party = detect_third_party(
            target.origin().host(),
            referer.as_ref(),
            origin_header.as_ref(),
        );
        let initial_bucket = StatusBucket::from_outcome(exchange.outcome());
        let is_opaque_tunnel = matches!(exchange.outcome(), Outcome::Tunnel(_));

        let (node_id, overflowed) = self.ensure_path(&target)?;
        self.record_arrival(
            node_id,
            &target,
            exchange.request().method(),
            initial_bucket,
            exchange.started_at(),
            third_party,
            is_opaque_tunnel,
            overflowed,
        );
        self.record_request_edges(&target, referer.as_ref(), origin_header.as_ref());

        let mut tracker = PendingExchange {
            exchange: exchange.clone(),
            node_id,
            target,
            status_bucket: initial_bucket,
            settled_recorded: false,
            recorded_bytes: 0,
            error_recorded: false,
        };
        self.apply_live_progress(&mut tracker);
        if tracker.exchange.is_live() && self.pending.len() < MAX_PENDING_EXCHANGES {
            self.pending.push(tracker);
        }
        self.bump_version();
        Some(node_id)
    }

    /// Polls currently live exchanges for newly arrived responses, streamed
    /// bytes, redirect headers, and final durations.
    ///
    /// Returns `true` if any map state changed.
    pub fn sync_live(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut remaining = Vec::with_capacity(self.pending.len());
        let pending = std::mem::take(&mut self.pending);
        for mut entry in pending {
            if self.apply_live_progress(&mut entry) {
                changed = true;
            }
            if entry.exchange.is_live() {
                remaining.push(entry);
            }
        }
        self.pending = remaining;
        if changed {
            self.bump_version();
        }
        changed
    }

    /// Inserts a complete [`ObservedInput`] directly (used in tests and benchmarks).
    pub fn observe(&mut self, input: ObservedInput) -> Option<NodeId> {
        let third_party = detect_third_party(
            input.target.origin().host(),
            input.referer.as_ref(),
            input.origin_header.as_ref(),
        );
        let (node_id, overflowed) = self.ensure_path(&input.target)?;
        self.record_arrival(
            node_id,
            &input.target,
            &input.method,
            input.status,
            input.started_at,
            third_party,
            input.target.is_tunnel(),
            overflowed,
        );
        self.record_request_edges(
            &input.target,
            input.referer.as_ref(),
            input.origin_header.as_ref(),
        );
        if matches!(
            input.status,
            StatusBucket::S4xx | StatusBucket::S5xx | StatusBucket::Failed
        ) {
            self.graph.record_error(input.target.origin().host());
        }
        if let Some(ct) = input.content_type.as_deref() {
            self.for_node_and_ancestors(node_id, |node, _| node.subtree.record_content_type(ct));
            if let Some(node) = self.nodes.get_mut(node_id.0) {
                node.direct.record_content_type(ct);
            }
        }
        if input.bytes > 0 {
            self.add_bytes_upward(node_id, input.target.origin().host(), input.bytes);
        }
        if let Some(duration) = input.duration {
            self.record_duration_upward(node_id, duration);
        }
        if let Some(dest) = input.redirect_location {
            self.graph.record_edge(
                input.target.origin().host(),
                dest.origin().host(),
                input.target.path_only(),
                dest.path_only(),
                EdgeKind::Redirect,
            );
        }
        self.bump_version();
        Some(node_id)
    }

    fn apply_live_progress(&mut self, entry: &mut PendingExchange) -> bool {
        let mut changed = false;
        let outcome = entry.exchange.outcome();
        let next_bucket = StatusBucket::from_outcome(outcome);

        if next_bucket != entry.status_bucket {
            let prev = entry.status_bucket;
            self.for_node_and_ancestors(entry.node_id, |node, is_direct| {
                if is_direct {
                    node.direct.transition_status(prev, next_bucket);
                }
                node.subtree.transition_status(prev, next_bucket);
            });
            entry.status_bucket = next_bucket;
            changed = true;
        }

        if !entry.error_recorded
            && matches!(
                next_bucket,
                StatusBucket::S4xx | StatusBucket::S5xx | StatusBucket::Failed
            )
        {
            self.graph.record_error(entry.target.origin().host());
            entry.error_recorded = true;
            changed = true;
        }

        if !entry.settled_recorded && !outcome.is_in_flight() {
            entry.settled_recorded = true;
            if self.record_settlement_outcome(entry.node_id, &entry.target, outcome) {
                changed = true;
            }
        }

        if let Some(bytes) = entry.exchange.received_bytes()
            && bytes > entry.recorded_bytes
        {
            let delta = bytes - entry.recorded_bytes;
            entry.recorded_bytes = bytes;
            self.add_bytes_upward(entry.node_id, entry.target.origin().host(), delta);
            changed = true;
        }

        if !entry.exchange.is_live() {
            let timing = entry.exchange.timing();
            if let Some(duration) = timing.complete().or_else(|| timing.settled()) {
                self.record_duration_upward(entry.node_id, duration);
                changed = true;
            }
        }
        changed
    }

    fn record_settlement_outcome(
        &mut self,
        node_id: NodeId,
        target: &NormalizedUrl,
        outcome: &Outcome,
    ) -> bool {
        let mut changed = false;
        match outcome {
            Outcome::Response(response) => {
                if let Some(media) = response.media_type() {
                    let essence = media.essence().to_owned();
                    self.for_node_and_ancestors(node_id, |node, is_direct| {
                        if is_direct {
                            node.direct.record_content_type(&essence);
                        }
                        node.subtree.record_content_type(&essence);
                    });
                    changed = true;
                }
                if response.status().is_redirection()
                    && let Some(location) = response.headers().get("location").next()
                    && let Some(dest) = resolve_location(target, location)
                {
                    self.graph.record_edge(
                        target.origin().host(),
                        dest.origin().host(),
                        target.path_only(),
                        dest.path_only(),
                        EdgeKind::Redirect,
                    );
                    changed = true;
                }
            }
            Outcome::Tunnel(_) => {
                self.for_node_and_ancestors(node_id, |node, is_direct| {
                    if is_direct {
                        node.direct.has_tunnel = true;
                    }
                    node.subtree.has_tunnel = true;
                });
                changed = true;
            }
            Outcome::InFlight
            | Outcome::InspectionEstablished
            | Outcome::Failed { .. }
            | Outcome::Cancelled
            | Outcome::Incomplete => {}
        }
        changed
    }

    fn ensure_path(&mut self, target: &NormalizedUrl) -> Option<(NodeId, bool)> {
        let now = Instant::now();
        let origin_id = if let Some(&id) = self.origins.get(target.origin()) {
            id
        } else {
            if self.nodes.len() >= self.max_nodes {
                self.node_limit_reached = true;
                return None;
            }
            let id = self.alloc_node(None, target.origin().clone(), String::new(), 0, now);
            let _ = self.origins.insert(target.origin().clone(), id);
            id
        };

        if target.is_tunnel() {
            return Some((origin_id, false));
        }

        let mut current = origin_id;
        let mut overflowed = false;
        for (depth_offset, segment) in target.segments().iter().enumerate() {
            let existing = self
                .nodes
                .get(current.0)
                .and_then(|node| node.children.get(segment).copied());
            if let Some(child_id) = existing {
                current = child_id;
                continue;
            }
            if self.nodes.len() >= self.max_nodes {
                self.node_limit_reached = true;
                overflowed = true;
                break;
            }
            let child_id = self.alloc_node(
                Some(current),
                target.origin().clone(),
                segment.clone(),
                depth_offset + 1,
                now,
            );
            if let Some(parent_node) = self.nodes.get_mut(current.0) {
                let _ = parent_node.children.insert(segment.clone(), child_id);
            }
            current = child_id;
        }
        Some((current, overflowed))
    }

    fn alloc_node(
        &mut self,
        parent: Option<NodeId>,
        origin: OriginKey,
        segment: String,
        depth: usize,
        created_at: Instant,
    ) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(MapNode {
            id,
            parent,
            origin,
            segment,
            depth,
            children: HashMap::new(),
            direct: NodeStats::default(),
            subtree: NodeStats::default(),
            created_at,
        });
        id
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "internal helper writing validated request arrival fields"
    )]
    fn record_arrival(
        &mut self,
        node_id: NodeId,
        target: &NormalizedUrl,
        method: &Method,
        initial_bucket: StatusBucket,
        started_at: SystemTime,
        third_party: bool,
        is_opaque_tunnel: bool,
        overflowed: bool,
    ) {
        self.graph.record_request(target.origin(), third_party);
        let query_names = target.query_param_names();
        self.for_node_and_ancestors(node_id, |node, is_direct| {
            if is_direct {
                node.direct.record_request(
                    method,
                    initial_bucket,
                    started_at,
                    third_party,
                    is_opaque_tunnel,
                );
                node.direct.record_query_params(query_names);
                if overflowed {
                    node.direct.overflow_merged = node.direct.overflow_merged.saturating_add(1);
                }
            }
            node.subtree.record_request(
                method,
                initial_bucket,
                started_at,
                third_party,
                is_opaque_tunnel,
            );
            node.subtree.record_query_params(query_names);
            if overflowed {
                node.subtree.overflow_merged = node.subtree.overflow_merged.saturating_add(1);
            }
        });
    }

    fn record_request_edges(
        &mut self,
        target: &NormalizedUrl,
        referer: Option<&NormalizedUrl>,
        origin_header: Option<&OriginKey>,
    ) {
        let target_host = target.origin().host();
        let target_path = target.path_only();
        if let Some(ref_url) = referer {
            self.graph.record_edge(
                ref_url.origin().host(),
                target_host,
                ref_url.path_only(),
                target_path.clone(),
                EdgeKind::Referer,
            );
        }
        if let Some(origin) = origin_header
            && origin != target.origin()
        {
            self.graph.record_edge(
                origin.host(),
                target_host,
                origin.to_string(),
                target_path,
                EdgeKind::Origin,
            );
        }
    }

    fn add_bytes_upward(&mut self, node_id: NodeId, host: &str, delta: u64) {
        self.graph.add_bytes(host, delta);
        self.for_node_and_ancestors(node_id, |node, is_direct| {
            if is_direct {
                node.direct.add_bytes(delta);
            }
            node.subtree.add_bytes(delta);
        });
    }

    fn record_duration_upward(&mut self, node_id: NodeId, duration: Duration) {
        self.for_node_and_ancestors(node_id, |node, is_direct| {
            if is_direct {
                node.direct.record_duration(duration);
            }
            node.subtree.record_duration(duration);
        });
    }

    fn for_node_and_ancestors(&mut self, start: NodeId, mut visit: impl FnMut(&mut MapNode, bool)) {
        let mut cursor = Some(start);
        let mut is_direct = true;
        while let Some(id) = cursor {
            let Some(node) = self.nodes.get_mut(id.0) else {
                break;
            };
            visit(node, is_direct);
            cursor = node.parent;
            is_direct = false;
        }
    }

    const fn bump_version(&mut self) {
        self.version = self.version.saturating_add(1);
    }
}

fn detect_third_party(
    target_host: &str,
    referer: Option<&NormalizedUrl>,
    origin_header: Option<&OriginKey>,
) -> bool {
    referer.is_some_and(|url| is_third_party(target_host, url.origin().host()))
        || origin_header.is_some_and(|origin| is_third_party(target_host, origin.host()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input(url: &str, status: StatusBucket, bytes: u64) -> ObservedInput {
        let Some(target) = parse_url(url) else {
            unreachable!("test URL is valid: {url}");
        };
        ObservedInput {
            target,
            method: Method::GET,
            status,
            content_type: Some("application/json".to_owned()),
            bytes,
            duration: Some(Duration::from_millis(25)),
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            referer: None,
            origin_header: None,
            redirect_location: None,
        }
    }

    #[test]
    fn inserts_hierarchy_rolls_up_stats_and_keeps_only_query_names() {
        let mut map = UrlMap::new();
        let _ = map.observe(sample_input(
            "https://api.example.com/v1/users?token=SECRET_VALUE&page=1",
            StatusBucket::S2xx,
            120,
        ));
        let mut post = sample_input(
            "https://api.example.com/v1/users/42?page=2&secret=DO_NOT_KEEP",
            StatusBucket::S4xx,
            80,
        );
        post.method = Method::POST;
        let _ = map.observe(post);

        // origin + v1 + users + 42 = 4 nodes
        assert_eq!(map.node_count(), 4);
        let origin_id = map
            .origins()
            .get(&OriginKey::new("https", "api.example.com", None))
            .copied();
        let root = origin_id.and_then(|id| map.node(id));
        assert!(root.is_some());
        if let Some(root) = root {
            assert_eq!(root.subtree.request_count, 2);
            assert_eq!(root.subtree.total_bytes, 200);
            assert_eq!(root.subtree.status.s2xx, 1);
            assert_eq!(root.subtree.status.s4xx, 1);
            assert_eq!(root.subtree.methods.names(), ["GET", "POST"]);
            assert_eq!(root.subtree.query_params.get("token"), Some(&1));
            assert_eq!(root.subtree.query_params.get("page"), Some(&2));
            assert!(!root.subtree.query_params.contains_key("SECRET_VALUE"));
            assert!(!root.subtree.query_params.contains_key("DO_NOT_KEEP"));
        }
    }

    #[test]
    fn enforces_node_cap_by_folding_overflow_into_deepest_ancestor() {
        let mut map = UrlMap::with_max_nodes(3);
        // Allocates origin (1), api (2), users (3) -> hits cap at 3.
        let _ = map.observe(sample_input(
            "https://example.com/api/users",
            StatusBucket::S2xx,
            10,
        ));
        assert_eq!(map.node_count(), 3);
        assert!(!map.node_limit_reached());

        // Deeper path cannot allocate a 4th node; folds into `/api/users`.
        let overflow_id = map.observe(sample_input(
            "https://example.com/api/users/999/profile",
            StatusBucket::S2xx,
            30,
        ));
        assert_eq!(map.node_count(), 3);
        assert!(map.node_limit_reached());
        let node = overflow_id.and_then(|id| map.node(id));
        assert_eq!(node.map(|n| n.segment.as_str()), Some("users"));
        assert_eq!(node.map(|n| n.direct.request_count), Some(2));
        assert_eq!(node.map(|n| n.direct.overflow_merged), Some(1));
        assert_eq!(node.map(|n| n.subtree.total_bytes), Some(40));
    }

    #[test]
    fn collects_referer_origin_and_redirect_edges_without_query_values() {
        let mut map = UrlMap::new();
        let mut req = sample_input(
            "https://cdn.thirdparty.net/sdk.js?api_key=SECRET_KEY",
            StatusBucket::S3xx,
            50,
        );
        req.referer = parse_url("https://shop.example.com/cart?session=SENSITIVE_REF");
        req.origin_header = parse_origin_header("https://shop.example.com");
        req.redirect_location = parse_url("https://static.thirdparty.net/v2/sdk.js?sig=SECRET_SIG");

        let _ = map.observe(req);

        assert_eq!(map.graph().domain_edges().len(), 3);
        for url_edge in map.graph().url_edges().keys() {
            assert!(!url_edge.from_url.contains("SENSITIVE_REF"));
            assert!(!url_edge.from_url.contains("SECRET_KEY"));
            assert!(!url_edge.to_url.contains("SECRET_KEY"));
            assert!(!url_edge.to_url.contains("SECRET_SIG"));
        }
        assert!(
            map.graph()
                .domains()
                .get("cdn.thirdparty.net")
                .is_some_and(crate::url_map::edges::DomainSummary::is_third_party)
        );
    }
}
