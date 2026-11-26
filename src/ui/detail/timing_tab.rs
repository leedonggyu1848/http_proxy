//! The Timing tab: how long each phase of an exchange took, as a two-column
//! table with the times right-aligned in monospace (D15, D23).

use std::time::Duration;

use eframe::egui;
use http_inspector_exchange::Exchange;

use crate::ui::requests::outcome_view;
use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};
use crate::ui::workbench::text::{self, ShellText};

/// One phase and its time, or `None` when the proxy did not observe it.
struct Phase {
    label: &'static str,
    millis: Option<f64>,
    /// The time keeps growing while the exchange is in flight.
    running: bool,
}

pub(super) fn show(ui: &mut egui::Ui, record: &Exchange) {
    let phases = phases(record);
    let label_width = phases
        .iter()
        .map(|phase| label_galley(ui, phase.label).size().x)
        .fold(0.0, f32::max);
    for phase in &phases {
        row(ui, phase, label_width);
    }
}

fn phases(record: &Exchange) -> [Phase; 6] {
    let timing = record.timing();
    let millis = |duration: Option<Duration>| duration.map(outcome_view::elapsed_millis);
    let in_flight = record.outcome().is_in_flight();
    let settled = if in_flight {
        Some(outcome_view::elapsed_millis(record.elapsed()))
    } else {
        millis(timing.settled())
    };
    [
        Phase {
            label: ShellText::Connect.text(),
            millis: millis(timing.connect()),
            running: false,
        },
        Phase {
            label: ShellText::Send.text(),
            millis: millis(timing.send()),
            running: false,
        },
        Phase {
            label: ShellText::Wait.text(),
            millis: millis(timing.wait()),
            running: false,
        },
        Phase {
            label: ShellText::Receive.text(),
            millis: millis(timing.receive()),
            running: false,
        },
        Phase {
            label: outcome_view::timing_label(record.outcome()),
            millis: settled,
            running: in_flight,
        },
        Phase {
            label: ShellText::Total.text(),
            millis: millis(timing.complete()),
            running: false,
        },
    ]
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
fn row(ui: &mut egui::Ui, phase: &Phase, label_width: f32) {
    let palette = Palette::of(ui);
    let value = phase.millis.map_or_else(
        || ShellText::NotObserved.text().to_owned(),
        |millis| {
            if phase.running {
                text::milliseconds_running(millis)
            } else {
                text::milliseconds(millis)
            }
        },
    );
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
