//! Chrome bands: the toolbar, pane headers, notice rows, and the status bar.
//!
//! A band is a full-width strip on `surfaces.chrome` with one
//! `borders.structural` hairline that separates it from the pane next to it
//! (D10, D22). Its contents are laid out left to right and centered
//! vertically, so every band of the same height lines up the same way in
//! both themes.

use eframe::egui;

use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};

/// Which side of a band touches the pane it separates from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Edge {
    /// Toolbars and pane headers sit above their pane.
    Bottom,
    /// The status bar sits below the panes.
    Top,
}

/// Horizontal inset of band contents: the toolbar and status bar use
/// [`Space::MD`] on both sides; pane headers keep their right-hand actions
/// closer to the edge, as in the concept.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inset {
    pub(crate) left: f32,
    pub(crate) right: f32,
}

impl Inset {
    pub(crate) const BAR: Self = Self {
        left: Space::MD,
        right: Space::MD,
    };
    pub(crate) const PANE_HEADER: Self = Self {
        left: Space::MD,
        right: Space::SM,
    };
}

/// Allocates a band of `height` across the available width, paints it, and
/// lays `add` out inside it from left to right.
pub(crate) fn band<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    height: f32,
    edge: Edge,
    inset: Inset,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let rect = allocate(ui, height);
    paint(ui, rect, edge);
    let mut child = row(ui, rect, inset, id_salt);
    add(&mut child)
}

/// Reserves a strip of `height` across the available width. The next item
/// starts right below it: a band meets its pane at the hairline, with no
/// item spacing in between.
pub(crate) fn allocate(ui: &mut egui::Ui, height: f32) -> egui::Rect {
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.spacing_mut().item_spacing.y = spacing;
    rect
}

/// A child of `ui` that lays its contents out left to right inside `rect`,
/// inset by `inset` and centered vertically.
pub(crate) fn row(ui: &mut egui::Ui, rect: egui::Rect, inset: Inset, id_salt: &str) -> egui::Ui {
    let inner = egui::Rect::from_x_y_ranges(
        (rect.left() + inset.left)..=(rect.right() - inset.right).max(rect.left() + inset.left),
        rect.y_range(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.shrink_clip_rect(rect);
    child
}

/// Fills `rect` with the chrome surface and draws its separating hairline.
pub(crate) fn paint(ui: &egui::Ui, rect: egui::Rect, edge: Edge) {
    let palette = Palette::of(ui);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, palette.surfaces.chrome);
    let stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, palette.borders.structural);
    let y = match edge {
        Edge::Bottom => rect.bottom() - StrokeWidth::STRUCTURAL / 2.0,
        Edge::Top => rect.top() + StrokeWidth::STRUCTURAL / 2.0,
    };
    painter.hline(rect.x_range(), y, stroke);
}

/// A pane header: [`Layout::PANE_HEADER_HEIGHT`] tall, names or tabs on the
/// left and the pane's own actions on the right.
pub(crate) fn pane_header<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    band(
        ui,
        id_salt,
        Layout::PANE_HEADER_HEIGHT,
        Edge::Bottom,
        Inset::PANE_HEADER,
        add,
    )
}

/// The 1px vertical line between toolbar groups.
pub(crate) fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(StrokeWidth::STRUCTURAL, Layout::TOOLBAR_SEPARATOR_HEIGHT),
        egui::Sense::hover(),
    );
    let stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.structural);
    ui.painter().vline(rect.center().x, rect.y_range(), stroke);
}

/// Makes the labels of `ui` status bar text: monospace 11pt in
/// `text.secondary` (D15, D22).
pub(crate) fn status_text(ui: &mut egui::Ui) {
    let secondary = Palette::of(ui).secondary;
    let style = ui.style_mut();
    style.override_font_id = Some(fonts::status());
    style.visuals.override_text_color = Some(secondary);
    style.spacing.item_spacing.x = Space::LG;
}
