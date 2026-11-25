//! Shared control styles: the primary action, tone chips, segmented
//! switches, and counted filter chips.
//!
//! Every control here is an ordinary egui button underneath, so it keeps
//! keyboard focus, `Space`/`Enter` activation, and an accessible name.

use eframe::egui::{self, Atoms, Button, CornerRadius, Response, RichText, Stroke};

use super::badge::Badge;
use super::tokens::{Radius, Space, ThemeTokens, Tone, TypeScale};

/// The one emphasized action of an area, drawn on the accent fill.
pub(crate) fn primary_button<'a>(ui: &egui::Ui, text: impl Into<String>) -> Button<'a> {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    Button::new(RichText::new(text).color(tokens.on_accent))
        .fill(tokens.accent)
        .stroke(Stroke::new(1.0, tokens.accent))
}

/// A rounded, tone-tinted button that states a status and opens details.
pub(crate) fn chip_button<'a>(ui: &egui::Ui, text: impl Into<String>, tone: Tone) -> Button<'a> {
    let colors = tone.colors(ui.ctx().theme());
    Button::new(RichText::new(text).color(colors.fg))
        .fill(colors.fill)
        .stroke(Stroke::new(1.0, colors.fill))
        .corner_radius(CornerRadius::same(u8::MAX))
}

/// One option of a [`segmented`] switch.
pub(crate) struct Segment<'a, T> {
    pub(crate) value: T,
    pub(crate) label: &'a str,
    /// Secondary text after the label, such as a count.
    pub(crate) detail: Option<String>,
    pub(crate) hover: Option<String>,
}

/// A row of mutually exclusive options in one inset track, such as the
/// main view switch. Each option is announced as a selected or unselected
/// tab. Returns whether the value changed.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    value: &mut T,
    segments: &[Segment<'_, T>],
) -> bool {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let mut changed = false;
    egui::Frame::new()
        .fill(tokens.surfaces.inset)
        .stroke(Stroke::new(1.0, tokens.surfaces.border))
        .corner_radius(CornerRadius::same(Radius::CONTROL))
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for segment in segments {
                    let selected = *value == segment.value;
                    let response = segment_button(ui, segment, selected, &tokens);
                    if response.clicked() && !selected {
                        *value = segment.value;
                        changed = true;
                    }
                }
            });
        });
    changed
}

fn segment_button<T>(
    ui: &mut egui::Ui,
    segment: &Segment<'_, T>,
    selected: bool,
    tokens: &ThemeTokens,
) -> Response {
    let color = if selected { tokens.text } else { tokens.muted };
    let mut label = RichText::new(segment.label).color(color);
    if selected {
        label = label.strong();
    }
    let mut atoms = Atoms::new(label);
    if let Some(detail) = &segment.detail {
        atoms.push_right(
            RichText::new(detail)
                .size(TypeScale::SMALL)
                .color(tokens.muted),
        );
    }
    let (fill, stroke) = if selected {
        (
            tokens.surfaces.raised,
            Stroke::new(1.0, tokens.surfaces.border),
        )
    } else {
        (egui::Color32::TRANSPARENT, Stroke::NONE)
    };
    let response = ui.add(
        Button::new(atoms)
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(Radius::CONTROL - 2))
            .gap(Space::XS + 2.0),
    );
    let name = segment.detail.as_ref().map_or_else(
        || segment.label.to_owned(),
        |detail| format!("{} ({detail})", segment.label),
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, &name));
    // A tab is selected, not toggled like a checkbox.
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.clear_toggled();
        node.set_selected(selected);
    });
    match &segment.hover {
        Some(hover) => response.on_hover_text(hover),
        None => response,
    }
}

/// A selectable chip with a label and a count, such as `Failures 3`.
/// Announced as a radio button named `Failures (3)`.
pub(crate) fn count_chip(ui: &mut egui::Ui, selected: bool, label: &str, count: usize) -> Response {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let (fill, stroke, color) = if selected {
        (
            tokens.surfaces.selection,
            Stroke::new(1.0, tokens.accent),
            tokens.text,
        )
    } else {
        (
            egui::Color32::TRANSPARENT,
            Stroke::new(1.0, tokens.surfaces.border),
            tokens.text,
        )
    };
    let atoms = (
        RichText::new(label).color(color),
        RichText::new(count.to_string())
            .size(TypeScale::SMALL)
            .color(if selected { tokens.text } else { tokens.muted }),
    );
    let response = ui.add(
        Button::new(atoms)
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(u8::MAX))
            .gap(Space::XS + 2.0),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::RadioButton,
            ui.is_enabled(),
            selected,
            format!("{label} ({count})"),
        )
    });
    response
}

/// Header line of a numbered step.
pub(crate) struct StepHeader<'a> {
    /// The step number; `None` draws a check mark for a finished step.
    pub(crate) number: Option<usize>,
    pub(crate) title: &'a str,
    /// Status in words, such as `Needed`.
    pub(crate) status: &'a str,
    pub(crate) tone: Tone,
    /// Dims the title of a step that cannot be done yet.
    pub(crate) muted: bool,
}

/// A numbered step: a marker circle, the title and a status badge, then an
/// indented explanation and, if any, a row of buttons added by `actions`.
pub(crate) fn step_card(
    ui: &mut egui::Ui,
    header: &StepHeader<'_>,
    detail: &str,
    actions: Option<impl FnOnce(&mut egui::Ui)>,
) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    ui.horizontal(|ui| {
        step_marker(ui, header.number, header.tone);
        let title = RichText::new(header.title).strong().color(if header.muted {
            tokens.muted
        } else {
            tokens.text
        });
        ui.label(title);
        ui.add(Badge::status(header.status, header.tone));
    });
    let indent = STEP_MARKER + ui.spacing().item_spacing.x;
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(detail).color(tokens.muted)).wrap());
            if let Some(actions) = actions {
                ui.horizontal_wrapped(actions);
            }
        });
    });
}

/// Diameter of a step marker.
const STEP_MARKER: f32 = 20.0;

/// A tinted circle with the step number, or a painted check mark, so the
/// marker does not depend on a font having a check glyph.
fn step_marker(ui: &mut egui::Ui, number: Option<usize>, tone: Tone) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(STEP_MARKER), egui::Sense::hover());
    let colors = tone.colors(ui.ctx().theme());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), STEP_MARKER / 2.0, colors.fill);
    if let Some(number) = number {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            number.to_string(),
            egui::FontId::proportional(TypeScale::SMALL),
            colors.fg,
        );
    } else {
        let center = rect.center();
        let points = vec![
            center + egui::vec2(-4.5, 0.0),
            center + egui::vec2(-1.5, 3.0),
            center + egui::vec2(4.5, -3.5),
        ];
        painter.add(egui::Shape::line(points, Stroke::new(2.0, colors.fg)));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn accessible_nodes(output: &egui::FullOutput) -> Vec<(egui::Role, String, Option<bool>)> {
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .map_or_else(Vec::new, |tree| {
                tree.nodes
                    .iter()
                    .filter_map(|(_, node)| {
                        node.label()
                            .map(|label| (node.role(), label.to_owned(), checked(node)))
                    })
                    .collect()
            })
    }

    /// Selection of a tab, or the checked state of a radio button.
    pub(crate) fn checked(node: &egui::accesskit::Node) -> Option<bool> {
        node.is_selected().or_else(|| {
            node.toggled()
                .map(|toggled| toggled == egui::accesskit::Toggled::True)
        })
    }

    #[test]
    fn segments_and_chips_announce_their_name_and_selection() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum View {
            List,
            Map,
        }
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut view = View::List;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _changed = segmented(
                ui,
                &mut view,
                &[
                    Segment {
                        value: View::List,
                        label: "Requests",
                        detail: Some("17".to_owned()),
                        hover: None,
                    },
                    Segment {
                        value: View::Map,
                        label: "Map",
                        detail: None,
                        hover: None,
                    },
                ],
            );
            let _chip = count_chip(ui, false, "Failures", 3);
        });
        output.textures_delta.clear();
        let nodes = accessible_nodes(&output);
        assert!(nodes.contains(&(egui::Role::Tab, "Requests (17)".to_owned(), Some(true))));
        assert!(nodes.contains(&(egui::Role::Tab, "Map".to_owned(), Some(false))));
        assert!(nodes.contains(&(
            egui::Role::RadioButton,
            "Failures (3)".to_owned(),
            Some(false)
        )));
    }

    #[test]
    fn control_text_meets_aa_on_its_fill() {
        use super::super::tokens::contrast_ratio;
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let tokens = ThemeTokens::for_theme(theme);
            for (text, fill) in [
                (tokens.text, tokens.surfaces.raised),
                (tokens.muted, tokens.surfaces.inset),
                (tokens.text, tokens.surfaces.selection),
                (tokens.muted, tokens.surfaces.bg),
            ] {
                let contrast = contrast_ratio(text, fill);
                assert!(
                    contrast >= 4.5,
                    "{theme:?} {text:?} on {fill:?}: {contrast}"
                );
            }
        }
    }
}
