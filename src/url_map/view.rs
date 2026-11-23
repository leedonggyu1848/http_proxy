//! View projection of [`UrlMap`] supporting ID folding (shared with #69),
//! filtering, sorting, and virtualized row flattening.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Instant;

use crate::Exchange;
use crate::url_map::id_fold::{ID_FOLD_THRESHOLD, PathSegmentPattern, classify_id_segment};
use crate::url_map::normalize::{
    NormalizedUrl, OriginKey, parse_exchange_target, percent_decode_display,
};
use crate::url_map::stats::NodeStats;
use crate::url_map::tree::{NodeId, UrlMap};

/// Stable identity of a rendered tree node, preserved across live updates and
/// shared with `Show in Requests` filtering and endpoint details (#69).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewNodeKey {
    /// Origin root of this node.
    pub origin: OriginKey,
    /// Path pattern from origin down to this node (empty for origin root).
    pub path: Vec<PathSegmentPattern>,
}

impl ViewNodeKey {
    /// Formats a human-readable URL or path pattern (e.g. `https://api.example.com/users/{id}`).
    #[must_use]
    pub fn display_url(&self) -> String {
        if self.path.is_empty() {
            return self.origin.to_string();
        }
        let mut out = self.origin.to_string();
        for pattern in &self.path {
            out.push('/');
            out.push_str(&pattern.display_text());
        }
        out
    }

    /// Formats the single-row display label for this node.
    #[must_use]
    pub fn row_label(&self) -> String {
        self.path.last().map_or_else(
            || self.origin.to_string(),
            |segment| format!("/{}", segment.display_text()),
        )
    }

    /// Reports whether `url` falls under this node's origin and path prefix.
    #[must_use]
    pub fn matches_url(&self, url: &NormalizedUrl) -> bool {
        if url.origin() != &self.origin {
            return false;
        }
        if self.path.is_empty() {
            return true;
        }
        if url.is_tunnel() || url.segments().len() < self.path.len() {
            return false;
        }
        self.path
            .iter()
            .zip(url.segments().iter())
            .all(|(pattern, segment)| pattern.matches(segment))
    }

    /// Reports whether `exchange` belongs to this node's origin and path prefix.
    #[must_use]
    pub fn matches_exchange(&self, exchange: &Exchange) -> bool {
        parse_exchange_target(exchange.request().target(), exchange.source())
            .is_some_and(|url| self.matches_url(&url))
    }
}

/// Sort key for siblings in the URL map tree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TreeSortField {
    /// Lexicographic order by display label.
    #[default]
    Name,
    /// Total requests in the node's subtree.
    Requests,
    /// Total errors (`4xx + 5xx + failed`) in the node's subtree.
    Errors,
    /// Total transferred bytes in the node's subtree.
    Bytes,
}

impl TreeSortField {
    /// All selectable sort fields in toolbar order.
    pub const ALL: [Self; 4] = [Self::Name, Self::Requests, Self::Errors, Self::Bytes];

    /// Display name shown in the sort selector.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Requests => "Requests",
            Self::Errors => "Errors",
            Self::Bytes => "Size",
        }
    }
}

/// Sort direction for the URL map tree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortDirection {
    /// Ascending order.
    #[default]
    Asc,
    /// Descending order.
    Desc,
}

impl SortDirection {
    /// Flips ascending to descending and vice versa.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }

    /// Compact arrow indicator.
    #[must_use]
    pub const fn arrow(self) -> &'static str {
        match self {
            Self::Asc => "▲",
            Self::Desc => "▼",
        }
    }
}

/// Status filter applied to URL map nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatusFilter {
    /// Include all statuses.
    #[default]
    All,
    /// Include nodes with `2xx` responses.
    S2xx,
    /// Include nodes with `3xx` responses.
    S3xx,
    /// Include nodes with `4xx` responses.
    S4xx,
    /// Include nodes with `5xx` responses.
    S5xx,
    /// Include nodes with failed/cancelled/incomplete requests.
    Failed,
    /// Include nodes with opaque tunnels.
    Tunnel,
}

impl StatusFilter {
    /// All status filter options.
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::S2xx,
        Self::S3xx,
        Self::S4xx,
        Self::S5xx,
        Self::Failed,
        Self::Tunnel,
    ];

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All status",
            Self::S2xx => "2xx",
            Self::S3xx => "3xx",
            Self::S4xx => "4xx",
            Self::S5xx => "5xx",
            Self::Failed => "Failed",
            Self::Tunnel => "Tunnel",
        }
    }

    const fn matches(self, stats: &NodeStats) -> bool {
        match self {
            Self::All => true,
            Self::S2xx => stats.status.s2xx > 0,
            Self::S3xx => stats.status.s3xx > 0,
            Self::S4xx => stats.status.s4xx > 0,
            Self::S5xx => stats.status.s5xx > 0,
            Self::Failed => stats.status.failed > 0,
            Self::Tunnel => stats.status.tunnel > 0 || stats.has_tunnel,
        }
    }
}

/// Coarse Content-Type category filter for URL map nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContentTypeFilter {
    /// Include all Content-Types.
    #[default]
    All,
    /// HTML documents (`text/html`, `application/xhtml+xml`).
    Html,
    /// JSON payloads (`application/json`, `*+json`).
    Json,
    /// JavaScript assets.
    Script,
    /// CSS stylesheets.
    Style,
    /// Image assets (`image/*`).
    Image,
    /// All other Content-Types.
    Other,
}

impl ContentTypeFilter {
    /// All Content-Type filter options.
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::Html,
        Self::Json,
        Self::Script,
        Self::Style,
        Self::Image,
        Self::Other,
    ];

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All types",
            Self::Html => "HTML",
            Self::Json => "JSON",
            Self::Script => "Script",
            Self::Style => "CSS",
            Self::Image => "Image",
            Self::Other => "Other",
        }
    }

    fn matches(self, stats: &NodeStats) -> bool {
        if self == Self::All {
            return true;
        }
        stats
            .content_types
            .keys()
            .any(|essence| classify_content_type(essence) == self)
    }
}

fn classify_content_type(essence: &str) -> ContentTypeFilter {
    if essence == "text/html" || essence == "application/xhtml+xml" {
        ContentTypeFilter::Html
    } else if essence == "application/json" || essence.ends_with("+json") {
        ContentTypeFilter::Json
    } else if essence.contains("javascript") || essence == "text/ecmascript" {
        ContentTypeFilter::Script
    } else if essence == "text/css" {
        ContentTypeFilter::Style
    } else if essence.starts_with("image/") {
        ContentTypeFilter::Image
    } else {
        ContentTypeFilter::Other
    }
}

/// First-party / third-party filter for URL map nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PartyFilter {
    /// Include both first-party and third-party nodes.
    #[default]
    All,
    /// Include only first-party nodes (no third-party hits).
    FirstParty,
    /// Include only nodes observed as third-party dependencies.
    ThirdParty,
}

impl PartyFilter {
    /// All party filter options.
    pub const ALL: [Self; 3] = [Self::All, Self::FirstParty, Self::ThirdParty];

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All parties",
            Self::FirstParty => "1st-party",
            Self::ThirdParty => "3rd-party",
        }
    }

    const fn matches(self, stats: &NodeStats) -> bool {
        match self {
            Self::All => true,
            Self::FirstParty => stats.has_first_party(),
            Self::ThirdParty => stats.is_third_party(),
        }
    }
}

/// Active filter and folding settings for the URL map tree view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeFilter {
    /// Case-insensitive substring filter applied to origin/host names.
    pub host_query: String,
    /// Status distribution filter.
    pub status: StatusFilter,
    /// Content-Type filter.
    pub content_type: ContentTypeFilter,
    /// First-party vs third-party filter.
    pub party: PartyFilter,
    /// Whether sibling ID-like path segments are folded into `{id}`.
    pub fold_ids: bool,
}

impl Default for TreeFilter {
    fn default() -> Self {
        Self {
            host_query: String::new(),
            status: StatusFilter::All,
            content_type: ContentTypeFilter::All,
            party: PartyFilter::All,
            fold_ids: true,
        }
    }
}

/// Projected tree node after optional ID folding, filtering, and sorting.
#[derive(Clone, Debug)]
pub struct ViewNode {
    /// Stable node key across updates.
    pub key: ViewNodeKey,
    /// Display text for this node (percent-decoded for display only).
    pub label: String,
    /// Depth from origin root (`0` for origin root).
    pub depth: usize,
    /// Whether this node is an origin root.
    pub is_origin: bool,
    /// Whether this node represents a folded `{id}` template.
    pub is_folded_id: bool,
    /// Number of raw sibling segments folded into this node (0 if not folded).
    pub folded_count: usize,
    /// Direct hit statistics on this node.
    pub direct: NodeStats,
    /// Subtree aggregate statistics.
    pub subtree: NodeStats,
    /// Newest creation time among raw nodes merged into this view node.
    pub created_at: Instant,
    /// Underlying raw [`NodeId`]s merged into this view node (reused by #69 endpoint schema merging).
    pub raw_node_ids: Vec<NodeId>,
    /// Sorted, filtered child nodes.
    pub children: Vec<Self>,
}

/// Kind of node represented by a [`VisibleRow`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowKind {
    /// Top-level origin root (`scheme://host[:port]`).
    Origin,
    /// Literal path segment.
    Segment,
    /// Folded `{id}` template representing `count` distinct ID segments.
    FoldedId {
        /// Number of distinct raw IDs folded into this row.
        count: usize,
    },
}

/// Expand/collapse state of a [`VisibleRow`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowExpansion {
    /// Row has no visible children.
    Leaf,
    /// Row has children and is currently collapsed.
    Collapsed,
    /// Row has children and is currently expanded.
    Expanded,
}

impl RowExpansion {
    /// Reports whether the row can be expanded or collapsed.
    #[must_use]
    pub const fn is_expandable(self) -> bool {
        !matches!(self, Self::Leaf)
    }

    /// Reports whether the row is currently expanded.
    #[must_use]
    pub const fn is_expanded(self) -> bool {
        matches!(self, Self::Expanded)
    }
}

/// One flattened visible row for virtualized `ScrollArea::show_rows` rendering.
#[derive(Clone, Debug)]
pub struct VisibleRow {
    /// Stable key of this row's node.
    pub key: ViewNodeKey,
    /// Stable key of the parent row, if any (used for Left-arrow navigation).
    pub parent: Option<ViewNodeKey>,
    /// Display label.
    pub label: String,
    /// Nesting depth (`0` for origin root).
    pub depth: usize,
    /// Structural kind of this row.
    pub kind: RowKind,
    /// Whether this row is a leaf, collapsed, or expanded.
    pub expansion: RowExpansion,
    /// Rolled-up subtree statistics displayed inline.
    pub subtree: NodeStats,
    /// Creation timestamp for brief new-node highlight.
    pub created_at: Instant,
}

/// Builds the projected tree roots from `map` applying ID folding, filters, and sorting.
#[must_use]
pub fn build_view_roots(
    map: &UrlMap,
    filter: &TreeFilter,
    sort: TreeSortField,
    direction: SortDirection,
) -> Vec<ViewNode> {
    let host_needle = filter.host_query.trim().to_ascii_lowercase();
    let mut roots = Vec::new();
    for (origin, &root_id) in map.origins() {
        if !host_needle.is_empty()
            && !origin
                .to_string()
                .to_ascii_lowercase()
                .contains(&host_needle)
        {
            continue;
        }
        let Some(root_node) = map.node(root_id) else {
            continue;
        };
        let key = ViewNodeKey {
            origin: origin.clone(),
            path: Vec::new(),
        };
        let spec = MergeSpec {
            key,
            depth: 0,
            folded_count: None,
            fold_ids: filter.fold_ids,
        };
        let mut view = build_merged_node(map, &[root_id], spec);
        view.label = origin.to_string();
        view.is_origin = true;
        view.created_at = root_node.created_at;
        if let Some(filtered) = filter_and_sort_node(view, filter, sort, direction) {
            roots.push(filtered);
        }
    }
    sort_siblings(&mut roots, sort, direction);
    roots
}

/// Flattens `roots` into visible rows according to `is_expanded`.
pub fn flatten_visible(
    roots: &[ViewNode],
    is_expanded: &impl Fn(&ViewNodeKey, usize) -> bool,
) -> Vec<VisibleRow> {
    let mut rows = Vec::new();
    for root in roots {
        push_visible(root, None, is_expanded, &mut rows);
    }
    rows
}

/// Looks up a [`ViewNode`] by its [`ViewNodeKey`] inside projected `roots`.
#[must_use]
pub fn find_view_node<'a>(roots: &'a [ViewNode], key: &ViewNodeKey) -> Option<&'a ViewNode> {
    let mut current = roots.iter().find(|root| root.key.origin == key.origin)?;
    if key.path.is_empty() {
        return Some(current);
    }
    for depth in 1..=key.path.len() {
        let prefix = key.path.get(..depth)?;
        current = current
            .children
            .iter()
            .find(|child| child.key.path.as_slice() == prefix)?;
    }
    Some(current)
}

fn push_visible(
    node: &ViewNode,
    parent: Option<&ViewNodeKey>,
    is_expanded: &impl Fn(&ViewNodeKey, usize) -> bool,
    out: &mut Vec<VisibleRow>,
) {
    let expansion = if node.children.is_empty() {
        RowExpansion::Leaf
    } else if is_expanded(&node.key, node.depth) {
        RowExpansion::Expanded
    } else {
        RowExpansion::Collapsed
    };
    let kind = if node.is_origin {
        RowKind::Origin
    } else if node.is_folded_id {
        RowKind::FoldedId {
            count: node.folded_count,
        }
    } else {
        RowKind::Segment
    };
    out.push(VisibleRow {
        key: node.key.clone(),
        parent: parent.cloned(),
        label: node.label.clone(),
        depth: node.depth,
        kind,
        expansion,
        subtree: node.subtree.clone(),
        created_at: node.created_at,
    });
    if expansion.is_expanded() {
        for child in &node.children {
            push_visible(child, Some(&node.key), is_expanded, out);
        }
    }
}

#[derive(Clone)]
struct MergeSpec {
    key: ViewNodeKey,
    depth: usize,
    folded_count: Option<usize>,
    fold_ids: bool,
}

fn build_single_node(map: &UrlMap, raw_id: NodeId, spec: MergeSpec) -> Option<ViewNode> {
    let raw = map.node(raw_id)?;
    let is_folded_id = spec.folded_count.is_some();
    let folded_count = spec.folded_count.unwrap_or(0);
    let mut direct = raw.direct.clone();
    let mut subtree = raw.subtree.clone();
    if is_folded_id && let Some(kind) = classify_id_segment(&raw.segment) {
        let _ = direct.observed_id_kinds.insert(kind);
        let _ = subtree.observed_id_kinds.insert(kind);
    }
    let children = if raw.children.is_empty() {
        Vec::new()
    } else {
        let mut grouped: BTreeMap<&str, Vec<NodeId>> = BTreeMap::new();
        for (segment, &child_id) in &raw.children {
            grouped.entry(segment.as_str()).or_default().push(child_id);
        }
        build_projected_children(map, &spec.key, spec.depth, spec.fold_ids, grouped)
    };
    let label = spec.key.row_label();
    Some(ViewNode {
        key: spec.key,
        label,
        depth: spec.depth,
        is_origin: spec.depth == 0,
        is_folded_id,
        folded_count,
        direct,
        subtree,
        created_at: raw.created_at,
        raw_node_ids: vec![raw_id],
        children,
    })
}

fn build_merged_node(map: &UrlMap, raw_ids: &[NodeId], spec: MergeSpec) -> ViewNode {
    if let [only_id] = raw_ids
        && let Some(single) = build_single_node(map, *only_id, spec.clone())
    {
        return single;
    }

    let is_folded_id = spec.folded_count.is_some();
    let folded_count = spec.folded_count.unwrap_or(0);
    let mut direct = NodeStats::default();
    let mut subtree = NodeStats::default();
    let mut newest = Instant::now();
    let mut first_time = true;

    let mut children_by_segment: BTreeMap<&str, Vec<NodeId>> = BTreeMap::new();
    for &raw_id in raw_ids {
        let Some(raw) = map.node(raw_id) else {
            continue;
        };
        direct.merge(&raw.direct);
        subtree.merge(&raw.subtree);
        if is_folded_id && let Some(kind) = classify_id_segment(&raw.segment) {
            let _ = direct.observed_id_kinds.insert(kind);
            let _ = subtree.observed_id_kinds.insert(kind);
        }
        if first_time || raw.created_at > newest {
            newest = raw.created_at;
            first_time = false;
        }
        for (segment, &child_id) in &raw.children {
            children_by_segment
                .entry(segment.as_str())
                .or_default()
                .push(child_id);
        }
    }

    let children = build_projected_children(
        map,
        &spec.key,
        spec.depth,
        spec.fold_ids,
        children_by_segment,
    );
    let label = spec.key.row_label();
    ViewNode {
        key: spec.key,
        label,
        depth: spec.depth,
        is_origin: spec.depth == 0,
        is_folded_id,
        folded_count,
        direct,
        subtree,
        created_at: newest,
        raw_node_ids: raw_ids.to_vec(),
        children,
    }
}

fn build_projected_children(
    map: &UrlMap,
    parent_key: &ViewNodeKey,
    parent_depth: usize,
    fold_ids: bool,
    children_by_segment: BTreeMap<&str, Vec<NodeId>>,
) -> Vec<ViewNode> {
    let id_count = if fold_ids {
        children_by_segment
            .keys()
            .filter(|seg| classify_id_segment(seg).is_some())
            .count()
    } else {
        0
    };
    let should_fold = fold_ids && id_count >= ID_FOLD_THRESHOLD;

    let mut children = Vec::with_capacity(children_by_segment.len());
    let mut folded_ids_raw = Vec::new();
    for (segment, child_raw_ids) in children_by_segment {
        if should_fold && classify_id_segment(segment).is_some() {
            folded_ids_raw.extend(child_raw_ids);
            continue;
        }
        let mut child_path = parent_key.path.clone();
        child_path.push(PathSegmentPattern::Literal(segment.to_owned()));
        let child_key = ViewNodeKey {
            origin: parent_key.origin.clone(),
            path: child_path,
        };
        let mut child_view = build_merged_node(
            map,
            &child_raw_ids,
            MergeSpec {
                key: child_key,
                depth: parent_depth + 1,
                folded_count: None,
                fold_ids,
            },
        );
        child_view.label = format!("/{}", percent_decode_display(segment));
        children.push(child_view);
    }

    if !folded_ids_raw.is_empty() {
        let mut folded_path = parent_key.path.clone();
        folded_path.push(PathSegmentPattern::FoldedId);
        let folded_key = ViewNodeKey {
            origin: parent_key.origin.clone(),
            path: folded_path,
        };
        let folded_view = build_merged_node(
            map,
            &folded_ids_raw,
            MergeSpec {
                key: folded_key,
                depth: parent_depth + 1,
                folded_count: Some(id_count),
                fold_ids,
            },
        );
        children.push(folded_view);
    }

    children
}

fn stats_match_filter(stats: &NodeStats, filter: &TreeFilter) -> bool {
    filter.status.matches(stats)
        && filter.content_type.matches(stats)
        && filter.party.matches(stats)
}

fn filter_and_sort_node(
    mut node: ViewNode,
    filter: &TreeFilter,
    sort: TreeSortField,
    direction: SortDirection,
) -> Option<ViewNode> {
    if !stats_match_filter(&node.subtree, filter) {
        return None;
    }
    let had_children = !node.children.is_empty();
    let mut kept_children = Vec::with_capacity(node.children.len());
    for child in node.children {
        if let Some(kept) = filter_and_sort_node(child, filter, sort, direction) {
            kept_children.push(kept);
        }
    }
    if kept_children.is_empty()
        && had_children
        && (node.direct.request_count == 0 || !stats_match_filter(&node.direct, filter))
    {
        return None;
    }
    sort_siblings(&mut kept_children, sort, direction);
    node.children = kept_children;
    Some(node)
}

fn sort_siblings(nodes: &mut [ViewNode], sort: TreeSortField, direction: SortDirection) {
    nodes.sort_by(|left, right| {
        let primary = match sort {
            TreeSortField::Name => left.label.cmp(&right.label),
            TreeSortField::Requests => left.subtree.request_count.cmp(&right.subtree.request_count),
            TreeSortField::Errors => left
                .subtree
                .status
                .errors()
                .cmp(&right.subtree.status.errors()),
            TreeSortField::Bytes => left.subtree.total_bytes.cmp(&right.subtree.total_bytes),
        };
        let ordered = match direction {
            SortDirection::Asc => primary,
            SortDirection::Desc => primary.reverse(),
        };
        if ordered == Ordering::Equal {
            left.label.cmp(&right.label)
        } else {
            ordered
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use http::Method;

    use super::*;
    use crate::url_map::id_fold::IdKind;
    use crate::url_map::normalize::parse_url;
    use crate::url_map::stats::StatusBucket;
    use crate::url_map::tree::ObservedInput;

    fn obs(url: &str, status: StatusBucket, ct: &str, bytes: u64) -> ObservedInput {
        let Some(target) = parse_url(url) else {
            unreachable!("valid test url");
        };
        ObservedInput {
            target,
            method: Method::GET,
            status,
            content_type: Some(ct.to_owned()),
            bytes,
            duration: Some(Duration::from_millis(10)),
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            referer: None,
            origin_header: None,
            redirect_location: None,
        }
    }

    #[test]
    fn folds_sibling_ids_and_merges_descendants_when_enabled() {
        let mut map = UrlMap::new();
        let _ = map.observe(obs(
            "https://api.example.com/users/101/posts",
            StatusBucket::S2xx,
            "application/json",
            100,
        ));
        let _ = map.observe(obs(
            "https://api.example.com/users/202/posts",
            StatusBucket::S4xx,
            "application/json",
            50,
        ));
        let _ = map.observe(obs(
            "https://api.example.com/users/settings",
            StatusBucket::S2xx,
            "application/json",
            25,
        ));

        let folded_roots = build_view_roots(
            &map,
            &TreeFilter {
                fold_ids: true,
                ..TreeFilter::default()
            },
            TreeSortField::Name,
            SortDirection::Asc,
        );
        assert_eq!(folded_roots.len(), 1);
        let Some(users) = folded_roots.first().and_then(|r| r.children.first()) else {
            unreachable!("users node must exist");
        };
        assert_eq!(users.label, "/users");
        let labels: Vec<&str> = users.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["/settings", "/{id}"]);

        let Some(folded) = users.children.iter().find(|c| c.is_folded_id) else {
            unreachable!("folded id node must exist");
        };
        assert_eq!(folded.folded_count, 2);
        assert_eq!(folded.subtree.request_count, 2);
        assert!(folded.subtree.observed_id_kinds.contains(&IdKind::Integer));
        assert_eq!(folded.children.len(), 1);
        assert_eq!(
            folded.children.first().map(|c| c.label.as_str()),
            Some("/posts")
        );
        assert_eq!(
            folded.children.first().map(|c| c.subtree.total_bytes),
            Some(150)
        );

        // Turning off fold_ids shows all raw paths again without losing data.
        let unfolded_roots = build_view_roots(
            &map,
            &TreeFilter {
                fold_ids: false,
                ..TreeFilter::default()
            },
            TreeSortField::Name,
            SortDirection::Asc,
        );
        let Some(unfolded_users) = unfolded_roots.first().and_then(|r| r.children.first()) else {
            unreachable!("users node must exist");
        };
        let raw_labels: Vec<&str> = unfolded_users
            .children
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(raw_labels, ["/101", "/202", "/settings"]);
    }

    #[test]
    fn sorts_and_filters_by_status_type_host_and_party() {
        let mut map = UrlMap::new();
        let _ = map.observe(obs(
            "https://app.example.com/index.html",
            StatusBucket::S2xx,
            "text/html",
            500,
        ));
        let mut third_party_script = obs(
            "https://cdn.other.net/tracker.js",
            StatusBucket::S5xx,
            "application/javascript",
            1_200,
        );
        third_party_script.referer = parse_url("https://app.example.com/index.html");
        let _ = map.observe(third_party_script);

        let by_bytes_desc = build_view_roots(
            &map,
            &TreeFilter::default(),
            TreeSortField::Bytes,
            SortDirection::Desc,
        );
        assert_eq!(
            by_bytes_desc.first().map(|r| r.label.as_str()),
            Some("https://cdn.other.net")
        );

        let only_third_party = build_view_roots(
            &map,
            &TreeFilter {
                party: PartyFilter::ThirdParty,
                ..TreeFilter::default()
            },
            TreeSortField::Name,
            SortDirection::Asc,
        );
        assert_eq!(only_third_party.len(), 1);
        assert_eq!(
            only_third_party.first().map(|r| r.label.as_str()),
            Some("https://cdn.other.net")
        );

        let only_html = build_view_roots(
            &map,
            &TreeFilter {
                content_type: ContentTypeFilter::Html,
                ..TreeFilter::default()
            },
            TreeSortField::Name,
            SortDirection::Asc,
        );
        assert_eq!(only_html.len(), 1);
        assert_eq!(
            only_html.first().map(|r| r.label.as_str()),
            Some("https://app.example.com")
        );
    }

    #[test]
    fn compound_filter_drops_empty_parent_nodes_unless_direct_matches() {
        let mut map = UrlMap::new();
        // Child /a is 200 JSON; child /b is 500 text/plain; root has no direct requests.
        let _ = map.observe(obs(
            "https://api.example.com/a",
            StatusBucket::S2xx,
            "application/json",
            100,
        ));
        let _ = map.observe(obs(
            "https://api.example.com/b",
            StatusBucket::S5xx,
            "text/plain",
            100,
        ));

        let compound = TreeFilter {
            status: StatusFilter::S5xx,
            content_type: ContentTypeFilter::Json,
            ..TreeFilter::default()
        };
        let roots = build_view_roots(&map, &compound, TreeSortField::Name, SortDirection::Asc);
        assert!(
            roots.is_empty(),
            "parent origin must be pruned when no child or direct request satisfies all compound filters"
        );

        // Once the root itself receives a direct 500 JSON response, it should be kept.
        let _ = map.observe(obs(
            "https://api.example.com/",
            StatusBucket::S5xx,
            "application/json",
            50,
        ));
        let roots = build_view_roots(&map, &compound, TreeSortField::Name, SortDirection::Asc);
        assert_eq!(roots.len(), 1);
        assert!(roots.first().is_some_and(|root| root.children.is_empty()));
    }
}
