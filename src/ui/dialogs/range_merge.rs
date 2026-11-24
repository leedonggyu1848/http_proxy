//! Showing the byte range of a `206` response and merging range responses.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::recording::body_store::response_range;
use http_inspector_core::recording::{RangePlan, SaveBody, SaveFormat};
use http_inspector_exchange::{BodySide, ByteSize, Exchange};

use crate::ui::InspectorApp;
use crate::ui::dialogs::body_saving::PRIVACY_NOTICE;
use crate::ui::style::theme::Palette;

use super::background_task::{BackgroundTask, TaskPoll};

/// Gaps listed in the dialog before the rest are counted.
const SHOWN_GAPS: usize = 8;

/// The `Merge ranges…` request and the dialog that confirms it.
#[derive(Default)]
pub(super) struct MergeControls {
    requested: Option<Exchange>,
    /// A plan being made on a background thread.
    planning: Option<BackgroundTask<Option<RangePlan>>>,
    plan: Option<RangePlan>,
}

impl MergeControls {
    /// Reports whether the dialog is open or about to open.
    pub(super) const fn is_open(&self) -> bool {
        self.requested.is_some() || self.planning.is_some() || self.plan.is_some()
    }

    /// Adds the byte range and `Merge ranges…` to a response save menu.
    /// Returns whether the menu should close.
    pub(super) fn menu_items(&mut self, ui: &mut egui::Ui, record: &Exchange) -> bool {
        let Some(range) = response_range(record) else {
            return false;
        };
        ui.separator();
        ui.weak(range_text(range.start(), range.end(), range.total()));
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

fn range_text(start: u64, end: u64, total: Option<u64>) -> String {
    let total = total.map_or_else(|| "unknown".to_owned(), |total| total.to_string());
    format!("206 range: bytes {start}-{end} of {total}")
}

fn plan_text(plan: &RangePlan) -> String {
    let total = plan.total().map_or_else(
        || "unknown length".to_owned(),
        |bytes| ByteSize::new(bytes).to_string(),
    );
    let validator = plan.validator().map_or_else(
        || "no ETag or Last-Modified".to_owned(),
        |value| format!("validator {value}"),
    );
    format!(
        "{} responses cover {} of {total} · {validator}. The file will be {}.",
        plan.parts(),
        ByteSize::new(plan.covered_bytes()),
        ByteSize::new(plan.file_bytes())
    )
}

fn gap_lines(plan: &RangePlan) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .gaps_with_lengths()
        .take(SHOWN_GAPS)
        .map(|(first, last, length)| {
            format!("Missing bytes {first}-{last} ({})", ByteSize::new(length))
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
    let viewport = context.content_rect();
    let max_width = (viewport.width() - 48.0).clamp(240.0, 460.0);
    let max_scroll_height = (viewport.height() - 140.0).max(120.0);
    egui::Modal::new(egui::Id::unique("merge-ranges")).show(context, |ui| {
        ui.set_max_width(max_width);
        ui.heading("Merge ranges");
        egui::ScrollArea::vertical()
            .max_height(max_scroll_height)
            .show(ui, |ui| {
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
            });
        ui.horizontal_wrapped(|ui| {
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
    pub(crate) fn range_merge(&mut self, context: &egui::Context) {
        if let Some(record) = self.session.body_saving.merge.requested.take() {
            self.start_range_plan(&record);
        }
        self.poll_range_plan(context);
        let Some(plan) = self.session.body_saving.merge.plan.as_ref() else {
            return;
        };
        let Some(save) = dialog(context, plan) else {
            return;
        };
        let Some(plan) = self.session.body_saving.merge.plan.take() else {
            return;
        };
        if !save {
            return;
        }
        let started = self.dialogs.save.directory().and_then(|directory| {
            self.session
                .body_saving
                .store()
                .ok_or_else(|| std::io::Error::other("Saving bodies is unavailable"))?
                .save_ranges(plan, directory)
        });
        match started {
            Ok(receiver) => self
                .session
                .body_saving
                .track(BackgroundTask::track(receiver)),
            Err(error) => self.session.error = Some(format!("Merging ranges failed: {error}")),
        }
    }
}

impl InspectorApp {
    /// Starts planning a merge for `record` on a background thread.
    fn start_range_plan(&mut self, record: &Exchange) {
        let Some(store) = self.session.body_saving.store() else {
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
        let candidates: Vec<SaveBody> = self.requests.records.iter().filter_map(response).collect();
        match store.plan_ranges_in_background(target, candidates) {
            Ok(receiver) => {
                self.session.body_saving.merge.planning = Some(BackgroundTask::track(receiver));
            }
            Err(error) => self.session.error = Some(format!("Merging ranges failed: {error}")),
        }
    }

    /// Shows a waiting dialog until the plan arrives, then keeps it for the merge dialog.
    fn poll_range_plan(&mut self, context: &egui::Context) {
        let Some(task) = self.session.body_saving.merge.planning.as_ref() else {
            return;
        };
        let result = match task.poll() {
            TaskPoll::Ready(result) => result,
            TaskPoll::Pending => {
                let mut cancelled = false;
                egui::Modal::new(egui::Id::unique("plan-ranges")).show(context, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking the range responses…");
                    });
                    cancelled = ui.button("Cancel").clicked();
                });
                if cancelled {
                    // The worker finishes on its own; its result is dropped.
                    self.session.body_saving.merge.planning = None;
                } else {
                    context.request_repaint_after(Duration::from_millis(100));
                }
                return;
            }
            TaskPoll::Disconnected => Err(io::Error::other(
                "The range planning worker stopped unexpectedly",
            )),
        };
        self.session.body_saving.merge.planning = None;
        match result {
            Ok(plan) => self.session.body_saving.merge.plan = plan,
            Err(error) => self.session.error = Some(format!("Merging ranges failed: {error}")),
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
