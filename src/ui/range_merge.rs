//! Showing the byte range of a `206` response and merging range responses.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

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
    /// A plan being made on a background thread.
    planning: Option<Receiver<io::Result<Option<RangePlan>>>>,
    plan: Option<RangePlan>,
}

impl MergeControls {
    /// Reports whether the dialog is open or about to open.
    pub(in crate::ui) const fn is_open(&self) -> bool {
        self.requested.is_some() || self.planning.is_some() || self.plan.is_some()
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
        "{} responses cover {} of {total} · {validator}. The file will be {}.",
        plan.parts(),
        format_size(plan.covered_bytes()),
        format_size(plan.file_bytes())
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
        if plan.validator().is_none() {
            ui.colored_label(
                palette.warning,
                "Without ETag or Last-Modified, parts of different versions of this URL may be mixed.",
            );
        }
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
            self.start_range_plan(&record);
        }
        self.poll_range_plan(context);
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

impl InspectorApp {
    /// Starts planning a merge for `record` on a background thread.
    fn start_range_plan(&mut self, record: &Exchange) {
        let Some(store) = self.body_saving.store() else {
            return;
        };
        let directory = PathBuf::new();
        let response = |exchange: &Exchange| {
            SaveBody::new(
                exchange,
                BodySide::Response,
                SaveFormat::AsReceived,
                directory.clone(),
            )
        };
        let Some(target) = response(record) else {
            return;
        };
        let candidates: Vec<SaveBody> = self.records.iter().filter_map(response).collect();
        match store.plan_ranges_in_background(target, candidates) {
            Ok(receiver) => self.body_saving.merge.planning = Some(receiver),
            Err(error) => self.error = Some(format!("Merging ranges failed: {error}")),
        }
    }

    /// Shows a waiting dialog until the plan arrives, then keeps it for the merge dialog.
    fn poll_range_plan(&mut self, context: &egui::Context) {
        let Some(receiver) = self.body_saving.merge.planning.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                egui::Modal::new(egui::Id::unique("plan-ranges")).show(context, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking the range responses…");
                    });
                });
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(io::Error::other(
                "The range planning worker stopped unexpectedly",
            )),
        };
        self.body_saving.merge.planning = None;
        match result {
            Ok(plan) => self.body_saving.merge.plan = plan,
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
