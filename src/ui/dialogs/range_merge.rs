//! Showing the byte range of a `206` response and merging range responses.

use crate::ui::app::state::{RangeMergeError, SessionError};
use crate::ui::dialogs::body_saving::privacy_notice;
use crate::ui::i18n::{self, Language, domain};
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Layout;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::recording::body_store::response_range;
use http_inspector_core::recording::{RangePlan, SaveBody, SaveFormat};
use http_inspector_exchange::{BodySide, Exchange};

use crate::ui::InspectorApp;

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
pub(super) fn menu_items(ui: &mut egui::Ui, record: &Exchange, lang: Language) -> bool {
    let Some(range) = response_range(record) else {
        return false;
    };
    let text = lang.range_merge();
    ui.separator();
    ui.weak(range_text(range.start(), range.end(), range.total(), lang));
    ui.button(text.menu_item)
        .on_hover_text(text.tooltips.menu_item_hint)
        .clicked()
}

fn range_text(start: u64, end: u64, total: Option<u64>, lang: Language) -> String {
    i18n::dialogs::range_merge::range_text(lang, start, end, total)
}

fn gap_lines(plan: &RangePlan, lang: Language) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .gaps_with_lengths()
        .take(SHOWN_GAPS)
        .map(|(first, last, length)| {
            i18n::dialogs::range_merge::missing_gap(lang, first, last, length)
        })
        .collect();
    if plan.gaps().len() > SHOWN_GAPS {
        let more = plan.gaps().len() - SHOWN_GAPS;
        lines.push(i18n::dialogs::range_merge::more_gaps(lang, more));
    }
    lines
}

/// Shows the plan and returns `Some(true)` to save, `Some(false)` to cancel.
fn dialog(context: &egui::Context, plan: &RangePlan, lang: Language) -> Option<bool> {
    let text = lang.range_merge();
    let shared = lang.shared();
    let mut choice = None;
    let viewport = context.content_rect();
    let max_width = Layout::BODY_EXPORT_WIDTH.fit(viewport.width());
    let max_scroll_height = Layout::BODY_EXPORT_HEIGHT.fit(viewport.height());
    let modal = egui::Modal::new(egui::Id::unique("merge-ranges")).show(context, |ui| {
        ui.set_max_width(max_width);
        ui.heading(text.heading);
        egui::ScrollArea::vertical()
            .max_height(max_scroll_height)
            .show(ui, |ui| {
                ui.label(i18n::dialogs::range_merge::plan_text(lang, plan));
                let palette = Palette::of(ui);
                if plan.validator().is_none() {
                    ui.colored_label(palette.warning, text.missing_validator_warning);
                }
                for line in gap_lines(plan, lang) {
                    ui.colored_label(palette.warning, line);
                }
                if let Some(reason) = domain::range_plan_incomplete_reason(lang, plan) {
                    let note = i18n::dialogs::range_merge::partial_file_note(lang, &reason);
                    ui.colored_label(palette.warning, note);
                }
                ui.label(privacy_notice(lang));
            });
        ui.horizontal_wrapped(|ui| {
            let save = if plan.is_complete() {
                shared.save
            } else {
                text.save_incomplete
            };
            if ui.button(save).clicked() {
                choice = Some(true);
            }
            if ui.button(shared.cancel).clicked() {
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
        let lang = self.language();
        let merge = &self.session.body_saving.merge;
        if merge.planning.is_some() {
            let modal = egui::Modal::new(egui::Id::unique("plan-ranges")).show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(lang.range_merge().checking_ranges);
                });
                ui.button(lang.shared().cancel).clicked()
            });
            return (modal.should_close() || modal.inner).then_some(RangeAction::CancelPlanning);
        }
        merge
            .plan
            .as_ref()
            .and_then(|plan| dialog(context, plan, lang))
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
        let directory = match self.dialogs.save.directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.report_error(SessionError::RangeMerge(RangeMergeError::Directory(error)));
                return;
            }
        };
        let Some(store) = self.session.body_saving.store() else {
            self.report_error(SessionError::RangeMerge(RangeMergeError::SavingUnavailable));
            return;
        };
        match store.save_ranges(plan, directory) {
            Ok(receiver) => self
                .session
                .body_saving
                .track(BackgroundTask::track(receiver)),
            Err(error) => self.report_error(SessionError::RangeMerge(RangeMergeError::Io(error))),
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
            Err(error) => self.report_error(SessionError::RangeMerge(RangeMergeError::Io(error))),
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
            TaskPoll::Disconnected => {
                self.session.body_saving.merge.planning = None;
                self.report_error(SessionError::RangeMerge(RangeMergeError::WorkerStopped));
                return;
            }
        };
        self.session.body_saving.merge.planning = None;
        match result {
            Ok(plan) => self.session.body_saving.merge.plan = plan,
            Err(error) => self.report_error(SessionError::RangeMerge(RangeMergeError::Io(error))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_ranges() {
        assert_eq!(
            range_text(0, 99, Some(1000), Language::English),
            "206 range: bytes 0-99 of 1000"
        );
        assert_eq!(
            range_text(5, 9, None, Language::English),
            "206 range: bytes 5-9 of unknown"
        );
        assert_eq!(
            range_text(0, 99, Some(1000), Language::Korean),
            "206 범위: 0-99 / 전체 1000바이트"
        );
        assert_eq!(
            range_text(5, 9, None, Language::Korean),
            "206 범위: 5-9 / 전체 알 수 없음"
        );
    }
}
