//! Controls drawn on chrome bands: text tabs, the segmented view switch,
//! and Ghost and Primary buttons (D25).
//!
//! Tabs are plain text with a 2px `accent.primary` bar under the active one;
//! they have no background, pill, or count (S11). Keyboard focus is a 1px
//! `accent.secondary` ring (D3).

use eframe::egui::{self, Role, accesskit};

use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens};

/// Groups the tabs added by `add` into one accessible tab list named `label`.
///
/// The scope keeps the parent's left-to-right layout, so the list stays as
/// tall as the band it sits in.
pub(crate) fn tab_list<R>(
    ui: &mut egui::Ui,
    label: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope_builder(
        egui::UiBuilder::new()
            .accessibility_role(accesskit::Role::TabList)
            .accessibility_label(label),
        |ui| {
            ui.spacing_mut().item_spacing.x = Space::LG;
            add(ui)
        },
    )
    .inner
}

/// One text tab that fills the height of its band. The active tab is
/// `text.primary` with a bar along the bottom edge; the others are
/// `text.secondary` until hovered.
pub(crate) fn text_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let palette = Palette::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), fonts::control(), palette.text);
    let height = ui.available_height().max(galley.size().y);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(galley.size().x, height), egui::Sense::click());
    announce_tab(ui, &response, selected, label);
    if ui.is_rect_visible(rect) {
        let color = if selected || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        let text_pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, color);
        if selected {
            paint_active_bar(ui, rect.x_range(), rect.bottom() - StrokeWidth::STRUCTURAL);
        }
        if response.has_focus() {
            paint_focus_ring(ui, rect.expand2(egui::vec2(Space::XS, -Space::XXS)));
        }
    }
    response
}

/// Requests and Map, shown as a bordered group of segments. The active
/// segment has the `surfaces.selected` fill and a bar under its text, as an
/// active Ghost toggle does. Returns the option the user picked.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    tab_list(ui, label, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut chosen = None;
        let mut outline: Option<egui::Rect> = None;
        let last = options.len().saturating_sub(1);
        for (index, &(value, text)) in options.iter().enumerate() {
            let selected = value == current;
            let corners = segment_corners(index == 0, index == last);
            let response = segment(ui, selected, text, corners);
            if index > 0 {
                let stroke = control_stroke(ui);
                ui.painter()
                    .vline(response.rect.left(), response.rect.y_range(), stroke);
            }
            outline = Some(outline.map_or(response.rect, |rect| rect.union(response.rect)));
            if response.clicked() && !selected {
                chosen = Some(value);
            }
        }
        if let Some(rect) = outline {
            ui.painter().rect_stroke(
                rect,
                Radius::CONTROL,
                control_stroke(ui),
                egui::StrokeKind::Inside,
            );
        }
        chosen
    })
}

fn segment(
    ui: &mut egui::Ui,
    selected: bool,
    text: &str,
    corners: egui::CornerRadius,
) -> egui::Response {
    let palette = Palette::of(ui);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), fonts::control(), palette.text);
    let size = egui::vec2(
        2.0f32.mul_add(Space::MD, galley.size().x),
        Layout::TOOLBAR_BUTTON_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    announce_tab(ui, &response, selected, text);
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect, corners, palette.surfaces.selected);
            let inset = rect.shrink2(egui::vec2(Space::SM, 0.0));
            paint_active_bar(ui, inset.x_range(), rect.bottom());
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, corners, palette.surfaces.hover);
        }
        let color = if selected || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
        if response.has_focus() {
            paint_focus_ring(ui, rect.shrink(StrokeWidth::FOCUS_RING));
        }
    }
    response
}

const fn segment_corners(first: bool, last: bool) -> egui::CornerRadius {
    let left = if first { Radius::CONTROL } else { 0 };
    let right = if last { Radius::CONTROL } else { 0 };
    egui::CornerRadius {
        nw: left,
        sw: left,
        ne: right,
        se: right,
    }
}

/// Reports a tab with its selection state, not as a toggle button.
fn announce_tab(ui: &egui::Ui, response: &egui::Response, selected: bool, label: &str) {
    response
        .widget_info(|| egui::WidgetInfo::selected(Role::Tab, ui.is_enabled(), selected, label));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.clear_toggled();
        node.set_selected(selected);
    });
}

/// The 2px `accent.primary` bar that marks an active tab or toggle, ending
/// at `bottom`.
fn paint_active_bar(ui: &egui::Ui, x_range: egui::Rangef, bottom: f32) {
    let accent = ThemeTokens::for_theme(ui.ctx().theme()).accent.primary;
    let bar = egui::Rect::from_x_y_ranges(x_range, (bottom - Layout::SELECTION_BAR_WIDTH)..=bottom);
    ui.painter().rect_filled(bar, 0.0, accent);
}

/// The keyboard focus ring: 1px `accent.secondary` (D3).
pub(crate) fn paint_focus_ring(ui: &egui::Ui, rect: egui::Rect) {
    let ring = ThemeTokens::for_theme(ui.ctx().theme()).accent.secondary;
    ui.painter().rect_stroke(
        rect,
        Radius::CONTROL,
        egui::Stroke::new(StrokeWidth::FOCUS_RING, ring),
        egui::StrokeKind::Outside,
    );
}

fn control_stroke(ui: &egui::Ui) -> egui::Stroke {
    egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.control)
}

/// A Ghost button: no fill or border until hovered.
pub(crate) fn ghost<'a>(text: impl Into<egui::WidgetText>, height: f32) -> egui::Button<'a> {
    egui::Button::new(text)
        .frame_when_inactive(false)
        .min_size(egui::vec2(0.0, height))
}

/// A Ghost toggle. While on it has the `surfaces.selected` fill and a bar
/// along its bottom edge, like an active tab (D23, D25).
pub(crate) fn ghost_toggle(
    ui: &mut egui::Ui,
    on: bool,
    label: &str,
    height: f32,
) -> egui::Response {
    let response = ui.add(ghost(label, height).selected(on));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label);
        node.set_toggled(accesskit::Toggled::from(on));
    });
    if on {
        let inset = response.rect.shrink2(egui::vec2(Space::XS, 0.0));
        paint_active_bar(ui, inset.x_range(), response.rect.bottom());
    }
    response
}

/// The one Primary button of the window (S9, D25).
pub(crate) fn primary<'a>(ui: &egui::Ui, text: &str) -> egui::Button<'a> {
    let colors = ThemeTokens::for_theme(ui.ctx().theme()).primary_action;
    egui::Button::new(egui::RichText::new(text).color(colors.text))
        .fill(colors.fill)
        .stroke(egui::Stroke::new(StrokeWidth::STRUCTURAL, colors.stroke))
        .min_size(egui::vec2(0.0, Layout::TOOLBAR_BUTTON_HEIGHT))
}

/// Runs `add` with Ghost buttons that keep no border while hovered or
/// pressed, so only the `surfaces.hover` fill shows the pointer.
pub(crate) fn ghost_scope<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ghost_style(ui);
        add(ui)
    })
    .inner
}

/// Makes the buttons of `ui` keep no border while hovered or pressed. Use it
/// on a child that holds only Ghost buttons and labels, where a nested
/// scope would break the parent's wrapping.
pub(crate) fn ghost_style(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        state.bg_stroke = egui::Stroke::NONE;
    }
}

/// Width of `text` in the control font.
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            fonts::control(),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x
}

/// Width a button labelled `text` takes with the current padding.
pub(crate) fn button_width(ui: &egui::Ui, text: &str) -> f32 {
    2.0f32.mul_add(ui.spacing().button_padding.x, text_width(ui, text))
}

/// Width a checkbox labelled `text` takes.
pub(crate) fn checkbox_width(ui: &egui::Ui, text: &str) -> f32 {
    let spacing = ui.spacing();
    text_width(ui, text) + spacing.icon_width + spacing.icon_spacing
}

/// Width [`segmented`] takes for `labels`.
pub(crate) fn segmented_width<'a>(ui: &egui::Ui, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    labels
        .into_iter()
        .map(|label| 2.0f32.mul_add(Space::MD, text_width(ui, label)))
        .sum()
}

/// A Ghost button that opens a menu.
pub(crate) fn ghost_menu<R>(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    height: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ghost_scope(ui, |ui| {
        let (response, inner) =
            egui::containers::menu::MenuButton::from_button(ghost(text, height)).ui(ui, contents);
        egui::InnerResponse::new(inner.map(|inner| inner.inner), response)
    })
}

/// A Primary button that opens a menu.
pub(crate) fn primary_menu<R>(
    ui: &mut egui::Ui,
    text: &str,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let button = primary(ui, text);
    let (response, inner) =
        egui::containers::menu::MenuButton::from_button(button).ui(ui, contents);
    egui::InnerResponse::new(inner.map(|inner| inner.inner), response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_outer_segments_are_rounded() {
        assert_eq!(segment_corners(true, false).nw, Radius::CONTROL);
        assert_eq!(segment_corners(true, false).ne, 0);
        assert_eq!(segment_corners(false, true).se, Radius::CONTROL);
        assert_eq!(segment_corners(false, false), egui::CornerRadius::ZERO);
    }
}
