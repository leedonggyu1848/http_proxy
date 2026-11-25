//! The top bar: proxy status on the left, the main view switch in the
//! middle, and HTTPS status with the main actions on the right. Narrow
//! windows move whole zones onto further rows instead of clipping them.

use eframe::egui;

use crate::ui::InspectorApp;
use crate::ui::app::MainView;
use crate::ui::style::controls::{Segment, segmented};
use crate::ui::style::tokens::Space;

/// A zone of the top bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Zone {
    Left,
    Center,
    Right,
}

const ZONES: [Zone; 3] = [Zone::Left, Zone::Center, Zone::Right];

/// Sizes of the zones measured in the previous frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TopBarSizes {
    left: egui::Vec2,
    center: egui::Vec2,
    right: egui::Vec2,
}

impl TopBarSizes {
    const fn of(&self, zone: Zone) -> egui::Vec2 {
        match zone {
            Zone::Left => self.left,
            Zone::Center => self.center,
            Zone::Right => self.right,
        }
    }

    const fn set(&mut self, zone: Zone, size: egui::Vec2) {
        match zone {
            Zone::Left => self.left = size,
            Zone::Center => self.center = size,
            Zone::Right => self.right = size,
        }
    }
}

/// Where each zone starts, relative to the bar's top left corner, and the
/// bar's total height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    left: egui::Vec2,
    center: egui::Vec2,
    right: egui::Vec2,
    height: f32,
}

impl Placement {
    const fn of(&self, zone: Zone) -> egui::Vec2 {
        match zone {
            Zone::Left => self.left,
            Zone::Center => self.center,
            Zone::Right => self.right,
        }
    }
}

/// Places the zones in `width`: one row with the switch centered when all
/// fit; otherwise the proxy and HTTPS zones share the first row and the
/// switch gets its own; otherwise one zone per row.
fn place(width: f32, sizes: &TopBarSizes, gap: egui::Vec2) -> Placement {
    let TopBarSizes {
        left,
        center,
        right,
    } = *sizes;
    let right_x = (width - right.x).max(0.0);
    if 2.0f32.mul_add(gap.x, left.x + center.x + right.x) <= width {
        let centered = (width - center.x) / 2.0;
        let earliest = left.x + gap.x;
        let latest = right_x - gap.x - center.x;
        let x = centered.clamp(earliest, latest.max(earliest));
        let height = left.y.max(center.y).max(right.y);
        return Placement {
            left: egui::vec2(0.0, (height - left.y) / 2.0),
            center: egui::vec2(x, (height - center.y) / 2.0),
            right: egui::vec2(right_x, (height - right.y) / 2.0),
            height,
        };
    }
    if left.x + right.x + gap.x <= width {
        let first = left.y.max(right.y);
        let second = first + gap.y;
        return Placement {
            left: egui::vec2(0.0, (first - left.y) / 2.0),
            right: egui::vec2(right_x, (first - right.y) / 2.0),
            center: egui::vec2(0.0, second),
            height: second + center.y,
        };
    }
    if left.x + center.x + gap.x <= width {
        let first = left.y.max(center.y);
        let second = first + gap.y;
        return Placement {
            left: egui::vec2(0.0, (first - left.y) / 2.0),
            center: egui::vec2(left.x + gap.x, (first - center.y) / 2.0),
            right: egui::vec2(0.0, second),
            height: second + right.y,
        };
    }
    let center_y = left.y + gap.y;
    let right_y = center_y + center.y + gap.y;
    Placement {
        left: egui::Vec2::ZERO,
        center: egui::vec2(0.0, center_y),
        right: egui::vec2(0.0, right_y),
        height: right_y + right.y,
    }
}

impl InspectorApp {
    /// The top bar, laid out from the zone sizes of the previous frame. A
    /// frame whose sizes changed is discarded and drawn again, so zones
    /// never show at stale positions.
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        // Keep the pills and buttons off the window edges.
        let inset = egui::vec2(Space::XS, Space::XS);
        let origin = ui.cursor().min + inset;
        let width = 2.0f32.mul_add(-inset.x, ui.available_width()).max(0.0);
        let gap = egui::vec2(Space::LG, Space::XS);
        let placement = place(width, &self.layout.top_bar, gap);
        let mut measured = TopBarSizes::default();
        for zone in ZONES {
            let min = origin + placement.of(zone);
            let max_width = (origin.x + width - min.x).max(0.0);
            let height = self
                .layout
                .top_bar
                .of(zone)
                .y
                .max(ui.spacing().interact_size.y);
            let rect = egui::Rect::from_min_size(min, egui::vec2(max_width, height));
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("top-bar", zone_name(zone)))
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            match zone {
                Zone::Left => self.proxy_status(&mut child),
                Zone::Center => self.view_switch(&mut child),
                Zone::Right => self.top_bar_actions(&mut child),
            }
            measured.set(zone, child.min_rect().size());
        }
        let changed = ZONES
            .into_iter()
            .any(|zone| (measured.of(zone) - self.layout.top_bar.of(zone)).length() > 0.5);
        self.layout.top_bar = measured;
        if changed {
            ui.ctx().request_discard("top bar zones changed size");
        }
        let bar = egui::Rect::from_min_size(origin, egui::vec2(width, placement.height))
            .expand2(egui::vec2(inset.x, 0.0));
        let bar = bar.with_min_y(bar.min.y - inset.y);
        ui.advance_cursor_after_rect(bar);
    }

    /// The `Requests | Map` switch.
    fn view_switch(&mut self, ui: &mut egui::Ui) {
        let map = self.map.analysis.url_map();
        let hosts = map.origins().len();
        let nodes = map.node_count();
        let _changed = segmented(
            ui,
            &mut self.layout.main_view,
            &[
                Segment {
                    value: MainView::Requests,
                    label: "Requests",
                    detail: Some(self.requests.records.len().to_string()),
                    hover: Some("Captured requests and their details".to_owned()),
                },
                Segment {
                    value: MainView::Map,
                    label: "Map",
                    detail: Some(format!("{hosts} hosts")),
                    hover: Some(format!(
                        "Site map and relationship graph: {hosts} hosts · {nodes} nodes"
                    )),
                },
            ],
        );
    }
}

const fn zone_name(zone: Zone) -> &'static str {
    match zone {
        Zone::Left => "left",
        Zone::Center => "center",
        Zone::Right => "right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: egui::Vec2 = egui::vec2(16.0, 4.0);

    fn sizes(left: f32, center: f32, right: f32) -> TopBarSizes {
        TopBarSizes {
            left: egui::vec2(left, 20.0),
            center: egui::vec2(center, 24.0),
            right: egui::vec2(right, 20.0),
        }
    }

    #[test]
    fn wide_bar_centers_the_switch_on_one_row() {
        let placement = place(1_000.0, &sizes(200.0, 160.0, 400.0), GAP);
        assert!((placement.height - 24.0).abs() < f32::EPSILON);
        assert!((placement.center.x - 420.0).abs() < f32::EPSILON);
        assert!((placement.right.x - 600.0).abs() < f32::EPSILON);
        // Shorter zones are centered vertically on the row.
        assert!((placement.left.y - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn switch_moves_aside_rather_than_overlap() {
        // Centered, the switch would start at 300 and overlap the left zone.
        let placement = place(760.0, &sizes(320.0, 160.0, 240.0), GAP);
        assert!((placement.height - 24.0).abs() < f32::EPSILON);
        assert!(placement.center.x >= 320.0 + GAP.x);
        assert!(placement.center.x + 160.0 <= placement.right.x - GAP.x);
    }

    #[test]
    fn narrow_bar_wraps_whole_zones() {
        let two_rows = place(624.0, &sizes(180.0, 160.0, 400.0), GAP);
        assert_eq!(two_rows.center, egui::vec2(0.0, 24.0));
        assert!((two_rows.right.x - 224.0).abs() < f32::EPSILON);

        let switch_beside_proxy = place(624.0, &sizes(260.0, 160.0, 400.0), GAP);
        assert!((switch_beside_proxy.center.x - 276.0).abs() < f32::EPSILON);
        assert_eq!(switch_beside_proxy.right, egui::vec2(0.0, 28.0));

        let stacked = place(300.0, &sizes(260.0, 160.0, 400.0), GAP);
        assert!(stacked.center.y > stacked.left.y && stacked.right.y > stacked.center.y);
        assert!((stacked.height - 2.0f32.mul_add(GAP.y, 20.0 + 24.0 + 20.0)).abs() < f32::EPSILON);
    }
}
