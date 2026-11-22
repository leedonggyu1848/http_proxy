//! Showing the byte range of a `206` response and merging range responses.

use std::path::PathBuf;

use eframe::egui;
use http_proxy::{BodySide, Exchange, RangePlan, SaveBody, SaveFormat};

use crate::ui::InspectorApp;
use crate::ui::body_saving::{PRIVACY_NOTICE, format_size};
use crate::ui::theme::Palette;

/// Gaps listed in the dialog before the rest are counted.
const SHOWN_GAPS: usize = 8;

/// The `Merge ranges…` request and the dialog that confirms it.
#[derive(Default)]
pub(in crate::ui) struct MergeControls {
    requested: Option<Exchange>,
    plan: Option<RangePlan>,
}

impl MergeControls {
    /// Reports whether the dialog is open or about to open.
    pub(in crate::ui) const fn is_open(&self) -> bool {
        self.requested.is_some() || self.plan.is_some()
    }

    /// Adds the byte range and `Merge ranges…` to a response save menu.
    /// Returns whether the menu should close.
    pub(in crate::ui) fn menu_items(&mut self, ui: &mut egui::Ui, record: &Exchange) -> bool {
        let Some(range) = response_range(record) else {
            return false;
        };
        ui.separator();
        ui.weak(range_text(range.0, range.1, range.2));
        let clicked = ui
            .button("Merge ranges…")
            .on_hover_text(
                "Join the 206 responses for this URL with the same ETag or Last-Modified into one file, in position order.",
            )
            .clicked();
        if clicked {
            self.requested = Some(record.clone());
        }
        clicked
    }
}

/// Returns `(start, end, total)` of a `206` response body.
fn response_range(record: &Exchange) -> Option<(u64, u64, Option<u64>)> {
    let body = SaveBody::new(
        record,
        BodySide::Response,
        SaveFormat::AsReceived,
        PathBuf::new(),
    )?;
    let range = body.content_range()?;
    Some((range.start(), range.end(), range.total()))
}

fn range_text(start: u64, end: u64, total: Option<u64>) -> String {
    let total = total.map_or_else(|| "unknown".to_owned(), |total| total.to_string());
    format!("206 range: bytes {start}-{end} of {total}")
}

fn plan_text(plan: &RangePlan) -> String {
    let total = plan
        .total()
        .map_or_else(|| "unknown length".to_owned(), format_size);
    let validator = plan.validator().map_or_else(
        || "no ETag or Last-Modified".to_owned(),
        |value| format!("validator {value}"),
    );
    format!(
        "{} responses cover {} of {total} · {validator}",
        plan.parts(),
        format_size(plan.covered_bytes())
    )
}

fn gap_lines(plan: &RangePlan) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .gaps()
        .iter()
        .take(SHOWN_GAPS)
        .map(|(first, last)| {
            format!(
                "Missing bytes {first}-{last} ({})",
                format_size(last - first + 1)
            )
        })
        .collect();
    if plan.gaps().len() > SHOWN_GAPS {
        lines.push(format!(
            "… and {} more gaps",
            plan.gaps().len() - SHOWN_GAPS
        ));
    }
    lines
}

/// Shows the plan and returns `Some(true)` to save, `Some(false)` to cancel.
fn dialog(context: &egui::Context, plan: &RangePlan) -> Option<bool> {
    let mut choice = None;
    egui::Modal::new(egui::Id::unique("merge-ranges")).show(context, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Merge ranges");
        ui.label(plan_text(plan));
        let palette = Palette::of(ui);
        for line in gap_lines(plan) {
            ui.colored_label(palette.warning, line);
        }
        if let Some(reason) = plan.incomplete_reason() {
            ui.colored_label(
                palette.warning,
                format!("{reason} The file name will end in .partial."),
            );
        }
        ui.label(PRIVACY_NOTICE);
        ui.horizontal(|ui| {
            let save = if plan.is_complete() {
                "Save"
            } else {
                "Save incomplete file"
            };
            if ui.button(save).clicked() {
                choice = Some(true);
            }
            if ui.button("Cancel").clicked() {
                choice = Some(false);
            }
        });
    });
    choice
}

impl InspectorApp {
    /// Plans a requested merge and shows its dialog; call once per frame.
    pub(in crate::ui) fn range_merge(&mut self, context: &egui::Context) {
        if let Some(record) = self.body_saving.merge.requested.take() {
            let plan = self.body_saving.store().and_then(|store| {
                let directory = PathBuf::new();
                let target = SaveBody::new(
                    &record,
                    BodySide::Response,
                    SaveFormat::AsReceived,
                    directory.clone(),
                )?;
                let candidates: Vec<SaveBody> = self
                    .records
                    .iter()
                    .filter_map(|other| {
                        SaveBody::new(
                            other,
                            BodySide::Response,
                            SaveFormat::AsReceived,
                            directory.clone(),
                        )
                    })
                    .collect();
                Some(store.plan_ranges(&target, &candidates))
            });
            match plan {
                Some(Ok(plan)) => self.body_saving.merge.plan = plan,
                Some(Err(error)) => self.error = Some(format!("Merging ranges failed: {error}")),
                None => {}
            }
        }
        let Some(plan) = self.body_saving.merge.plan.as_ref() else {
            return;
        };
        let Some(save) = dialog(context, plan) else {
            return;
        };
        let Some(plan) = self.body_saving.merge.plan.take() else {
            return;
        };
        if !save {
            return;
        }
        let started = self.save.directory().and_then(|directory| {
            self.body_saving
                .store()
                .ok_or_else(|| std::io::Error::other("Saving bodies is unavailable"))?
                .save_ranges(plan, directory)
        });
        match started {
            Ok(receiver) => self.body_saving.track(receiver),
            Err(error) => self.error = Some(format!("Merging ranges failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_ranges() {
        assert_eq!(
            range_text(0, 99, Some(1000)),
            "206 range: bytes 0-99 of 1000"
        );
        assert_eq!(range_text(5, 9, None), "206 range: bytes 5-9 of unknown");
    }
}
