//! Interactive force-directed domain relationship graph rendered directly with
//! [`egui::Painter`].

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use http_inspector_analysis::url_map::{
    EdgeKind, ForceLayout, GraphEdge, GraphNode, RelationshipGraph, STEPS_PER_FRAME,
};
use http_inspector_exchange::ByteSize;

use super::EdgeVisibility;
use crate::ui::style::theme::Palette;

const CANVAS_PADDING: f32 = 56.0;
const MIN_NODE_RADIUS: f32 = 12.0;
const MAX_NODE_RADIUS: f32 = 28.0;
const ARROW_LENGTH: f32 = 9.0;
const ARROW_HALF_WIDTH: f32 = 4.5;

/// Renders the relationship graph and returns the clicked host (if any) to jump
/// to in Tree view.
pub(super) fn show(
    ui: &mut egui::Ui,
    graph: &RelationshipGraph,
    layout: &mut ForceLayout,
    visibility: EdgeVisibility,
    dragged_node: &mut Option<usize>,
) -> Option<String> {
    layout.sync(graph);
    if !layout.is_settled() && layout.step(STEPS_PER_FRAME) {
        ui.ctx().request_repaint();
    }

    let palette = Palette::of(ui);
    render_legend(ui, layout, visibility, &palette);

    if layout.nodes().is_empty() {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "No domain relationships yet.\nPages loading scripts, APIs, CORS requests, or redirects appear here.",
                    )
                    .weak(),
                )
                .wrap(),
            );
        });
        return None;
    }

    let available = ui.available_size().max(Vec2::new(120.0, 120.0));
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let max_requests = layout
        .nodes()
        .iter()
        .map(|node| node.request_count)
        .max()
        .unwrap_or(1)
        .max(1);
    let positions: Vec<(Pos2, f32)> = layout
        .nodes()
        .iter()
        .map(|node| {
            (
                to_screen(rect, node.x, node.y),
                node_radius(node, max_requests),
            )
        })
        .collect();

    let hovered = response
        .hover_pos()
        .and_then(|pos| hit_test(&positions, pos));

    handle_drag(layout, rect, &response, &positions, dragged_node);

    let clicked_host = if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(idx) = hit_test(&positions, pos)
    {
        layout.nodes().get(idx).map(|node| node.host.clone())
    } else {
        None
    };

    paint_edges(&painter, layout.edges(), &positions, visibility, &palette);
    paint_nodes(
        &NodeCanvas {
            painter: &painter,
            visuals: ui.visuals(),
            rect,
            palette: &palette,
        },
        layout.nodes(),
        &positions,
        hovered,
    );

    if let Some(idx) = hovered
        && let Some(node) = layout.nodes().get(idx)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let tooltip = format_node_tooltip(graph, node);
        let _ = response.on_hover_text(tooltip);
    }

    clicked_host
}

fn legend_summary_text(layout: &ForceLayout, visibility: EdgeVisibility) -> (String, bool) {
    let shown = layout.nodes().len();
    let total = layout.total_domains();
    let total_edges = layout.edges().len();
    let visible_edges = layout
        .edges()
        .iter()
        .filter(|edge| visibility.allows(edge.kind))
        .count();
    let edge_text = if visible_edges == total_edges {
        format!("{total_edges} edges")
    } else {
        format!("{visible_edges}/{total_edges} edges")
    };
    if total > shown {
        (
            format!("Showing top {shown} of {total} domains · {edge_text}"),
            true,
        )
    } else {
        (format!("{shown} domains · {edge_text}"), false)
    }
}

fn render_legend(
    ui: &mut egui::Ui,
    layout: &ForceLayout,
    visibility: EdgeVisibility,
    palette: &Palette,
) {
    let chip = |ui: &mut egui::Ui, text: egui::RichText| {
        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
    };
    let (summary, capped) = legend_summary_text(layout, visibility);
    let summary_color = if capped {
        palette.warning
    } else {
        palette.muted
    };
    ui.horizontal_wrapped(|ui| {
        chip(
            ui,
            egui::RichText::new(summary).color(summary_color).small(),
        );
        ui.separator();
        chip(
            ui,
            egui::RichText::new("1st-party (solid)")
                .color(palette.info)
                .small(),
        );
        chip(
            ui,
            egui::RichText::new("3rd-party (dashed)")
                .color(palette.warning)
                .small(),
        );
        ui.separator();
        chip(
            ui,
            egui::RichText::new("Referer (solid)")
                .color(palette.info)
                .small(),
        );
        chip(
            ui,
            egui::RichText::new("Origin (dashed)")
                .color(palette.warning)
                .small(),
        );
        chip(
            ui,
            egui::RichText::new("Redirect (thick)")
                .color(palette.success)
                .small(),
        );
        ui.separator();
        chip(
            ui,
            egui::RichText::new("Click a domain to open Tree view.")
                .color(palette.muted)
                .small(),
        );
    });
}

fn handle_drag(
    layout: &mut ForceLayout,
    rect: Rect,
    response: &egui::Response,
    positions: &[(Pos2, f32)],
    dragged_node: &mut Option<usize>,
) {
    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos()
    {
        *dragged_node = hit_test(positions, pos);
    }
    if response.dragged()
        && let Some(idx) = *dragged_node
        && let Some(pos) = response.interact_pointer_pos()
    {
        let (nx, ny) = from_screen(rect, pos);
        layout.move_node(idx, nx, ny);
    }
    if response.drag_stopped() {
        *dragged_node = None;
    }
}

fn paint_edges(
    painter: &egui::Painter,
    edges: &[GraphEdge],
    positions: &[(Pos2, f32)],
    visibility: EdgeVisibility,
    palette: &Palette,
) {
    let visible: Vec<&GraphEdge> = edges
        .iter()
        .filter(|edge| visibility.allows(edge.kind))
        .collect();
    for (idx, edge) in visible.iter().enumerate() {
        let (Some(&(from_pos, from_r)), Some(&(to_pos, to_r))) =
            (positions.get(edge.from), positions.get(edge.to))
        else {
            continue;
        };
        let (color, width, dashed) = match edge.kind {
            EdgeKind::Referer => (palette.info, 1.6, false),
            EdgeKind::Origin => (palette.warning, 1.6, true),
            EdgeKind::Redirect => (palette.success, 2.6, false),
        };
        if edge.from == edge.to {
            let loop_center = Pos2::new(from_pos.x, from_pos.y - from_r - 12.0);
            painter.circle_stroke(loop_center, 10.0, Stroke::new(width, color));
            continue;
        }
        let delta = to_pos - from_pos;
        let distance = delta.length();
        if distance <= from_r + to_r + 6.0 {
            continue;
        }
        let dir = delta / distance;
        let perp = Vec2::new(-dir.y, dir.x);

        // Separate parallel edges between the same unordered node pair so every
        // edge kind (Referer, Origin, Redirect) remains visible simultaneously.
        let pair = (edge.from.min(edge.to), edge.from.max(edge.to));
        let siblings = visible
            .iter()
            .filter(|other| (other.from.min(other.to), other.from.max(other.to)) == pair)
            .count();
        let rank = visible
            .iter()
            .take(idx)
            .filter(|other| (other.from.min(other.to), other.from.max(other.to)) == pair)
            .count();
        #[allow(clippy::cast_precision_loss, reason = "small count of parallel edges")]
        let shift = (siblings.saturating_sub(1) as f32).mul_add(-0.5, rank as f32) * 9.0;
        let offset = perp * shift;

        let start = from_pos + dir * (from_r + 2.0) + offset;
        let end = to_pos - dir * (to_r + 3.0) + offset;

        if dashed {
            paint_dashed_line(painter, start, end, Stroke::new(width, color));
        } else {
            painter.line_segment([start, end], Stroke::new(width, color));
        }
        paint_arrowhead(painter, end, dir, color);
    }
}

fn paint_dashed_line(painter: &egui::Painter, start: Pos2, end: Pos2, stroke: Stroke) {
    const DASH: f32 = 6.0;
    const GAP: f32 = 4.0;
    const STRIDE: f32 = DASH + GAP;
    let delta = end - start;
    let total = delta.length();
    if total <= f32::EPSILON {
        return;
    }
    let dir = delta / total;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive segment count along finite screen line"
    )]
    let steps = (total / STRIDE).ceil() as usize;
    for step in 0..steps {
        #[allow(
            clippy::cast_precision_loss,
            reason = "dash index along finite screen line"
        )]
        let t = step as f32 * STRIDE;
        let seg_end = (t + DASH).min(total);
        painter.line_segment([start + dir * t, start + dir * seg_end], stroke);
    }
}

fn paint_arrowhead(painter: &egui::Painter, tip: Pos2, dir: Vec2, color: Color32) {
    let perp = Vec2::new(-dir.y, dir.x);
    let base = tip - dir * ARROW_LENGTH;
    let left = base + perp * ARROW_HALF_WIDTH;
    let right = base - perp * ARROW_HALF_WIDTH;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        color,
        Stroke::NONE,
    ));
}

struct NodeCanvas<'a> {
    painter: &'a egui::Painter,
    visuals: &'a egui::Visuals,
    rect: Rect,
    palette: &'a Palette,
}

fn paint_nodes(
    canvas: &NodeCanvas<'_>,
    nodes: &[GraphNode],
    positions: &[(Pos2, f32)],
    hovered: Option<usize>,
) {
    let painter = canvas.painter;
    let palette = canvas.palette;
    let fill = canvas.visuals.panel_fill;
    let hover_fill = canvas.visuals.widgets.hovered.bg_fill;
    let text_color = canvas.visuals.text_color();
    let label_font = FontId::proportional(11.5);
    let count_font = FontId::monospace(10.5);
    let mut placed_labels: Vec<Rect> = Vec::with_capacity(nodes.len());

    for (index, (node, &(center, radius))) in nodes.iter().zip(positions.iter()).enumerate() {
        let is_hovered = hovered == Some(index);
        let node_fill = if is_hovered { hover_fill } else { fill };
        painter.circle_filled(center, radius, node_fill);

        if node.third_party {
            paint_dashed_circle(painter, center, radius, Stroke::new(2.5, palette.warning));
        } else {
            let stroke_width = if is_hovered { 3.0 } else { 2.0 };
            painter.circle_stroke(center, radius, Stroke::new(stroke_width, palette.info));
        }

        // Numeric request count inside the node circle.
        let _ = painter.text(
            center,
            Align2::CENTER_CENTER,
            node.request_count.to_string(),
            count_font.clone(),
            text_color,
        );

        let tag = if node.third_party {
            format!("{} [3rd]", node.host)
        } else {
            node.host.clone()
        };
        let galley = painter.layout_no_wrap(tag, label_font.clone(), text_color);
        let label_size = galley.size();
        let half_w = label_size.x * 0.5;
        let label_x = center.x.clamp(
            canvas.rect.left() + half_w + 4.0,
            canvas.rect.right() - half_w - 4.0,
        );
        let preferred_y = center.y + radius + 4.0;
        let label_y = if preferred_y + label_size.y <= canvas.rect.bottom() - 4.0 {
            preferred_y
        } else {
            (center.y - radius - label_size.y - 4.0).max(canvas.rect.top() + 4.0)
        };
        let label_rect = Rect::from_min_size(Pos2::new(label_x - half_w, label_y), label_size);

        // Skip secondary overlapping labels unless this node is hovered, preventing overlap.
        let overlaps = placed_labels
            .iter()
            .any(|existing| existing.expand(2.0).intersects(label_rect));
        if !overlaps || is_hovered {
            painter.rect_filled(label_rect.expand2(Vec2::new(3.0, 1.0)), 2.0, fill);
            painter.galley(label_rect.min, galley, text_color);
            placed_labels.push(label_rect);
        }
    }
}

fn paint_dashed_circle(painter: &egui::Painter, center: Pos2, radius: f32, stroke: Stroke) {
    const SEGMENTS: usize = 16;
    let step = std::f32::consts::TAU / 16.0;
    for i in 0..SEGMENTS {
        if i % 2 == 1 {
            continue;
        }
        #[allow(clippy::cast_precision_loss, reason = "small fixed dash segment index")]
        let a0 = i as f32 * step;
        let a1 = step.mul_add(0.8, a0);
        let p0 = Pos2::new(
            radius.mul_add(a0.cos(), center.x),
            radius.mul_add(a0.sin(), center.y),
        );
        let p1 = Pos2::new(
            radius.mul_add(a1.cos(), center.x),
            radius.mul_add(a1.sin(), center.y),
        );
        painter.line_segment([p0, p1], stroke);
    }
}

fn to_screen(rect: Rect, nx: f32, ny: f32) -> Pos2 {
    let half_w = rect.width().mul_add(0.5, -CANVAS_PADDING).max(20.0);
    let half_h = rect.height().mul_add(0.5, -CANVAS_PADDING).max(20.0);
    let center = rect.center();
    Pos2::new(nx.mul_add(half_w, center.x), ny.mul_add(half_h, center.y))
}

fn from_screen(rect: Rect, pos: Pos2) -> (f32, f32) {
    let half_w = rect.width().mul_add(0.5, -CANVAS_PADDING).max(20.0);
    let half_h = rect.height().mul_add(0.5, -CANVAS_PADDING).max(20.0);
    let center = rect.center();
    ((pos.x - center.x) / half_w, (pos.y - center.y) / half_h)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "node radius scaling from request count"
)]
fn node_radius(node: &GraphNode, max_requests: u64) -> f32 {
    let numerator = (node.request_count.max(1) as f32).ln_1p();
    let denominator = (max_requests.max(1) as f32).ln_1p().max(1.0);
    let t = (numerator / denominator).clamp(0.0, 1.0);
    t.mul_add(MAX_NODE_RADIUS - MIN_NODE_RADIUS, MIN_NODE_RADIUS)
}

fn hit_test(positions: &[(Pos2, f32)], pointer: Pos2) -> Option<usize> {
    positions
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (center, radius))| center.distance(pointer) <= radius + 4.0)
        .map(|(index, _)| index)
}

fn format_node_tooltip(graph: &RelationshipGraph, node: &GraphNode) -> String {
    let party = if node.third_party {
        "3rd-party"
    } else {
        "1st-party"
    };
    let bytes = graph
        .domains()
        .get(&node.host)
        .map_or(0, |summary| summary.total_bytes);
    let outgoing = graph
        .domain_edges()
        .keys()
        .filter(|key| key.from_host == node.host)
        .count();
    let incoming = graph
        .domain_edges()
        .keys()
        .filter(|key| key.to_host == node.host)
        .count();
    format!(
        "{}\nSite (eTLD+1): {}\nParty: {party}\nRequests: {} · Errors: {} · Size: {}\nEdges: {outgoing} outgoing, {incoming} incoming\nClick to open in Tree view",
        node.host,
        node.site,
        node.request_count,
        node.error_count,
        ByteSize::new(bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_inspector_analysis::url::OriginKey;

    #[test]
    fn legend_reflects_filtered_edge_count() {
        let mut graph = RelationshipGraph::default();
        let a = OriginKey::new("https", "app.example.com", None);
        let b = OriginKey::new("https", "cdn.other.net", None);
        graph.record_request(&a, false);
        graph.record_request(&b, true);
        graph.record_edge(
            "app.example.com",
            "cdn.other.net",
            "https://app.example.com/".to_owned(),
            "https://cdn.other.net/a.js".to_owned(),
            EdgeKind::Referer,
        );
        graph.record_edge(
            "app.example.com",
            "cdn.other.net",
            "https://app.example.com/".to_owned(),
            "https://cdn.other.net/a.js".to_owned(),
            EdgeKind::Origin,
        );

        let mut layout = ForceLayout::new(40);
        layout.sync(&graph);

        let (all_text, capped) = legend_summary_text(&layout, EdgeVisibility::default());
        assert!(!capped);
        assert_eq!(all_text, "2 domains · 2 edges");

        let only_referer = EdgeVisibility {
            referer: true,
            origin: false,
            redirect: false,
        };
        let (filtered_text, _) = legend_summary_text(&layout, only_referer);
        assert_eq!(filtered_text, "2 domains · 1/2 edges");
    }
}
