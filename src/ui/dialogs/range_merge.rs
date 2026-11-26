//! Showing the byte range of a `206` response and merging range responses.

use crate::ui::style::tokens::Layout;
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

#[derive(Clone, Copy)]
pub(crate) enum RangeAction {
    CancelPlanning,
    Confirm(bool),
}

/// The `Merge ranges…` request and the dialog that confirms it.
#[derive(Default)]
pub(super) struct MergeControls {
    /// A plan being made on a background thread.
    planning: Option<BackgroundTask<Option<RangePlan>>>,
    plan: Option<RangePlan>,
}

impl MergeControls {
    /// Reports whether the dialog is open or about to open.
    pub(super) const fn is_open(&self) -> bool {
        self.planning.is_some() || self.plan.is_some()
    }
}

/// Adds the byte range and `Merge ranges…` to a response save menu.
/// Returns whether the menu should close.
pub(super) fn menu_items(ui: &mut egui::Ui, record: &Exchange) -> bool {
    let Some(range) = response_range(record) else {
        return false;
    };
    ui.separator();
    ui.weak(range_text(range.start(), range.end(), range.total()));
    ui
        .button("Merge ranges…")
        .on_hover_text(
            "Join the 206 responses for this URL with the same ETag or Last-Modified into one file, in position order.",
        )
        .clicked()
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
    let max_width = Layout::BODY_EXPORT_WIDTH.fit(viewport.width());
    let max_scroll_height = Layout::BODY_EXPORT_HEIGHT.fit(viewport.height());
    let modal = egui::Modal::new(egui::Id::unique("merge-ranges")).show(context, |ui| {
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
    if modal.should_close() {
        return Some(false);
    }
    choice
}

impl InspectorApp {
    pub(crate) fn show_range_merge(&self, context: &egui::Context) -> Option<RangeAction> {
        let merge = &self.session.body_saving.merge;
        if merge.planning.is_some() {
            let modal = egui::Modal::new(egui::Id::unique("plan-ranges")).show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the range responses…");
                });
                ui.button("Cancel").clicked()
            });
            return (modal.should_close() || modal.inner).then_some(RangeAction::CancelPlanning);
        }
        merge
            .plan
            .as_ref()
            .and_then(|plan| dialog(context, plan))
            .map(RangeAction::Confirm)
    }

    pub(crate) fn apply_range_action(&mut self, action: RangeAction) {
        match action {
            RangeAction::CancelPlanning => self.session.body_saving.merge.planning = None,
            RangeAction::Confirm(save) => self.finish_range_merge(save),
        }
    }

    fn finish_range_merge(&mut self, save: bool) {
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
            Err(error) => self.report_error(format!("Merging ranges failed: {error}")),
        }
    }
}

impl InspectorApp {
    /// Starts planning a merge for `record` on a background thread.
    pub(crate) fn start_range_plan(&mut self, record: &Exchange) {
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
            Err(error) => self.report_error(format!("Merging ranges failed: {error}")),
        }
    }

    /// Collects the merge plan when the worker has finished.
    pub(crate) fn poll_range_plan(&mut self, context: &egui::Context) {
        let Some(task) = self.session.body_saving.merge.planning.as_ref() else {
            return;
        };
        let result = match task.poll() {
            TaskPoll::Ready(result) => result,
            TaskPoll::Pending => {
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            TaskPoll::Disconnected => Err(io::Error::other(
                "The range planning worker stopped unexpectedly",
            )),
        };
        self.session.body_saving.merge.planning = None;
        match result {
            Ok(plan) => self.session.body_saving.merge.plan = plan,
            Err(error) => self.report_error(format!("Merging ranges failed: {error}")),
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
