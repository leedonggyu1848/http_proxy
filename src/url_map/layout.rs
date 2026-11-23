//! Deterministic Fruchterman-Reingold force-directed layout for the domain
//! relationship graph.
//!
//! Layout updates run incrementally with a per-frame iteration cap and only
//! restart when the visible domain topology changes, keeping UI frame times low.

use std::collections::HashMap;
use std::f32::consts::TAU;

use crate::url_map::edges::{EdgeKind, RelationshipGraph};

/// Default maximum number of domains shown in the relationship graph.
pub const DEFAULT_MAX_GRAPH_DOMAINS: usize = 40;
/// Maximum relaxation iterations executed for one layout convergence cycle.
const MAX_TOTAL_STEPS: u32 = 120;
/// Default number of iterations executed per UI frame until settled.
pub const STEPS_PER_FRAME: u32 = 12;

/// Positioned domain node in normalized `[-1.0, 1.0]` coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    /// Lowercase host name.
    pub host: String,
    /// Registrable site (eTLD+1).
    pub site: String,
    /// Total requests targeting this domain.
    pub request_count: u64,
    /// Total error requests (`4xx`/`5xx`/failed).
    pub error_count: u64,
    /// Whether this domain was observed as a third-party dependency.
    pub third_party: bool,
    /// Normalized horizontal coordinate.
    pub x: f32,
    /// Normalized vertical coordinate.
    pub y: f32,
}

/// Directed edge referencing node indices in [`ForceLayout::nodes`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    /// Index of the source node.
    pub from: usize,
    /// Index of the destination node.
    pub to: usize,
    /// Edge source kind.
    pub kind: EdgeKind,
    /// Number of observed requests along this edge.
    pub count: u64,
    /// Whether this edge crosses registrable site boundaries.
    pub third_party: bool,
}

/// Incremental force-directed layout state for the domain relationship graph.
#[derive(Clone, Debug)]
pub struct ForceLayout {
    topology_version: u64,
    max_domains: usize,
    total_domains: usize,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    steps_done: u32,
    settled: bool,
}

impl Default for ForceLayout {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_GRAPH_DOMAINS)
    }
}

impl ForceLayout {
    /// Creates an empty layout capped at `max_domains` top domains.
    #[must_use]
    pub const fn new(max_domains: usize) -> Self {
        Self {
            topology_version: u64::MAX,
            max_domains,
            total_domains: 0,
            nodes: Vec::new(),
            edges: Vec::new(),
            steps_done: 0,
            settled: true,
        }
    }

    /// Returns positioned graph nodes.
    #[must_use]
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    /// Returns directed graph edges between indices of [`Self::nodes`].
    #[must_use]
    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    /// Total number of domains present in the underlying map before top-N capping.
    #[must_use]
    pub const fn total_domains(&self) -> usize {
        self.total_domains
    }

    /// Maximum number of domains rendered simultaneously.
    #[must_use]
    pub const fn max_domains(&self) -> usize {
        self.max_domains
    }

    /// Updates the domain cap, triggering a rebuild on the next sync.
    pub const fn set_max_domains(&mut self, max_domains: usize) {
        if self.max_domains != max_domains {
            self.max_domains = max_domains;
            self.topology_version = u64::MAX;
        }
    }

    /// Reports whether the layout has finished relaxing.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.settled
    }

    fn refresh_counts(&mut self, graph: &RelationshipGraph) {
        for node in &mut self.nodes {
            if let Some(summary) = graph.domains().get(&node.host) {
                node.request_count = summary.request_count;
                node.error_count = summary.error_count;
                node.third_party = summary.is_third_party();
            }
        }
        for edge in &mut self.edges {
            if let (Some(from), Some(to)) = (self.nodes.get(edge.from), self.nodes.get(edge.to)) {
                let key = crate::url_map::edges::DomainEdgeKey {
                    from_host: from.host.clone(),
                    to_host: to.host.clone(),
                    kind: edge.kind,
                };
                if let Some(stats) = graph.domain_edges().get(&key) {
                    edge.count = stats.count;
                }
            }
        }
    }

    /// Synchronizes nodes and edges with `graph`.
    ///
    /// Recomputes node positions when `graph.topology_version()` changes or when
    /// Top-N membership shifts among domains exceeding `max_domains`. Existing
    /// node positions are preserved across updates for visual stability.
    pub fn sync(&mut self, graph: &RelationshipGraph) {
        self.total_domains = graph.domains().len();
        let top_n_changed = self.total_domains > self.max_domains
            && top_n_membership_changed(&self.nodes, graph, self.max_domains);
        if self.topology_version == graph.topology_version() && !top_n_changed {
            self.refresh_counts(graph);
            return;
        }

        let previous_positions: HashMap<String, (f32, f32)> = self
            .nodes
            .iter()
            .map(|node| (node.host.clone(), (node.x, node.y)))
            .collect();

        let mut ranked: Vec<_> = graph.domains().values().collect();
        ranked.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| left.host.cmp(&right.host))
        });
        ranked.truncate(self.max_domains.max(1));
        ranked.sort_by(|left, right| left.host.cmp(&right.host));

        let count = ranked.len();
        let mut host_to_index = HashMap::with_capacity(count);
        let mut nodes = Vec::with_capacity(count);
        for (index, summary) in ranked.into_iter().enumerate() {
            let (x, y) = previous_positions
                .get(&summary.host)
                .copied()
                .unwrap_or_else(|| initial_circle_position(index, count));
            let _ = host_to_index.insert(summary.host.clone(), index);
            nodes.push(GraphNode {
                host: summary.host.clone(),
                site: summary.site.clone(),
                request_count: summary.request_count,
                error_count: summary.error_count,
                third_party: summary.is_third_party(),
                x,
                y,
            });
        }

        let mut edges = Vec::new();
        for (key, stats) in graph.domain_edges() {
            if let (Some(&from), Some(&to)) = (
                host_to_index.get(&key.from_host),
                host_to_index.get(&key.to_host),
            ) {
                if from == to && key.kind != EdgeKind::Redirect {
                    continue;
                }
                edges.push(GraphEdge {
                    from,
                    to,
                    kind: key.kind,
                    count: stats.count,
                    third_party: stats.third_party,
                });
            }
        }

        self.topology_version = graph.topology_version();
        self.nodes = nodes;
        self.edges = edges;
        self.steps_done = 0;
        self.settled = self.nodes.len() <= 1;
    }

    /// Runs up to `max_steps` force-directed relaxation iterations.
    ///
    /// Returns `true` if positions moved and another repaint should be scheduled.
    pub fn step(&mut self, max_steps: u32) -> bool {
        if self.settled || self.nodes.len() <= 1 {
            self.settled = true;
            return false;
        }
        let mut moved = false;
        for _ in 0..max_steps {
            if self.steps_done >= MAX_TOTAL_STEPS {
                self.settled = true;
                break;
            }
            let max_displacement = self.relax_once();
            self.steps_done += 1;
            moved = true;
            if max_displacement < 0.002 {
                self.settled = true;
                break;
            }
        }
        moved
    }

    /// Allows dragging one node to a normalized coordinate.
    pub fn move_node(&mut self, index: usize, x: f32, y: f32) {
        if let Some(node) = self.nodes.get_mut(index) {
            node.x = x.clamp(-0.95, 0.95);
            node.y = y.clamp(-0.95, 0.95);
        }
    }

    #[allow(clippy::cast_precision_loss, reason = "small node count for 2D layout")]
    fn relax_once(&mut self) -> f32 {
        let count = self.nodes.len();
        let ideal = (1.6 / (count as f32).sqrt()).clamp(0.18, 0.75);
        let cooling = 1.0 - (self.steps_done as f32 / MAX_TOTAL_STEPS as f32);
        let temperature = 0.16 * cooling.max(0.05);

        let mut disp_x = vec![0.0_f32; count];
        let mut disp_y = vec![0.0_f32; count];

        // Pairwise repulsion between all visible nodes.
        for i in 0..count {
            let Some(a) = self.nodes.get(i) else {
                continue;
            };
            for j in (i + 1)..count {
                let Some(b) = self.nodes.get(j) else {
                    continue;
                };
                let dx = a.x - b.x;
                let dy = a.y - b.y;
                let dist = dx.hypot(dy).max(0.01);
                let force = (ideal * ideal) / dist;
                let fx = (dx / dist) * force;
                let fy = (dy / dist) * force;
                if let (Some(ax), Some(ay)) = (disp_x.get_mut(i), disp_y.get_mut(i)) {
                    *ax += fx;
                    *ay += fy;
                }
                if let (Some(bx), Some(by)) = (disp_x.get_mut(j), disp_y.get_mut(j)) {
                    *bx -= fx;
                    *by -= fy;
                }
            }
        }

        // Spring attraction along edges.
        for edge in &self.edges {
            if edge.from == edge.to {
                continue;
            }
            let (Some(a), Some(b)) = (self.nodes.get(edge.from), self.nodes.get(edge.to)) else {
                continue;
            };
            let dx = a.x - b.x;
            let dy = a.y - b.y;
            let dist = dx.hypot(dy).max(0.01);
            let force = (dist * dist) / ideal;
            let fx = (dx / dist) * force;
            let fy = (dy / dist) * force;
            if let (Some(ax), Some(ay)) = (disp_x.get_mut(edge.from), disp_y.get_mut(edge.from)) {
                *ax -= fx;
                *ay -= fy;
            }
            if let (Some(bx), Some(by)) = (disp_x.get_mut(edge.to), disp_y.get_mut(edge.to)) {
                *bx += fx;
                *by += fy;
            }
        }

        // Gentle central gravity + clamped displacement update.
        let mut max_step = 0.0_f32;
        for (i, node) in self.nodes.iter_mut().enumerate() {
            let gx = -node.x * 0.12;
            let gy = -node.y * 0.12;
            let dx = disp_x.get(i).copied().unwrap_or(0.0) + gx;
            let dy = disp_y.get(i).copied().unwrap_or(0.0) + gy;
            let length = dx.hypot(dy);
            if length > f32::EPSILON {
                let step = length.min(temperature);
                node.x = (dx / length).mul_add(step, node.x).clamp(-0.9, 0.9);
                node.y = (dy / length).mul_add(step, node.y).clamp(-0.9, 0.9);
                max_step = max_step.max(step);
            }
        }
        max_step
    }
}

fn top_n_membership_changed(
    current: &[GraphNode],
    graph: &RelationshipGraph,
    max_domains: usize,
) -> bool {
    let limit = max_domains.max(1);
    let mut ranked: Vec<_> = graph.domains().values().collect();
    ranked.sort_by(|left, right| {
        right
            .request_count
            .cmp(&left.request_count)
            .then_with(|| left.host.cmp(&right.host))
    });
    ranked.truncate(limit);
    ranked.sort_by(|left, right| left.host.cmp(&right.host));

    ranked.len() != current.len()
        || ranked
            .iter()
            .zip(current.iter())
            .any(|(summary, node)| summary.host != node.host)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "small node index for circle angle"
)]
fn initial_circle_position(index: usize, count: usize) -> (f32, f32) {
    if count <= 1 {
        return (0.0, 0.0);
    }
    let angle = TAU * (index as f32) / (count as f32);
    let radius = 0.65;
    (radius * angle.cos(), radius * angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url_map::normalize::OriginKey;

    #[test]
    fn caps_top_n_domains_and_settles_deterministically() {
        let mut graph = RelationshipGraph::default();
        for i in 0..15_u32 {
            let host = format!("host-{i:02}.example.com");
            let origin = OriginKey::new("https", &host, None);
            for _ in 0..=i {
                graph.record_request(&origin, i > 7);
            }
            if i > 0 {
                let prev = format!("host-{:02}.example.com", i - 1);
                graph.record_edge(
                    &prev,
                    &host,
                    format!("https://{prev}/"),
                    format!("https://{host}/"),
                    EdgeKind::Referer,
                );
            }
        }

        let mut layout = ForceLayout::new(8);
        layout.sync(&graph);
        assert_eq!(layout.total_domains(), 15);
        assert_eq!(layout.nodes().len(), 8);
        assert!(!layout.is_settled());

        let mut rounds = 0;
        while !layout.is_settled() && rounds < 20 {
            let _ = layout.step(STEPS_PER_FRAME);
            rounds += 1;
        }
        assert!(layout.is_settled());
        for node in layout.nodes() {
            assert!(node.x.is_finite() && node.x.abs() <= 0.95);
            assert!(node.y.is_finite() && node.y.abs() <= 0.95);
        }
    }

    #[test]
    fn updates_top_n_when_existing_domain_surpasses_shown_nodes_without_topology_change() {
        let mut graph = RelationshipGraph::default();
        let a = OriginKey::new("https", "a.example.com", None);
        let b = OriginKey::new("https", "b.example.com", None);
        let c = OriginKey::new("https", "c.example.com", None);
        for _ in 0..5 {
            graph.record_request(&a, false);
        }
        for _ in 0..3 {
            graph.record_request(&b, false);
        }
        graph.record_request(&c, false);

        let mut layout = ForceLayout::new(2);
        layout.sync(&graph);
        let topo_before = graph.topology_version();
        let hosts_before: Vec<&str> = layout.nodes().iter().map(|n| n.host.as_str()).collect();
        assert_eq!(hosts_before, vec!["a.example.com", "b.example.com"]);

        // Traffic surges on `c.example.com` without adding any new host or edge.
        for _ in 0..10 {
            graph.record_request(&c, false);
        }
        assert_eq!(graph.topology_version(), topo_before);

        layout.sync(&graph);
        let hosts_after: Vec<&str> = layout.nodes().iter().map(|n| n.host.as_str()).collect();
        assert_eq!(hosts_after, vec!["a.example.com", "c.example.com"]);
    }
}
