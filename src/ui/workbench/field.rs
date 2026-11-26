//! Single-line inputs in the workbench style (D25).
//!
//! egui outlines a focused text edit in the selection stroke, which this app
//! keeps at `text.primary` so selected text stays readable (D8). The input
//! here draws its own frame instead: `borders.control` at rest,
//! `accent.secondary` plus a 1px outer line while focused, and
//! `status.error` with a short note at the right end while its value is
//! invalid.

use std::sync::Arc;

use eframe::egui;

use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens};

/// How one input looks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Field<'a> {
    width: f32,
    hint: &'a str,
    search_icon: bool,
    monospace: bool,
    password: bool,
    error: Option<&'a str>,
}

impl<'a> Field<'a> {
    pub(crate) const fn new(width: f32) -> Self {
        Self {
            width,
            hint: "",
            search_icon: false,
            monospace: false,
            password: false,
            error: None,
        }
    }

    /// Placeholder shown while empty; also the input's accessible name.
    pub(crate) const fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    /// Shows a magnifier at the left end, as the label of a filter input.
    pub(crate) const fn search_icon(mut self) -> Self {
        self.search_icon = true;
        self
    }

    /// Edits data, such as a port or a pattern, in the monospace font.
    pub(crate) const fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    /// Hides the value, such as a credential, while `hidden` is true.
    pub(crate) const fn password(mut self, hidden: bool) -> Self {
        self.password = hidden;
        self
    }

    /// Marks the value invalid and shows `short` at the right end.
    pub(crate) const fn error(mut self, short: Option<&'a str>) -> Self {
        self.error = short;
        self
    }
}

/// Shows `text` in a [`Layout::INPUT_HEIGHT`] input of `field`'s width.
/// `trailing` adds small controls inside the right end, such as match
/// options. Returns the text edit's response.
pub(crate) fn single_line(
    ui: &mut egui::Ui,
    text: &mut String,
    field: Field<'_>,
    trailing: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    single_line_with(ui, text, field, trailing, None, egui::Ui::add)
}

pub(crate) type Layouter<'a> =
    dyn FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> Arc<egui::Galley> + 'a;

/// Adapts the text edit while keeping the standard workbench frame and controls.
pub(crate) fn single_line_with<'a>(
    ui: &mut egui::Ui,
    text: &'a mut String,
    field: Field<'_>,
    trailing: impl FnOnce(&mut egui::Ui),
    layouter: Option<&'a mut Layouter<'a>>,
    show: impl FnOnce(&mut egui::Ui, egui::TextEdit<'a>) -> egui::Response,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(field.width, Layout::INPUT_HEIGHT),
        egui::Sense::hover(),
    );
    let palette = Palette::of(ui);
    ui.painter()
        .rect_filled(rect, Radius::CONTROL, palette.surfaces.input);
    let mut content = rect.shrink2(egui::vec2(Space::SM, 0.0));
    if field.search_icon {
        paint_magnifier(
            ui,
            content.left() + Layout::ICON_SMALL / 2.0,
            content.center().y,
        );
        content.min.x += Layout::ICON_SMALL + Space::S;
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.shrink_clip_rect(content);
    child.spacing_mut().item_spacing.x = Space::XS;
    trailing(&mut child);
    if let Some(short) = field.error {
        let error = ThemeTokens::for_theme(ui.ctx().theme()).status.error;
        child.label(
            egui::RichText::new(short)
                .font(fonts::input_error())
                .color(error),
        );
    }
    let font = if field.monospace {
        fonts::data()
    } else {
        fonts::control()
    };
    let mut edit = egui::TextEdit::singleline(text)
        .frame(egui::Frame::NONE)
        .font(font)
        .hint_text(field.hint)
        .password(field.password)
        .align(egui::Align2::LEFT_CENTER)
        .min_size(egui::vec2(0.0, content.height()))
        .desired_width(child.available_width());
    if let Some(layouter) = layouter {
        edit = edit.layouter(layouter);
    }
    let response = show(&mut child, edit);
    paint_border(ui, rect, response.has_focus(), field.error.is_some());
    response
}

/// A multi-line monospace text area of `rows` rows across the available
/// width, with the same frame and focus border as [`single_line`].
pub(crate) fn multi_line(ui: &mut egui::Ui, text: &mut String, rows: usize) -> egui::Response {
    let palette = Palette::of(ui);
    let frame = egui::Frame::new()
        .fill(palette.surfaces.input)
        .corner_radius(Radius::CONTROL)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .frame(egui::Frame::NONE)
                    .font(fonts::data())
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY),
            )
        });
    let response = frame.inner;
    paint_border(ui, frame.response.rect, response.has_focus(), false);
    response
}

fn paint_border(ui: &egui::Ui, rect: egui::Rect, focused: bool, invalid: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let color = if invalid {
        tokens.status.error
    } else if focused {
        tokens.accent.secondary
    } else {
        tokens.borders.control
    };
    let width = StrokeWidth::STRUCTURAL;
    ui.painter().rect_stroke(
        rect,
        Radius::CONTROL,
        egui::Stroke::new(width, color),
        egui::StrokeKind::Inside,
    );
    if focused {
        ui.painter().rect_stroke(
            rect,
            Radius::CONTROL,
            egui::Stroke::new(StrokeWidth::FOCUS_RING, tokens.accent.secondary),
            egui::StrokeKind::Outside,
        );
    }
}

/// A 14px line magnifier in `text.muted`, the inactive icon color (D8, D17).
fn paint_magnifier(ui: &egui::Ui, center_x: f32, center_y: f32) {
    let color = ThemeTokens::for_theme(ui.ctx().theme()).text.muted;
    let stroke = egui::Stroke::new(StrokeWidth::ICON, color);
    let size = Layout::ICON_SMALL;
    let lens_radius = size * 0.3;
    let lens = egui::pos2(size.mul_add(-0.08, center_x), size.mul_add(-0.08, center_y));
    let painter = ui.painter();
    painter.circle_stroke(lens, lens_radius, stroke);
    let handle_start =
        lens + egui::vec2(lens_radius, lens_radius) * std::f32::consts::FRAC_1_SQRT_2;
    let handle_end = egui::pos2(size.mul_add(0.4, center_x), size.mul_add(0.4, center_y));
    painter.line_segment([handle_start, handle_end], stroke);
}
