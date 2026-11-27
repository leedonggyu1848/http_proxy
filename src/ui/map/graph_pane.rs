//! Interactive force-directed domain relationship graph rendered directly with
//! [`egui::Painter`].

use super::EdgeVisibility;
use crate::ui::i18n::{self, Language};
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens};
use crate::ui::style::{fonts, theme::Palette};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use http_inspector_analysis::url_map::{
    EdgeKind, ForceLayout, GraphEdge, GraphNode, RelationshipGraph, STEPS_PER_FRAME,
};

/// Inputs describing how to render the relationship graph.
pub(super) struct GraphView<'a> {
    pub(super) graph: &'a RelationshipGraph,
    pub(super) visibility: EdgeVisibility,
    pub(super) selected_host: Option<&'a str>,
    pub(super) lang: Language,
}

/// Renders the relationship graph and returns the clicked host (if any) to jump
/// to in Tree view. `selected_host` is the host of the node selected in the
/// tree, drawn as active with the hovered node.
pub(super) fn show(
    ui: &mut egui::Ui,
    view: &GraphView<'_>,
    layout: &mut ForceLayout,
    dragged_node: &mut Option<usize>,
) -> Option<String> {
    let GraphView {
        graph,
        visibility,
        selected_host,
        lang,
    } = *view;
    layout.sync(graph);
    if !layout.is_settled() && layout.step(STEPS_PER_FRAME) {
        ui.ctx().request_repaint();
    }

    let palette = Palette::of(ui);
    if layout.nodes().is_empty() {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(lang.map().no_relationships).weak()).wrap(),
            );
        });
        return None;
    }

    let available = ui
        .available_size()
        .max(Vec2::splat(Layout::GRAPH_CANVAS_MIN));
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

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

    let active: Vec<bool> = layout
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| hovered == Some(index) || selected_host == Some(node.host.as_str()))
        .collect();
    paint_edges(&painter, layout.edges(), &positions, visibility, &active);
    paint_nodes(
        &NodeCanvas {
            painter: &painter,
            rect,
            palette: &palette,
        },
        layout.nodes(),
        &positions,
        &active,
    );

    if let Some(idx) = hovered
        && let Some(node) = layout.nodes().get(idx)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let tooltip = format_node_tooltip(graph, node, lang);
        let _ = response.on_hover_text(tooltip);
    }

    clicked_host
}

/// Domains shown, edges shown, and whether the domain limit hid some, for
/// the status bar.
pub(super) fn summary_text(
    layout: &ForceLayout,
    visibility: EdgeVisibility,
    lang: Language,
) -> (String, bool) {
    let shown = layout.nodes().len();
    let total = layout.total_domains();
    let total_edges = layout.edges().len();
    let visible_edges = layout
        .edges()
        .iter()
        .filter(|edge| visibility.allows(edge.kind))
        .count();
    (
        i18n::toolbar::graph_count(lang, shown, total, visible_edges, total_edges),
        total > shown,
    )
}

/// How an edge kind is drawn when it is not on the active path: Referer
/// solid, Origin dashed 3-3, Redirect dotted (D19).
pub(super) const fn edge_dash(kind: EdgeKind) -> Option<(f32, f32)> {
    match kind {
        EdgeKind::Referer => None,
        EdgeKind::Origin => Some((Layout::GRAPH_DASH, Layout::GRAPH_DASH)),
        EdgeKind::Redirect => Some((StrokeWidth::GRAPH_ACTIVE, Layout::GRAPH_DASH)),
    }
}

/// Stroke of an inactive graph edge and its swatch in [`super::legend_pane`].
pub(super) const fn inactive_edge_stroke(tokens: &ThemeTokens) -> Stroke {
    Stroke {
        width: StrokeWidth::GRAPH_EDGE,
        color: tokens.text.muted,
    }
}

/// Stroke of an inactive node outline and its swatch in [`super::legend_pane`].
pub(super) const fn inactive_node_stroke(tokens: &ThemeTokens) -> Stroke {
    Stroke {
        width: StrokeWidth::STRUCTURAL,
        color: tokens.text.muted,
    }
}

/// Stroke of an active node outline and edges touching an active node (D19).
pub(super) const fn active_stroke(tokens: &ThemeTokens) -> Stroke {
    Stroke {
        width: StrokeWidth::GRAPH_ACTIVE,
        color: tokens.accent.primary,
    }
}

/// Edge kind visibility toggles in the graph's **Edges ▼** menu.
///
/// Swatches and descriptions live in [`super::legend_pane`] so the menu and
/// the legend panel do not repeat the same samples (S15).
pub(super) fn edges_menu(ui: &mut egui::Ui, visibility: &mut EdgeVisibility, lang: Language) {
    let text = lang.map();
    for (shown, label) in [
        (&mut visibility.referer, text.referer),
        (&mut visibility.origin, text.origin_cors),
        (&mut visibility.redirect, text.redirect),
    ] {
        ui.checkbox(shown, label);
    }
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

/// Edges touching an active node are 1.5px `accent.primary` solid lines;
/// every other edge is a 1.25px `text.muted` line in its kind's dash
/// pattern (D19).
fn paint_edges(
    painter: &egui::Painter,
    edges: &[GraphEdge],
    positions: &[(Pos2, f32)],
    visibility: EdgeVisibility,
    active: &[bool],
) {
    let tokens = ThemeTokens::for_theme(painter.ctx().theme());
    let visible: Vec<&GraphEdge> = edges
        .iter()
        .filter(|edge| visibility.allows(edge.kind))
        .collect();
    let is_active = |edge: &GraphEdge| {
        active.get(edge.from).copied().unwrap_or_default()
            || active.get(edge.to).copied().unwrap_or_default()
    };
    // Active edges are drawn last so that they stay on top.
    let mut order: Vec<usize> = (0..visible.len()).collect();
    order.sort_by_key(|&idx| visible.get(idx).is_some_and(|edge| is_active(edge)));
    for idx in order {
        let Some(edge) = visible.get(idx) else {
            continue;
        };
        let (Some(&(from_pos, from_r)), Some(&(to_pos, to_r))) =
            (positions.get(edge.from), positions.get(edge.to))
        else {
            continue;
        };
        let (stroke, dash) = if is_active(edge) {
            (active_stroke(&tokens), None)
        } else {
            (inactive_edge_stroke(&tokens), edge_dash(edge.kind))
        };
        if edge.from == edge.to {
            let loop_center =
                Pos2::new(from_pos.x, from_pos.y - from_r - Layout::GRAPH_LOOP_OFFSET);
            painter.circle_stroke(loop_center, Layout::GRAPH_LOOP_RADIUS, stroke);
            continue;
        }
        let delta = to_pos - from_pos;
        let distance = delta.length();
        if distance <= from_r + to_r + Layout::GRAPH_EDGE_CLEARANCE {
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
        let shift = (siblings.saturating_sub(1) as f32).mul_add(-0.5, rank as f32)
            * Layout::GRAPH_EDGE_SEPARATION;
        let offset = perp * shift;

        let start = from_pos + dir * (from_r + Space::XXS) + offset;
        let end = to_pos - dir * (to_r + Layout::GRAPH_ARROW_TIP_PADDING) + offset;

        match dash {
            Some((dash, gap)) => paint_dashed_line(painter, start, end, stroke, dash, gap),
            None => {
                painter.line_segment([start, end], stroke);
            }
        }
        paint_arrowhead(painter, end, dir, stroke.color);
    }
}

pub(super) fn paint_dashed_line(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    let stride = dash + gap;
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
    let steps = (total / stride).ceil() as usize;
    for step in 0..steps {
        #[allow(
            clippy::cast_precision_loss,
            reason = "dash index along finite screen line"
        )]
        let t = step as f32 * stride;
        let seg_end = (t + dash).min(total);
        painter.line_segment([start + dir * t, start + dir * seg_end], stroke);
    }
}

pub(super) fn paint_arrowhead(painter: &egui::Painter, tip: Pos2, dir: Vec2, color: Color32) {
    let perp = Vec2::new(-dir.y, dir.x);
    let base = tip - dir * Layout::GRAPH_ARROW_LENGTH;
    let left = base + perp * Layout::GRAPH_ARROW_HALF_WIDTH;
    let right = base - perp * Layout::GRAPH_ARROW_HALF_WIDTH;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        color,
        Stroke::NONE,
    ));
}

struct NodeCanvas<'a> {
    painter: &'a egui::Painter,
    rect: Rect,
    palette: &'a Palette,
}

/// Nodes are circles on the pane: inactive ones outlined in 1px
/// `text.muted` with `text.secondary` labels, active ones in 1.5px
/// `accent.primary` with `text.primary` labels. Third-party domains have a
/// dashed outline (D19).
fn paint_nodes(
    canvas: &NodeCanvas<'_>,
    nodes: &[GraphNode],
    positions: &[(Pos2, f32)],
    active: &[bool],
) {
    let painter = canvas.painter;
    let palette = canvas.palette;
    let tokens = ThemeTokens::for_theme(painter.ctx().theme());
    let fill = tokens.surfaces.pane;
    let (label_font, count_font) = fonts::graph_fonts();
    let mut placed_labels: Vec<Rect> = Vec::with_capacity(nodes.len());
    // Active nodes are drawn last so that their labels win overlaps.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&index| active.get(index).copied().unwrap_or_default());
    for index in order.into_iter().rev() {
        let (Some(node), Some(&(center, radius))) = (nodes.get(index), positions.get(index)) else {
            continue;
        };
        let is_active = active.get(index).copied().unwrap_or_default();
        painter.circle_filled(center, radius, fill);
        let stroke = if is_active {
            active_stroke(&tokens)
        } else {
            inactive_node_stroke(&tokens)
        };
        if node.third_party {
            paint_dashed_circle(painter, center, radius, stroke);
        } else {
            painter.circle_stroke(center, radius, stroke);
        }
        let text_color = if is_active {
            palette.text
        } else {
            palette.secondary
        };
        let _ = painter.text(
            center,
            Align2::CENTER_CENTER,
            node.request_count.to_string(),
            count_font.clone(),
            text_color,
        );

        let galley = painter.layout_no_wrap(node.host.clone(), label_font.clone(), text_color);
        let label_size = galley.size();
        let half_w = label_size.x * 0.5;
        let label_x = center.x.clamp(
            canvas.rect.left() + half_w + Space::XS,
            canvas.rect.right() - half_w - Space::XS,
        );
        let preferred_y = center.y + radius + Space::XS;
        let label_y = if preferred_y + label_size.y <= canvas.rect.bottom() - Space::XS {
            preferred_y
        } else {
            (center.y - radius - label_size.y - Space::XS).max(canvas.rect.top() + Space::XS)
        };
        let label_rect = Rect::from_min_size(Pos2::new(label_x - half_w, label_y), label_size);

        // Skip labels that would overlap one already placed, unless active.
        let overlaps = placed_labels
            .iter()
            .any(|existing| existing.expand(Space::XXS).intersects(label_rect));
        if !overlaps || is_active {
            // The pane color under the label keeps edges from crossing it.
            painter.rect_filled(
                label_rect.expand2(Layout::GRAPH_LABEL_PADDING),
                Radius::PANE,
                fill,
            );
            painter.galley(label_rect.min, galley, text_color);
            placed_labels.push(label_rect);
        }
    }
}

pub(super) fn paint_dashed_circle(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    stroke: Stroke,
) {
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
    let half_w = rect
        .width()
        .mul_add(0.5, -Layout::GRAPH_CANVAS_PADDING)
        .max(Layout::GRAPH_AXIS_MIN);
    let half_h = rect
        .height()
        .mul_add(0.5, -Layout::GRAPH_CANVAS_PADDING)
        .max(Layout::GRAPH_AXIS_MIN);
    let center = rect.center();
    Pos2::new(nx.mul_add(half_w, center.x), ny.mul_add(half_h, center.y))
}

fn from_screen(rect: Rect, pos: Pos2) -> (f32, f32) {
    let half_w = rect
        .width()
        .mul_add(0.5, -Layout::GRAPH_CANVAS_PADDING)
        .max(Layout::GRAPH_AXIS_MIN);
    let half_h = rect
        .height()
        .mul_add(0.5, -Layout::GRAPH_CANVAS_PADDING)
        .max(Layout::GRAPH_AXIS_MIN);
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
    t.mul_add(
        Layout::GRAPH_MAX_NODE_RADIUS - Layout::GRAPH_MIN_NODE_RADIUS,
        Layout::GRAPH_MIN_NODE_RADIUS,
    )
}

fn hit_test(positions: &[(Pos2, f32)], pointer: Pos2) -> Option<usize> {
    positions
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (center, radius))| center.distance(pointer) <= radius + Space::XS)
        .map(|(index, _)| index)
}

fn format_node_tooltip(graph: &RelationshipGraph, node: &GraphNode, lang: Language) -> String {
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
    i18n::map::graph_node_tooltip(
        lang,
        i18n::map::GraphNodeSummary {
            host: &node.host,
            site: &node.site,
            third_party: node.third_party,
            request_count: node.request_count,
            error_count: node.error_count,
            total_bytes: bytes,
            outgoing,
            incoming,
        },
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

        let (all_text, capped) =
            summary_text(&layout, EdgeVisibility::default(), Language::English);
        assert!(!capped);
        assert_eq!(all_text, "2 domains · 2 edges");

        let only_referer = EdgeVisibility {
            referer: true,
            origin: false,
            redirect: false,
        };
        let (filtered_text, _) = summary_text(&layout, only_referer, Language::English);
        assert_eq!(filtered_text, "2 domains · 1/2 edges");

        let (ko_text, _) = summary_text(&layout, only_referer, Language::Korean);
        assert_eq!(ko_text, "도메인 2 · 연결선 1/2");
    }

    #[test]
    fn inactive_edges_and_legend_keep_non_text_contrast_and_hierarchy() {
        use crate::ui::style::tokens::contrast_ratio;

        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let tokens = ThemeTokens::for_theme(theme);
            let edge = inactive_edge_stroke(&tokens);
            let node = inactive_node_stroke(&tokens);
            let active = active_stroke(&tokens);

            assert!((edge.width - StrokeWidth::GRAPH_EDGE).abs() < f32::EPSILON);
            assert!((node.width - StrokeWidth::STRUCTURAL).abs() < f32::EPSILON);
            assert!(edge.width < active.width, "{theme:?}: active stays thicker");
            assert_ne!(edge.color, active.color, "{theme:?}: active color differs");

            // Inactive edges and legend samples meet WCAG 1.4.11 (3:1) on both
            // the graph canvas (`pane`) and the `Edges` menu (`elevated`).
            for surface in [tokens.surfaces.pane, tokens.surfaces.elevated] {
                let ratio = contrast_ratio(edge.color, surface);
                assert!(
                    ratio >= 3.0,
                    "{theme:?}: inactive edge contrast {ratio:.2} on {surface:?}"
                );
            }
        }
    }

    #[test]
    fn edge_kinds_have_distinct_dash_patterns() {
        let referer = edge_dash(EdgeKind::Referer);
        let origin = edge_dash(EdgeKind::Origin);
        let redirect = edge_dash(EdgeKind::Redirect);
        assert_eq!(referer, None);
        assert!(origin.is_some());
        assert!(redirect.is_some());
        assert_ne!(origin, redirect);
    }
}
