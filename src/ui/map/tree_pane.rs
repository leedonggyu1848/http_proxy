//! Virtualized host -> path tree pane with keyboard navigation, counts in
//! aligned monospace columns, and brief new-node highlights.

use std::time::Duration;

use eframe::egui::{self, Key, RichText};
use http_inspector_analysis::url_map::{NodeStats, RowKind, UrlMap, ViewNodeKey, VisibleRow};

use super::MapPanel;
use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space};
use crate::ui::workbench::text::{self, ShellText};
use crate::ui::workbench::tree;

const NEW_NODE_HIGHLIGHT: Duration = Duration::from_secs(2);

pub(super) fn show(panel: &mut MapPanel, ui: &mut egui::Ui, map: &UrlMap, shortcuts_allowed: bool) {
    if panel.cached_rows.is_empty() {
        let message = if map.node_count() == 0 {
            ShellText::NoUrlsYet
        } else {
            ShellText::NoNodesMatch
        };
        ui.centered_and_justified(|ui| {
            ui.add(egui::Label::new(RichText::new(message.text()).weak()).wrap());
        });
        return;
    }

    let available_height = ui.available_height().max(Layout::TREE_ROW_HEIGHT);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive viewport height divided by fixed row height"
    )]
    {
        panel.page_rows = ((available_height / Layout::TREE_ROW_HEIGHT).floor() as usize).max(1);
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
            .mul_add(-0.35, target_row as f32 * Layout::TREE_ROW_HEIGHT)
            .max(0.0);
        scroll = scroll.vertical_scroll_offset(offset);
    }

    let mut toggle_key: Option<(ViewNodeKey, bool)> = None;
    let mut select_key: Option<ViewNodeKey> = None;
    let mut any_fading = false;
    let row_count = panel.cached_rows.len();

    scroll.show_rows(ui, Layout::TREE_ROW_HEIGHT, row_count, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
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

/// One 24pt tree row: the node name, then its counts right-aligned in fixed
/// columns so they line up from row to row (D15, D18).
fn render_row(ui: &mut egui::Ui, row: &VisibleRow, is_selected: bool) -> RowOutcome {
    let palette = Palette::of(ui);
    let age = row.created_at.elapsed();
    let fading = age < NEW_NODE_HIGHLIGHT;
    if fading && !is_selected {
        let rect = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), Layout::TREE_ROW_HEIGHT),
        );
        let strength = new_highlight_strength(age);
        ui.painter().rect_filled(
            rect,
            0.0,
            palette.surfaces.cross_highlight.gamma_multiply(strength),
        );
    }
    let mark = if is_selected {
        tree::Mark::Selected
    } else {
        tree::Mark::None
    };
    let shown = tree::row(
        ui,
        tree::TreeRow::new(row.depth, &row.label)
            .disclosure(tree::Disclosure::new(
                row.expansion.is_expandable(),
                row.expansion.is_expanded(),
            ))
            .mark(mark)
            .clickable(),
        |ui| {
            ui.add(
                egui::Label::new(format_row_label(row, &palette))
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .selectable(false),
            );
            render_row_metrics(ui, row, &palette);
        },
    );
    let response = shown.response.on_hover_text(status_breakdown(&row.subtree));
    let clicked = response.clicked() && !shown.toggled;
    let toggled = shown.toggled || (response.double_clicked() && row.expansion.is_expandable());
    RowOutcome {
        clicked,
        toggled,
        fading,
    }
}

fn format_row_label(row: &VisibleRow, palette: &Palette) -> RichText {
    match row.kind {
        RowKind::Origin => RichText::new(&row.label)
            .font(fonts::control())
            .color(palette.text),
        RowKind::FoldedId { count } => RichText::new(text::folded_ids(&row.label, count))
            .font(fonts::data())
            .color(palette.secondary),
        RowKind::Segment => RichText::new(&row.label)
            .font(fonts::data())
            .color(palette.text),
    }
}

/// Party and tunnel markers, methods, failures, and the request count,
/// right to left from the row's right edge.
fn render_row_metrics(ui: &mut egui::Ui, row: &VisibleRow, palette: &Palette) {
    let stats = &row.subtree;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(Space::SM);
        let data = |text: String, color| RichText::new(text).font(fonts::data()).color(color);
        column(
            ui,
            data(text::request_total(stats.request_count), palette.text),
        );
        let errors = stats.status.errors();
        let errors_text = if errors > 0 {
            data(text::error_total(errors), palette.error)
        } else {
            RichText::new("")
        };
        column(ui, errors_text);
        let mut methods: Vec<String> = stats
            .methods
            .names()
            .into_iter()
            .take(MAX_METHODS)
            .map(str::to_owned)
            .collect();
        let total_methods = stats.methods.names().len();
        if total_methods > MAX_METHODS {
            methods.push(format!("+{}", total_methods - MAX_METHODS));
        }
        ui.add(
            egui::Label::new(data(methods.join(" "), palette.secondary))
                .wrap_mode(egui::TextWrapMode::Extend)
                .selectable(false),
        );
        let mut markers = Vec::new();
        if stats.has_tunnel {
            markers.push(ShellText::Tunnel.text());
        }
        if stats.is_third_party() {
            markers.push(ShellText::ThirdParty.text());
        }
        if !markers.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new(markers.join(" · "))
                        .font(fonts::control())
                        .color(palette.secondary),
                )
                .selectable(false),
            );
        }
    });
}

/// Methods listed in a row before the rest are counted as `+N`.
const MAX_METHODS: usize = 4;

/// A right-aligned column of [`Layout::MAP_COUNT_WIDTH`].
fn column(ui: &mut egui::Ui, text: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(Layout::MAP_COUNT_WIDTH, Layout::TREE_ROW_HEIGHT),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(text).selectable(false));
        },
    );
}

/// Every status count of a node, for the row tooltip and screen readers.
fn status_breakdown(stats: &NodeStats) -> String {
    let counts = compact_status_numbers(stats);
    if counts.is_empty() {
        text::request_total(stats.request_count)
    } else {
        format!("{} · {counts}", text::request_total(stats.request_count))
    }
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

/// Share of the cross-highlight color left on a node of `age`: full when
/// the node appears, fading to none over [`NEW_NODE_HIGHLIGHT`].
fn new_highlight_strength(age: Duration) -> f32 {
    1.0 - (age.as_secs_f32() / NEW_NODE_HIGHLIGHT.as_secs_f32()).clamp(0.0, 1.0)
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
