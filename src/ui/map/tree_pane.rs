//! Virtualized host -> path tree pane with keyboard navigation, accessible
//! status indicators, method badges, and brief new-node highlights.

use std::time::Duration;

use eframe::egui::{self, Color32, Key, RichText, Stroke};
use http_inspector_analysis::url_map::{NodeStats, RowKind, UrlMap, ViewNodeKey, VisibleRow};

use super::MapPanel;
use crate::ui::detail::tree_view;
use crate::ui::style::theme::{self, Palette};
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth};

const NEW_NODE_HIGHLIGHT: Duration = Duration::from_secs(2);

pub(super) fn show(panel: &mut MapPanel, ui: &mut egui::Ui, map: &UrlMap, shortcuts_allowed: bool) {
    if panel.cached_rows.is_empty() {
        let message = if map.node_count() == 0 {
            "No URLs captured yet.\nSend HTTP/HTTPS traffic through the proxy to build the URL map."
        } else {
            "No nodes match the current map filters."
        };
        ui.centered_and_justified(|ui| {
            ui.add(egui::Label::new(RichText::new(message).weak()).wrap());
        });
        return;
    }

    let available_height = ui.available_height().max(Layout::MAP_TREE_ROW_HEIGHT);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive viewport height divided by fixed row height"
    )]
    {
        panel.page_rows =
            ((available_height / Layout::MAP_TREE_ROW_HEIGHT).floor() as usize).max(1);
    }

    if shortcuts_allowed {
        handle_keyboard(panel, ui);
    }

    let mut scroll = egui::ScrollArea::both()
        .id_salt("url-map-tree-rows")
        .auto_shrink([false, false]);
    if let Some(target_row) = panel.keyboard_scroll_to.take() {
        #[allow(clippy::cast_precision_loss, reason = "row index for scroll offset")]
        let offset = available_height
            .mul_add(-0.35, target_row as f32 * Layout::MAP_TREE_ROW_HEIGHT)
            .max(0.0);
        scroll = scroll.vertical_scroll_offset(offset);
    }

    let mut toggle_key: Option<(ViewNodeKey, bool)> = None;
    let mut select_key: Option<ViewNodeKey> = None;
    let mut any_fading = false;
    let row_count = panel.cached_rows.len();

    scroll.show_rows(ui, Layout::MAP_TREE_ROW_HEIGHT, row_count, |ui, range| {
        for index in range {
            let Some(row) = panel.cached_rows.get(index) else {
                continue;
            };
            let is_selected = panel.selected.as_ref() == Some(&row.key);
            let outcome = render_row(ui, row, is_selected);
            if outcome.toggled {
                toggle_key = Some((row.key.clone(), !row.expansion.is_expanded()));
            }
            if outcome.clicked {
                select_key = Some(row.key.clone());
            }
            any_fading |= outcome.fading;
        }
    });

    if let Some((key, expanded)) = toggle_key {
        panel.set_expanded(key, expanded);
        ui.ctx().request_repaint();
    }
    if let Some(key) = select_key {
        panel.selected = Some(key);
    }
    if any_fading {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
}

struct RowOutcome {
    clicked: bool,
    toggled: bool,
    fading: bool,
}

fn render_row(ui: &mut egui::Ui, row: &VisibleRow, is_selected: bool) -> RowOutcome {
    let palette = Palette::of(ui);
    let age = row.created_at.elapsed();
    let fading = age < NEW_NODE_HIGHLIGHT;

    let mut frame = egui::Frame::NONE.inner_margin(Layout::MAP_TREE_ROW_MARGIN);
    if is_selected {
        frame = frame.fill(palette.surfaces.selection);
    } else if fading {
        let opacity = 1.0 - age.as_secs_f32() / NEW_NODE_HIGHLIGHT.as_secs_f32();
        frame = frame.fill(theme::new_map_node_fill(ui, opacity));
    }

    let mut toggled = false;
    let mut clicked = false;
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(Layout::MAP_TREE_ROW_HEIGHT - Space::XS);
            toggled = tree_view::tree_row_prefix(
                ui,
                row.depth,
                row.expansion.is_expandable(),
                row.expansion.is_expanded(),
            );

            let label_text = format_row_label(row, &palette);
            let response = ui.add(
                egui::Label::new(label_text)
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .sense(egui::Sense::click()),
            );
            if response.clicked() {
                clicked = true;
            }
            if response.double_clicked() && row.expansion.is_expandable() {
                toggled = true;
            }

            render_row_badges(ui, row, &palette);
        });
    });

    RowOutcome {
        clicked,
        toggled,
        fading,
    }
}

fn format_row_label(row: &VisibleRow, palette: &Palette) -> RichText {
    match row.kind {
        RowKind::Origin => RichText::new(&row.label).strong(),
        RowKind::FoldedId { count } => RichText::new(format!("{} ({count} IDs)", row.label))
            .monospace()
            .color(palette.info),
        RowKind::Segment => RichText::new(&row.label).monospace(),
    }
}

fn render_row_badges(ui: &mut egui::Ui, row: &VisibleRow, palette: &Palette) {
    let stats = &row.subtree;
    if stats.is_tunnel_only() {
        ui.label(
            RichText::new(format!(
                "[Tunnel] No path info · {} req",
                stats.request_count
            ))
            .color(palette.info)
            .small(),
        );
        return;
    }

    if stats.has_tunnel {
        ui.label(RichText::new("[Tunnel]").color(palette.info).small());
    }

    ui.label(
        RichText::new(format!("{} req", stats.request_count))
            .color(palette.muted)
            .small(),
    );

    paint_status_bar(ui, stats, palette);

    let numeric_status = compact_status_numbers(stats);
    if !numeric_status.is_empty() {
        let color = if stats.status.errors() > 0 {
            palette.error
        } else if stats.status.s3xx > 0 {
            palette.warning
        } else {
            palette.muted
        };
        ui.label(RichText::new(numeric_status).color(color).small());
    }

    for method in stats.methods.names().into_iter().take(4) {
        let color = method_color(method, palette);
        ui.label(RichText::new(method).monospace().color(color).small());
    }
    let total_methods = stats.methods.names().len();
    if total_methods > 4 {
        ui.label(
            RichText::new(format!("+{}", total_methods - 4))
                .color(palette.muted)
                .small(),
        );
    }

    if stats.is_third_party() {
        ui.label(
            RichText::new("[3rd]")
                .color(palette.warning)
                .small()
                .strong(),
        );
    }
}

fn paint_status_bar(ui: &mut egui::Ui, stats: &NodeStats, palette: &Palette) {
    let total = stats.status.total();
    if total == 0 {
        return;
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(Layout::MAP_STATUS_WIDTH, Layout::MAP_STATUS_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, Radius::STATUS_BAR, palette.surfaces.inset);

    let segments = [
        (stats.status.s2xx, palette.success),
        (stats.status.s3xx, palette.warning),
        (
            stats
                .status
                .s4xx
                .saturating_add(stats.status.s5xx)
                .saturating_add(stats.status.failed),
            palette.error,
        ),
        (stats.status.tunnel, palette.info),
        (stats.status.pending, palette.muted),
    ];

    let mut x = rect.left();
    #[allow(clippy::cast_precision_loss, reason = "proportional status bar width")]
    let total_f = total as f32;
    for (count, color) in segments {
        if count == 0 {
            continue;
        }
        #[allow(clippy::cast_precision_loss, reason = "proportional status bar width")]
        let share = (count as f32 / total_f) * rect.width();
        let seg_width = share
            .max(Layout::MAP_STATUS_MIN_SEGMENT)
            .min(rect.right() - x);
        if seg_width <= 0.0 {
            break;
        }
        let seg_rect = egui::Rect::from_min_max(
            egui::pos2(x, rect.top()),
            egui::pos2((x + seg_width).min(rect.right()), rect.bottom()),
        );
        painter.rect_filled(seg_rect, Radius::PANE, color);
        x += seg_width;
    }
    painter.rect_stroke(
        rect,
        Radius::STATUS_BAR,
        Stroke::new(StrokeWidth::MAP_STATUS, palette.surfaces.border),
        egui::StrokeKind::Inside,
    );

    let _ = response.on_hover_text(format!(
        "2xx: {} · 3xx: {} · 4xx: {} · 5xx: {} · Failed: {} · Tunnel: {} · Pending: {}",
        stats.status.s2xx,
        stats.status.s3xx,
        stats.status.s4xx,
        stats.status.s5xx,
        stats.status.failed,
        stats.status.tunnel,
        stats.status.pending,
    ));
}

fn compact_status_numbers(stats: &NodeStats) -> String {
    let mut parts = Vec::new();
    if stats.status.s2xx > 0 {
        parts.push(format!("2xx:{}", stats.status.s2xx));
    }
    if stats.status.s3xx > 0 {
        parts.push(format!("3xx:{}", stats.status.s3xx));
    }
    let errs = stats.status.errors();
    if errs > 0 {
        parts.push(format!("err:{errs}"));
    }
    if stats.status.tunnel > 0 {
        parts.push(format!("tun:{}", stats.status.tunnel));
    }
    if stats.status.pending > 0 {
        parts.push(format!("pend:{}", stats.status.pending));
    }
    parts.join(" ")
}

fn method_color(method: &str, palette: &Palette) -> Color32 {
    match method {
        "GET" | "HEAD" => palette.info,
        "POST" | "PUT" | "PATCH" => palette.success,
        "DELETE" => palette.error,
        _ => palette.warning,
    }
}

#[derive(Clone, Copy)]
enum TreeNavKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

fn take_nav_keys(ui: &egui::Ui) -> Vec<TreeNavKey> {
    let mut keys = Vec::new();
    ui.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
                ..
            } = *event
            {
                let mapped = match key {
                    Key::ArrowUp => Some(TreeNavKey::Up),
                    Key::ArrowDown => Some(TreeNavKey::Down),
                    Key::ArrowLeft => Some(TreeNavKey::Left),
                    Key::ArrowRight => Some(TreeNavKey::Right),
                    Key::Home => Some(TreeNavKey::Home),
                    Key::End => Some(TreeNavKey::End),
                    Key::PageUp => Some(TreeNavKey::PageUp),
                    Key::PageDown => Some(TreeNavKey::PageDown),
                    _ => None,
                };
                if let Some(nav) = mapped {
                    keys.push(nav);
                    return false;
                }
            }
            true
        });
    });
    keys
}

fn handle_keyboard(panel: &mut MapPanel, ui: &egui::Ui) {
    let keys = take_nav_keys(ui);
    if keys.is_empty() {
        return;
    }
    let len = panel.cached_rows.len();
    let Some(last) = len.checked_sub(1) else {
        return;
    };
    let mut current = panel
        .selected
        .as_ref()
        .and_then(|sel| panel.cached_rows.iter().position(|row| &row.key == sel));
    let mut expanded_change: Option<(ViewNodeKey, bool)> = None;

    for key in keys {
        match key {
            TreeNavKey::Up => {
                current = Some(current.map_or(last, |i| i.saturating_sub(1)));
            }
            TreeNavKey::Down => {
                current = Some(current.map_or(0, |i| (i + 1).min(last)));
            }
            TreeNavKey::Home => current = Some(0),
            TreeNavKey::End => current = Some(last),
            TreeNavKey::PageUp => {
                current = Some(current.map_or(last, |i| i.saturating_sub(panel.page_rows)));
            }
            TreeNavKey::PageDown => {
                current = Some(current.map_or(0, |i| i.saturating_add(panel.page_rows).min(last)));
            }
            TreeNavKey::Right => {
                if let Some(idx) = current
                    && let Some(row) = panel.cached_rows.get(idx)
                {
                    if row.expansion.is_expandable() && !row.expansion.is_expanded() {
                        expanded_change = Some((row.key.clone(), true));
                    } else if row.expansion.is_expanded() && idx < last {
                        current = Some(idx + 1);
                    }
                }
            }
            TreeNavKey::Left => {
                if let Some(idx) = current
                    && let Some(row) = panel.cached_rows.get(idx)
                {
                    if row.expansion.is_expanded() {
                        expanded_change = Some((row.key.clone(), false));
                    } else if let Some(parent_key) = &row.parent
                        && let Some(parent_idx) =
                            panel.cached_rows.iter().position(|r| &r.key == parent_key)
                    {
                        current = Some(parent_idx);
                    }
                }
            }
        }
    }

    if let Some((key, expanded)) = expanded_change {
        panel.set_expanded(key, expanded);
    }
    if let Some(idx) = current
        && let Some(row) = panel.cached_rows.get(idx)
    {
        panel.selected = Some(row.key.clone());
        panel.keyboard_scroll_to = Some(idx);
    }
    ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    ui.ctx().request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use http_inspector_analysis::url_map::StatusBucket;
    use std::time::SystemTime;

    #[test]
    fn compact_status_numbers_includes_tunnel_and_pending_for_accessibility() {
        let now = SystemTime::UNIX_EPOCH;
        let mut stats = NodeStats::default();
        stats.record_request(&Method::GET, StatusBucket::S2xx, now, false, false);
        stats.record_request(&Method::CONNECT, StatusBucket::Tunnel, now, false, true);
        stats.record_request(&Method::GET, StatusBucket::Pending, now, false, false);
        stats.record_request(&Method::CONNECT, StatusBucket::Failed, now, false, true);
        assert_eq!(compact_status_numbers(&stats), "2xx:1 err:1 tun:1 pend:1");
    }
}
