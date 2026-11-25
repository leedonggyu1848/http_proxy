//! State indicators without a box: a 6px dot or a 2×12 left tick beside
//! text. The text always states the meaning; the mark only reinforces it.

use eframe::egui;

use super::tokens::{Layout, Space, Tone};

/// A dot in `tone` followed by `text` in the default text color. Use it for
/// states shown on chrome surfaces (toolbar, notices, status bar), where
/// status colors are not readable as text.
pub(crate) fn dot_label(
    ui: &mut egui::Ui,
    tone: Tone,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = Space::S;
        dot(ui, tone);
        ui.label(text)
    })
    .inner
}

/// A 6px dot in `tone`, centered on a line of body text. Put text right
/// after it, `Space::S` away.
pub(crate) fn dot(ui: &mut egui::Ui, tone: Tone) {
    let height = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(Layout::DOT_SIZE, height), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(
            rect.center(),
            Layout::DOT_SIZE / 2.0,
            tone.color(ui.ctx().theme()),
        );
    }
}

/// Reserves the slot of a left tick and paints the tick when `tone` marks
/// an exception, so that the text after it lines up with or without a tick.
/// The caller sets the gap to the text.
pub(crate) fn tick(ui: &mut egui::Ui, tone: Option<Tone>) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(Layout::TICK_WIDTH, Layout::TICK_HEIGHT),
        egui::Sense::hover(),
    );
    if let Some(tone) = tone
        && ui.is_rect_visible(rect)
    {
        ui.painter()
            .rect_filled(rect, 0.0, tone.color(ui.ctx().theme()));
    }
}
