//! The Timing tab: how long each phase of an exchange took, as a two-column
//! table with the times right-aligned in monospace (D15, D23).

use std::time::Instant;

use eframe::egui;
use http_inspector_exchange::Exchange;

use crate::ui::i18n::Language;
use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};
use crate::ui::timing::{ExchangeTiming, TimingEntry};

pub(super) fn show(ui: &mut egui::Ui, record: &Exchange, lang: Language) {
    let entries =
        ExchangeTiming::from_exchange(record, Instant::now()).entries(record.outcome(), lang);
    let label_width = entries
        .iter()
        .map(|entry| label_galley(ui, entry.label).size().x)
        .fold(0.0, f32::max);
    for entry in &entries {
        row(ui, entry, label_width, lang);
    }
}

fn label_galley(ui: &egui::Ui, label: &str) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        label.to_owned(),
        fonts::control(),
        Palette::of(ui).secondary,
    )
}

/// One table row: the phase in `text.secondary` sans, then its time right-
/// aligned in a column as wide as the longest time, over a `borders.subtle`
/// row line.
fn row(ui: &mut egui::Ui, phase: &TimingEntry, label_width: f32, lang: Language) {
    let palette = Palette::of(ui);
    let value = phase.value_text(lang);
    let value_color = if phase.millis.is_some() {
        palette.text
    } else {
        palette.secondary
    };
    let value_galley = ui
        .painter()
        .layout_no_wrap(value, fonts::data(), value_color);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), Layout::TREE_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let label = label_galley(ui, phase.label);
    let left = rect.left() + Space::MD;
    let painter = ui.painter();
    painter.galley(
        egui::pos2(left, rect.center().y - label.size().y / 2.0),
        label,
        palette.secondary,
    );
    let value_right =
        (left + label_width + Space::LG + Layout::TIMING_VALUE_WIDTH).min(rect.right() - Space::SM);
    painter.galley(
        egui::pos2(
            value_right - value_galley.size().x,
            rect.center().y - value_galley.size().y / 2.0,
        ),
        value_galley,
        value_color,
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - StrokeWidth::STRUCTURAL / 2.0,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, palette.borders.subtle),
    );
}
