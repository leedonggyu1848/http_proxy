//! Entries of the status bar and how they share its one 24pt row.
//!
//! The bar never wraps. When the entries do not fit, the least important
//! ones are left out and a `+N` entry at the end names them in its tooltip,
//! so a warning never disappears without a trace.

use eframe::egui;

use crate::ui::style::tokens::{Layout, Space, Tone};
use crate::ui::style::{fonts, indicator};

/// How much an entry matters when the bar is too narrow for all of them.
/// Lower values stay longer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Priority {
    /// Request counts, which the bar is for.
    Count,
    /// Something failed or was lost.
    Error,
    /// A limit was reached or work is partial.
    Warning,
    /// Work in progress.
    Progress,
}

/// One status bar entry: text, an optional state dot, and a one-sentence
/// tooltip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StatusItem {
    pub(crate) text: String,
    pub(crate) tone: Option<Tone>,
    pub(crate) detail: Option<String>,
    pub(crate) priority: Priority,
    /// Shows a spinner before the text while work runs.
    pub(crate) spinner: bool,
}

impl StatusItem {
    pub(crate) const fn count(text: String) -> Self {
        Self {
            text,
            tone: None,
            detail: None,
            priority: Priority::Count,
            spinner: false,
        }
    }

    /// An entry with a dot in `tone`, kept as an error or a warning.
    pub(crate) fn marked(tone: Tone, text: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Some(tone),
            detail: Some(detail.into()),
            priority: if tone == Tone::Danger {
                Priority::Error
            } else {
                Priority::Warning
            },
            spinner: false,
        }
    }

    pub(crate) const fn progress(text: String) -> Self {
        Self {
            text,
            tone: None,
            detail: None,
            priority: Priority::Progress,
            spinner: true,
        }
    }

    fn width(&self, ui: &egui::Ui) -> f32 {
        let text = ui
            .painter()
            .layout_no_wrap(
                self.text.clone(),
                fonts::status(),
                egui::Color32::PLACEHOLDER,
            )
            .size()
            .x;
        let mark = if self.tone.is_some() || self.spinner {
            Layout::STATUS_MARK_WIDTH + Space::S
        } else {
            0.0
        };
        text + mark
    }
}

/// Indices of `widths`, in their order, that fit in `available` with
/// `spacing` between them, keeping lower priorities first. `overflow` is
/// the width of the `+N` entry reserved when some are left out.
fn fitting(widths: &[(f32, Priority)], available: f32, spacing: f32, overflow: f32) -> Vec<usize> {
    let total: f32 = widths.iter().map(|(width, _)| width + spacing).sum();
    if total <= available {
        return (0..widths.len()).collect();
    }
    let mut order: Vec<usize> = (0..widths.len()).collect();
    order.sort_by_key(|&index| widths.get(index).map(|(_, priority)| *priority));
    let mut budget = available - overflow - spacing;
    let mut kept = Vec::new();
    for index in order {
        let Some((width, _)) = widths.get(index) else {
            continue;
        };
        if width + spacing <= budget {
            budget -= width + spacing;
            kept.push(index);
        }
    }
    kept.sort_unstable();
    kept
}

/// Draws the entries that fit in `available` points, most important first,
/// and a `+N` entry for the rest.
pub(crate) fn show(ui: &mut egui::Ui, items: &[StatusItem], available: f32) {
    let spacing = ui.spacing().item_spacing.x;
    let widths: Vec<(f32, Priority)> = items
        .iter()
        .map(|item| (item.width(ui), item.priority))
        .collect();
    let overflow_width = ui
        .painter()
        .layout_no_wrap(
            super::text::more_entries(items.len()),
            fonts::status(),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x;
    let kept = fitting(&widths, available, spacing, overflow_width);
    for &index in &kept {
        if let Some(item) = items.get(index) {
            entry(ui, item);
        }
    }
    let hidden: Vec<&StatusItem> = items
        .iter()
        .enumerate()
        .filter(|(index, _)| !kept.contains(index))
        .map(|(_, item)| item)
        .collect();
    if !hidden.is_empty() {
        let names: Vec<&str> = hidden.iter().map(|item| item.text.as_str()).collect();
        ui.label(super::text::more_entries(hidden.len()))
            .on_hover_text(names.join("\n"));
    }
}

fn entry(ui: &mut egui::Ui, item: &StatusItem) {
    let response = if let Some(tone) = item.tone {
        indicator::dot_label(ui, tone, item.text.as_str())
    } else if item.spinner {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Space::S;
            ui.add(egui::Spinner::new().size(Layout::STATUS_MARK_WIDTH));
            ui.label(item.text.as_str())
        })
        .inner
    } else {
        ui.label(item.text.as_str())
    };
    if let Some(detail) = &item.detail {
        response.on_hover_text(detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_every_entry_that_fits_in_order() {
        let widths = [(40.0, Priority::Count), (30.0, Priority::Warning)];
        assert_eq!(fitting(&widths, 200.0, 10.0, 20.0), [0, 1]);
    }

    #[test]
    fn drops_the_least_important_entries_first() {
        let widths = [
            (60.0, Priority::Count),
            (50.0, Priority::Progress),
            (50.0, Priority::Warning),
            (50.0, Priority::Error),
        ];
        // Room for the count, the error, and the `+N` entry only.
        assert_eq!(fitting(&widths, 160.0, 0.0, 20.0), [0, 3]);
        // The order on screen stays the order of the entries.
        assert_eq!(fitting(&widths, 200.0, 0.0, 20.0), [0, 2, 3]);
    }
}
