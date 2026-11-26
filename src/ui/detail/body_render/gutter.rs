//! The line number gutter of the raw viewer: numbers in `text.muted`, right
//! aligned, with a 1px `borders.subtle` edge between them and the text (D8,
//! D24).

use eframe::egui;

use crate::ui::style::fonts;
use crate::ui::style::tokens::{Space, StrokeWidth, ThemeTokens};

/// Width of a gutter for `lines` numbered lines: the widest number, at
/// least two digits, with [`Space::MD`] before it and [`Space::SM`] after.
pub(super) fn width(ui: &egui::Ui, lines: usize) -> f32 {
    let widest = "9".repeat(lines.max(10).to_string().len());
    let digits = ui
        .painter()
        .layout_no_wrap(widest, fonts::data(), egui::Color32::PLACEHOLDER)
        .size()
        .x;
    digits + Space::MD + Space::SM
}

/// Gap between the gutter edge and the text.
const TEXT_GAP: f32 = Space::MD;

/// Paints `number` right-aligned in a gutter of `gutter_width` on a row of
/// `row_height`, and reserves the gutter and the gap after it.
pub(super) fn number(ui: &mut egui::Ui, number: usize, gutter_width: f32, row_height: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(gutter_width + TEXT_GAP, row_height),
        egui::Sense::hover(),
    );
    paint_number(ui, number, rect.left() + gutter_width, rect.center().y);
}

fn paint_number(ui: &egui::Ui, number: usize, gutter_right: f32, center_y: f32) {
    let muted = ThemeTokens::for_theme(ui.ctx().theme()).text.muted;
    ui.painter().text(
        egui::pos2(gutter_right - Space::SM, center_y),
        egui::Align2::RIGHT_CENTER,
        number.to_string(),
        fonts::data(),
        muted,
    );
}

/// The 1px edge on the right of the gutter.
pub(super) fn paint_edge(ui: &egui::Ui, x: f32, y_range: egui::Rangef) {
    let subtle = ThemeTokens::for_theme(ui.ctx().theme()).borders.subtle;
    ui.painter().vline(
        x.round() - StrokeWidth::STRUCTURAL / 2.0,
        y_range,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, subtle),
    );
}

/// Shows the text edit `add` makes to the right of a gutter that numbers
/// each source line of its wrapped text. A wrapped line keeps one number,
/// on its first row.
pub(super) fn beside_text(
    ui: &mut egui::Ui,
    lines: usize,
    add: impl FnOnce(&mut egui::Ui) -> egui::text_edit::TextEditOutput,
) -> egui::text_edit::TextEditOutput {
    let gutter_width = width(ui, lines);
    let top_left = ui.cursor().min;
    let full_width = ui.available_width();
    let text_rect = egui::Rect::from_min_max(
        top_left + egui::vec2(gutter_width + TEXT_GAP, 0.0),
        egui::pos2(ui.max_rect().right(), f32::INFINITY),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let output = add(&mut child);
    let height = output.response.rect.height();
    let rect = egui::Rect::from_min_size(top_left, egui::vec2(full_width, height));
    ui.allocate_rect(rect, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let gutter_right = top_left.x + gutter_width;
        let mut number = 1;
        let mut starts_line = true;
        for row in &output.galley.rows {
            if starts_line {
                let center_y = output.galley_pos.y + row.rect().center().y;
                paint_number(ui, number, gutter_right, center_y);
                number += 1;
            }
            starts_line = row.ends_with_newline;
        }
        paint_edge(ui, gutter_right, rect.y_range());
    }
    output
}
