//! Docked collapsible legend panel for the relationship graph.
//!
//! Swatches reuse the drawing helpers from [`super::graph_pane`] and color
//! tokens from [`ThemeTokens`] so the legend never drifts from the rendered
//! graph (D6, D7, D19). Descriptions are standard [`egui::Label`] widgets so
//! assistive technology reads them directly.

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use http_inspector_analysis::url_map::EdgeKind;

use super::graph_pane;
use crate::ui::i18n::Language;
use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth, ThemeTokens};

/// Renders the docked relationship graph legend inside `ui`.
pub(super) fn show(ui: &mut egui::Ui, lang: Language) {
    let map = lang.map();
    let shared = lang.shared();
    egui::ScrollArea::vertical()
        .id_salt("map-graph-legend-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::from(Space::SM))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = Space::SM;

                    swatch_row(ui, map.node, map.legend_node_desc, paint_node_swatch);
                    swatch_row(
                        ui,
                        map.legend_node_size,
                        map.legend_node_size_desc,
                        paint_node_size_swatch,
                    );
                    swatch_row(
                        ui,
                        shared.first_party,
                        map.legend_first_party_desc,
                        |ui, rect| paint_party_swatch(ui, rect, false),
                    );
                    swatch_row(
                        ui,
                        shared.third_party,
                        map.legend_third_party_desc,
                        |ui, rect| paint_party_swatch(ui, rect, true),
                    );

                    section_divider(ui);

                    swatch_row(ui, map.referer, map.legend_referer_desc, |ui, rect| {
                        paint_edge_swatch(ui, rect, EdgeKind::Referer);
                    });
                    swatch_row(ui, map.origin_cors, map.legend_origin_desc, |ui, rect| {
                        paint_edge_swatch(ui, rect, EdgeKind::Origin);
                    });
                    swatch_row(ui, map.redirect, map.legend_redirect_desc, |ui, rect| {
                        paint_edge_swatch(ui, rect, EdgeKind::Redirect);
                    });
                    swatch_row(
                        ui,
                        map.legend_active,
                        map.legend_active_desc,
                        paint_active_swatch,
                    );
                    swatch_row(
                        ui,
                        map.legend_self_loop,
                        map.legend_self_loop_desc,
                        paint_self_loop_swatch,
                    );

                    section_divider(ui);

                    control_row(ui, map.legend_drag, map.legend_drag_desc);
                    control_row(ui, map.legend_click, map.legend_click_desc);
                });
        });
}

fn swatch_row(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    paint: impl FnOnce(&egui::Ui, Rect),
) {
    let palette = Palette::of(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = Space::SM;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                Layout::GRAPH_LEGEND_SWATCH_WIDTH,
                Layout::GRAPH_LEGEND_SWATCH_HEIGHT,
            ),
            Sense::hover(),
        );
        if ui.is_rect_visible(rect) {
            paint(ui, rect);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = Space::XXS;
            ui.add(
                egui::Label::new(
                    RichText::new(label)
                        .font(fonts::control())
                        .color(palette.text),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(description)
                        .font(fonts::column_header())
                        .color(palette.secondary),
                )
                .wrap(),
            );
        });
    });
}

fn control_row(ui: &mut egui::Ui, action: &str, description: &str) {
    let palette = Palette::of(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = Space::SM;
        let key_width = Layout::GRAPH_LEGEND_SWATCH_WIDTH;
        ui.allocate_ui_with_layout(
            egui::vec2(key_width, 0.0),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                ui.set_min_width(key_width);
                ui.set_max_width(key_width);
                ui.add(
                    egui::Label::new(
                        RichText::new(action)
                            .font(fonts::control())
                            .color(palette.text),
                    )
                    .wrap(),
                );
            },
        );
        ui.add(
            egui::Label::new(
                RichText::new(description)
                    .font(fonts::column_header())
                    .color(palette.secondary),
            )
            .wrap(),
        );
    });
}

fn section_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), StrokeWidth::STRUCTURAL),
        Sense::hover(),
    );
    let stroke = Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle);
    ui.painter().hline(rect.x_range(), rect.center().y, stroke);
}

fn paint_node_swatch(ui: &egui::Ui, rect: Rect) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let stroke = graph_pane::inactive_node_stroke(&tokens);
    let painter = ui.painter();
    let center = rect.center();
    let radius = Layout::GRAPH_LEGEND_NODE_RADIUS;
    painter.circle_filled(center, radius, tokens.surfaces.pane);
    painter.circle_stroke(center, radius, stroke);
    let (_, count_font) = fonts::graph_fonts();
    let _ = painter.text(
        center,
        Align2::CENTER_CENTER,
        "5",
        count_font,
        tokens.text.secondary,
    );
}

fn paint_node_size_swatch(ui: &egui::Ui, rect: Rect) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let stroke = graph_pane::inactive_node_stroke(&tokens);
    let painter = ui.painter();
    let small_r = Layout::GRAPH_LEGEND_SMALL_RADIUS;
    let large_r = Layout::GRAPH_LEGEND_NODE_RADIUS;
    let gap = Space::XS;
    let total = 2.0f32.mul_add(small_r + large_r, gap);
    let left = total.mul_add(-0.5, rect.center().x);
    let y = rect.center().y;
    let small_center = Pos2::new(left + small_r, y);
    let large_center = Pos2::new(2.0f32.mul_add(small_r, left) + gap + large_r, y);
    for (center, radius) in [(small_center, small_r), (large_center, large_r)] {
        painter.circle_filled(center, radius, tokens.surfaces.pane);
        painter.circle_stroke(center, radius, stroke);
    }
}

fn paint_party_swatch(ui: &egui::Ui, rect: Rect, third_party: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let stroke = graph_pane::inactive_node_stroke(&tokens);
    let painter = ui.painter();
    let center = rect.center();
    let radius = Layout::GRAPH_LEGEND_PARTY_RADIUS;
    painter.circle_filled(center, radius, tokens.surfaces.pane);
    if third_party {
        graph_pane::paint_dashed_circle(painter, center, radius, stroke);
    } else {
        painter.circle_stroke(center, radius, stroke);
    }
}

fn paint_edge_swatch(ui: &egui::Ui, rect: Rect, kind: EdgeKind) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let stroke = graph_pane::inactive_edge_stroke(&tokens);
    let painter = ui.painter();
    let y = rect.center().y;
    let start = Pos2::new(rect.left() + Space::XXS, y);
    let tip = Pos2::new(rect.right() - Space::XXS, y);
    let line_end = Pos2::new(
        tip.x - Layout::GRAPH_ARROW_LENGTH + StrokeWidth::STRUCTURAL,
        y,
    );
    match graph_pane::edge_dash(kind) {
        Some((dash, gap)) => {
            graph_pane::paint_dashed_line(painter, start, line_end, stroke, dash, gap);
        }
        None => {
            painter.line_segment([start, line_end], stroke);
        }
    }
    graph_pane::paint_arrowhead(painter, tip, Vec2::X, stroke.color);
}

fn paint_active_swatch(ui: &egui::Ui, rect: Rect) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let stroke = graph_pane::active_stroke(&tokens);
    let painter = ui.painter();
    let y = rect.center().y;
    let radius = Layout::GRAPH_LEGEND_COMPACT_RADIUS;
    let node_center = Pos2::new(rect.left() + Space::XXS + radius, y);
    painter.circle_filled(node_center, radius, tokens.surfaces.pane);
    painter.circle_stroke(node_center, radius, stroke);
    let start = Pos2::new(node_center.x + radius + Space::XXS, y);
    let tip = Pos2::new(rect.right() - Space::XXS, y);
    let line_end = Pos2::new(
        tip.x - Layout::GRAPH_ARROW_LENGTH + StrokeWidth::STRUCTURAL,
        y,
    );
    painter.line_segment([start, line_end], stroke);
    graph_pane::paint_arrowhead(painter, tip, Vec2::X, stroke.color);
}

fn paint_self_loop_swatch(ui: &egui::Ui, rect: Rect) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let loop_stroke = graph_pane::inactive_edge_stroke(&tokens);
    let node_stroke = graph_pane::inactive_node_stroke(&tokens);
    let painter = ui.painter();
    let radius = Layout::GRAPH_LEGEND_COMPACT_RADIUS;
    let loop_center = Pos2::new(rect.center().x, rect.center().y - Space::XS);
    let node_center = Pos2::new(rect.center().x, rect.center().y + Space::XS);
    // Draw the loop before the node so the node sits on top, as in the graph.
    painter.circle_stroke(loop_center, radius, loop_stroke);
    painter.circle_filled(node_center, radius, tokens.surfaces.pane);
    painter.circle_stroke(node_center, radius, node_stroke);
}
