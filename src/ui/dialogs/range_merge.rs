//! Showing the byte range of a `206` response and merging range responses.

use crate::ui::app::state::SessionError;
use crate::ui::dialogs::body_saving::privacy_notice;
use crate::ui::dialogs::save::localize_directory_error;
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Layout;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::recording::body_store::response_range;
use http_inspector_core::recording::{RangePlan, SaveBody, SaveFormat};
use http_inspector_exchange::{BodySide, ByteSize, Exchange};

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
    ui.separator();
    ui.weak(range_text(range.start(), range.end(), range.total(), lang));
    ui.button(lang.pick("Merge ranges…", "범위 응답 병합…"))
        .on_hover_text(lang.pick(
            "Join the 206 responses for this URL with the same ETag or Last-Modified into one file, in position order.",
            "같은 ETag 또는 Last-Modified를 가진 이 URL의 206 응답들을 바이트 위치 순서대로 하나의 파일로 합칩니다.",
        ))
        .clicked()
}

fn range_text(start: u64, end: u64, total: Option<u64>, lang: Language) -> String {
    match lang {
        Language::English => {
            let total = total.map_or_else(|| "unknown".to_owned(), |total| total.to_string());
            format!("206 range: bytes {start}-{end} of {total}")
        }
        Language::Korean => total.map_or_else(
            || format!("206 범위: {start}-{end} / 전체 알 수 없음"),
            |total| format!("206 범위: {start}-{end} / 전체 {total}바이트"),
        ),
    }
}

fn plan_text(plan: &RangePlan, lang: Language) -> String {
    let covered = ByteSize::new(plan.covered_bytes());
    let file_size = ByteSize::new(plan.file_bytes());
    let parts = plan.parts();
    match lang {
        Language::English => {
            let total = plan.total().map_or_else(
                || "unknown length".to_owned(),
                |bytes| ByteSize::new(bytes).to_string(),
            );
            let validator = plan.validator().map_or_else(
                || "no ETag or Last-Modified".to_owned(),
                |value| format!("validator {value}"),
            );
            format!(
                "{parts} responses cover {covered} of {total} · {validator}. The file will be {file_size}."
            )
        }
        Language::Korean => {
            let total = plan.total().map_or_else(
                || "전체 길이 알 수 없음".to_owned(),
                |bytes| ByteSize::new(bytes).to_string(),
            );
            let validator = plan.validator().map_or_else(
                || "ETag 또는 Last-Modified 없음".to_owned(),
                |value| format!("검증자 {value}"),
            );
            format!(
                "응답 {parts}건이 전체 {total} 중 {covered} 범위를 포함 · {validator}. 생성될 파일 크기는 {file_size}입니다."
            )
        }
    }
}

fn gap_lines(plan: &RangePlan, lang: Language) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .gaps_with_lengths()
        .take(SHOWN_GAPS)
        .map(|(first, last, length)| {
            let len = ByteSize::new(length);
            match lang {
                Language::English => format!("Missing bytes {first}-{last} ({len})"),
                Language::Korean => format!("누락된 바이트 구간 {first}-{last} ({len})"),
            }
        })
        .collect();
    if plan.gaps().len() > SHOWN_GAPS {
        let more = plan.gaps().len() - SHOWN_GAPS;
        lines.push(match lang {
            Language::English => format!("… and {more} more gaps"),
            Language::Korean => format!("… 외 누락 구간 {more}개"),
        });
    }
    lines
}

/// Shows the plan and returns `Some(true)` to save, `Some(false)` to cancel.
fn dialog(context: &egui::Context, plan: &RangePlan, lang: Language) -> Option<bool> {
    let mut choice = None;
    let viewport = context.content_rect();
    let max_width = Layout::BODY_EXPORT_WIDTH.fit(viewport.width());
    let max_scroll_height = Layout::BODY_EXPORT_HEIGHT.fit(viewport.height());
    let modal = egui::Modal::new(egui::Id::unique("merge-ranges")).show(context, |ui| {
        ui.set_max_width(max_width);
        ui.heading(lang.pick("Merge ranges", "범위 응답 병합"));
        egui::ScrollArea::vertical()
            .max_height(max_scroll_height)
            .show(ui, |ui| {
                ui.label(plan_text(plan, lang));
                let palette = Palette::of(ui);
                if plan.validator().is_none() {
                    ui.colored_label(
                        palette.warning,
                        lang.pick(
                            "Without ETag or Last-Modified, parts of different versions of this URL may be mixed.",
                            "ETag 또는 Last-Modified가 없으면 이 URL의 서로 다른 버전 조각이 섞일 수 있습니다.",
                        ),
                    );
                }
                for line in gap_lines(plan, lang) {
                    ui.colored_label(palette.warning, line);
                }
                if let Some(reason) = domain::range_plan_incomplete_reason(lang, plan) {
                    let note = match lang {
                        Language::English => {
                            format!("{reason} The file name will end in .partial.")
                        }
                        Language::Korean => {
                            format!("{reason} 파일 이름 끝에 .partial이 붙습니다.")
                        }
                    };
                    ui.colored_label(palette.warning, note);
                }
                ui.label(privacy_notice(lang));
            });
        ui.horizontal_wrapped(|ui| {
            let save = if plan.is_complete() {
                lang.text(Text::Save)
            } else {
                lang.pick("Save incomplete file", "불완전한 파일 저장")
            };
            if ui.button(save).clicked() {
                choice = Some(true);
            }
            if ui.button(lang.text(Text::Cancel)).clicked() {
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
                    ui.label(
                        lang.pick("Checking the range responses…", "범위 응답을 확인하는 중…"),
                    );
                });
                ui.button(lang.text(Text::Cancel)).clicked()
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
        let lang = self.language();
        let directory = match self.dialogs.save.directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.report_error(SessionError::RangeMerge(localize_directory_error(
                    &error, lang,
                )));
                return;
            }
        };
        let Some(store) = self.session.body_saving.store() else {
            self.report_error(SessionError::RangeMerge(
                lang.pick(
                    "Saving bodies is unavailable",
                    "본문 저장을 사용할 수 없습니다",
                )
                .to_owned(),
            ));
            return;
        };
        match store.save_ranges(plan, directory) {
            Ok(receiver) => self
                .session
                .body_saving
                .track(BackgroundTask::track(receiver)),
            Err(error) => self.report_error(SessionError::RangeMerge(error.to_string())),
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
            Err(error) => self.report_error(SessionError::RangeMerge(error.to_string())),
        }
    }

    /// Collects the merge plan when the worker has finished.
    pub(crate) fn poll_range_plan(&mut self, context: &egui::Context) {
        let Some(task) = self.session.body_saving.merge.planning.as_ref() else {
            return;
        };
        let lang = self.language();
        let result = match task.poll() {
            TaskPoll::Ready(result) => result,
            TaskPoll::Pending => {
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            TaskPoll::Disconnected => Err(io::Error::other(lang.pick(
                "The range planning worker stopped unexpectedly",
                "범위 병합 계획 작업자가 예기치 않게 중지되었습니다",
            ))),
        };
        self.session.body_saving.merge.planning = None;
        match result {
            Ok(plan) => self.session.body_saving.merge.plan = plan,
            Err(error) => self.report_error(SessionError::RangeMerge(error.to_string())),
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
