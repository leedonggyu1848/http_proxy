//! The `Save bodies…` dialog and the progress of a running bulk save.

use crate::ui::i18n::{Language, Text};
use crate::ui::style::tokens::Layout;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use http_inspector_core::recording::{
    BodyLayout, BodyStore, BulkSave, BulkSummary, SaveBody, SaveFormat,
};
use http_inspector_exchange::{BodySide, ByteSize, Exchange};

use crate::ui::InspectorApp;
use crate::ui::dialogs::body_saving::privacy_notice;
use crate::ui::dialogs::save::localize_directory_error;

/// Which requests a bulk save covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BulkScope {
    All,
    Filtered,
    Selected,
}

/// Which bodies of each request are saved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BulkSides {
    Responses,
    Requests,
    Both,
}

impl BulkSides {
    const fn sides(self) -> &'static [BodySide] {
        match self {
            Self::Responses => &[BodySide::Response],
            Self::Requests => &[BodySide::Request],
            Self::Both => &[BodySide::Request, BodySide::Response],
        }
    }
}

/// Choices made in the dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BulkChoice {
    pub(super) scope: BulkScope,
    pub(super) sides: BulkSides,
    pub(super) format: SaveFormat,
    pub(super) layout: BodyLayout,
}

impl Default for BulkChoice {
    fn default() -> Self {
        Self {
            scope: BulkScope::Filtered,
            sides: BulkSides::Responses,
            format: SaveFormat::Decoded,
            layout: BodyLayout::Flat,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BulkAction {
    Close,
    Save(BulkChoice),
}

/// How many requests each scope covers, shown next to the choices.
#[derive(Clone, Copy, Debug)]
pub(super) struct ScopeCounts {
    pub(crate) all: usize,
    pub(crate) filtered: usize,
    pub(super) selected: usize,
}

enum BulkFailure {
    Directory(io::Error),
    Unavailable,
    Io(String),
}

impl BulkFailure {
    fn localized(&self, lang: Language) -> String {
        match self {
            Self::Directory(err) => localize_directory_error(err, lang),
            Self::Unavailable => lang
                .pick(
                    "Saving bodies is unavailable",
                    "본문 저장을 사용할 수 없습니다",
                )
                .to_owned(),
            Self::Io(detail) => detail.clone(),
        }
    }
}

/// Outcome of the most recent bulk save.
enum BulkResult {
    Finished(BulkSummary),
    Failed(BulkFailure),
}

/// The bulk save dialog, the running save and its last result.
#[derive(Default)]
pub(super) struct BulkControls {
    dialog: Option<BulkChoice>,
    last_choice: Option<BulkChoice>,
    running: Option<BulkSave>,
    result: Option<BulkResult>,
}

impl BulkControls {
    /// Opens the dialog unless a bulk save is already running.
    pub(super) fn open(&mut self) {
        if self.running.is_none() {
            self.dialog = Some(self.last_choice.unwrap_or_default());
        }
    }

    pub(super) const fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Shows the form and reports what the user chose.
    fn show(
        &mut self,
        context: &egui::Context,
        counts: ScopeCounts,
        lang: Language,
    ) -> Option<BulkAction> {
        let choice = self.dialog.as_mut()?;
        let mut confirmed = None;
        let viewport = context.content_rect();
        let max_width = Layout::BODY_EXPORT_WIDTH.fit(viewport.width());
        let max_scroll_height = Layout::BODY_EXPORT_HEIGHT.fit(viewport.height());
        let modal = egui::Modal::new(egui::Id::unique("save-bodies")).show(context, |ui| {
            ui.set_max_width(max_width);
            ui.heading(lang.pick("Save bodies", "본문 일괄 저장"));
            egui::ScrollArea::vertical()
                .max_height(max_scroll_height)
                .show(ui, |ui| choice_options(ui, choice, counts, lang));
            confirmed = confirm_buttons(ui, lang);
        });
        if modal.should_close() {
            confirmed = Some(false);
        }
        confirmed.map(|confirmed| {
            if confirmed {
                BulkAction::Save(*choice)
            } else {
                BulkAction::Close
            }
        })
    }

    const fn finish(&mut self, action: BulkAction) -> Option<BulkChoice> {
        self.dialog = None;
        if let BulkAction::Save(choice) = action {
            self.last_choice = Some(choice);
            Some(choice)
        } else {
            None
        }
    }

    /// Collects a finished save; call once per frame.
    pub(super) fn poll(&mut self, context: &egui::Context) {
        let Some(running) = self.running.as_ref() else {
            return;
        };
        match running.try_finish() {
            Some(Ok(summary)) => {
                self.result = Some(BulkResult::Finished(summary));
                self.running = None;
            }
            Some(Err(error)) => {
                self.result = Some(BulkResult::Failed(BulkFailure::Io(error.to_string())));
                self.running = None;
            }
            None => context.request_repaint_after(Duration::from_millis(200)),
        }
    }

    /// Shows progress with a Cancel button, or the last result, in the status bar.
    pub(super) fn status(&self, ui: &mut egui::Ui, lang: Language) -> bool {
        if let Some(running) = self.running.as_ref() {
            let progress = running.progress();
            ui.separator();
            #[expect(
                clippy::cast_precision_loss,
                reason = "The fraction only sizes a progress bar"
            )]
            let fraction = progress.done() as f32 / progress.total().max(1) as f32;
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(Layout::BULK_PROGRESS_WIDTH)
                    .text(format!("{}/{}", progress.done(), progress.total())),
            );
            let bytes = ByteSize::new(progress.bytes());
            let saving_label = match lang {
                Language::English => format!("Saving bodies · {bytes}"),
                Language::Korean => format!("본문 저장 중 · {bytes}"),
            };
            ui.label(saving_label);
            if progress.is_cancelled() {
                ui.weak(lang.pick("Cancelling…", "취소 중…"));
                return false;
            }
            return ui.small_button(lang.text(Text::Cancel)).clicked();
        }
        match &self.result {
            Some(BulkResult::Finished(summary)) => {
                ui.separator();
                ui.label(summary_text(summary, lang));
            }
            Some(BulkResult::Failed(error)) => {
                ui.separator();
                let detail = error.localized(lang);
                let text = match lang {
                    Language::English => format!("Saving bodies failed: {detail}"),
                    Language::Korean => format!("본문 일괄 저장 실패: {detail}"),
                };
                crate::ui::style::indicator::dot_label(
                    ui,
                    crate::ui::style::tokens::Tone::Danger,
                    text,
                );
            }
            None => {}
        }
        false
    }

    pub(super) fn cancel(&self) {
        if let Some(running) = self.running.as_ref() {
            running.cancel();
        }
    }

    fn start(
        &mut self,
        store: &BodyStore,
        bodies: Vec<SaveBody>,
        layout: BodyLayout,
        folder: PathBuf,
    ) {
        self.result = None;
        match store.save_all(bodies, layout, folder) {
            Ok(running) => self.running = Some(running),
            Err(error) => {
                self.result = Some(BulkResult::Failed(BulkFailure::Io(error.to_string())));
            }
        }
    }
}

fn choice_options(ui: &mut egui::Ui, choice: &mut BulkChoice, counts: ScopeCounts, lang: Language) {
    ui.label(lang.pick("Requests", "요청 범위"));
    ui.horizontal_wrapped(|ui| {
        let filtered_label = match lang {
            Language::English => format!("Filtered list ({})", counts.filtered),
            Language::Korean => format!("필터된 목록 ({}건)", counts.filtered),
        };
        let all_label = match lang {
            Language::English => format!("All ({})", counts.all),
            Language::Korean => format!("전체 ({}건)", counts.all),
        };
        ui.radio_value(&mut choice.scope, BulkScope::Filtered, filtered_label);
        ui.radio_value(&mut choice.scope, BulkScope::All, all_label);
        ui.add_enabled_ui(counts.selected > 0, |ui| {
            ui.radio_value(
                &mut choice.scope,
                BulkScope::Selected,
                lang.pick("Selected request", "선택한 요청"),
            );
        });
    });
    ui.label(lang.pick("Bodies", "대상 본문"));
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut choice.sides,
            BulkSides::Responses,
            lang.pick("Responses", "응답"),
        );
        ui.radio_value(
            &mut choice.sides,
            BulkSides::Requests,
            lang.pick("Requests", "요청"),
        );
        ui.radio_value(
            &mut choice.sides,
            BulkSides::Both,
            lang.pick("Both", "모두"),
        );
    });
    ui.label(lang.pick("Format", "저장 형식"));
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut choice.format,
            SaveFormat::Decoded,
            lang.pick("Decoded", "디코딩됨"),
        )
        .on_hover_text(lang.pick(
            "Remove gzip, deflate, br and zstd Content-Encoding.",
            "gzip, deflate, br, zstd Content-Encoding을 해제해 저장합니다.",
        ));
        ui.radio_value(
            &mut choice.format,
            SaveFormat::AsReceived,
            lang.pick("As received", "수신 원본 그대로"),
        )
        .on_hover_text(lang.pick(
            "Keep the exact bytes that were received.",
            "수신한 바이트 그대로 저장합니다.",
        ));
    });
    ui.label(lang.pick("Folders", "폴더 구조"));
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut choice.layout,
            BodyLayout::Flat,
            lang.pick("One folder", "단일 폴더"),
        );
        ui.radio_value(
            &mut choice.layout,
            BodyLayout::HostPath,
            lang.pick("host/path folders", "host/path 폴더 구조"),
        );
    });
    ui.weak(lang.pick(
        "Files go into a new folder inside the Save directory, with a manifest.json that lists each URL, status, size and SHA-256. Empty bodies are skipped.",
        "저장 폴더 안의 새 폴더에 파일이 저장되며, 각 URL·상태·크기·SHA-256을 기록한 manifest.json이 함께 생성됩니다. 빈 본문은 건너뜁니다.",
    ));
    ui.label(privacy_notice(lang));
}

fn confirm_buttons(ui: &mut egui::Ui, lang: Language) -> Option<bool> {
    let mut confirmed = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button(lang.text(Text::Save)).clicked() {
            confirmed = Some(true);
        }
        if ui.button(lang.text(Text::Cancel)).clicked() {
            confirmed = Some(false);
        }
    });
    confirmed
}

fn summary_text(summary: &BulkSummary, lang: Language) -> String {
    let mut parts = vec![match lang {
        Language::English => format!(
            "Saved {} bodies to {}",
            summary.saved(),
            summary.directory().display()
        ),
        Language::Korean => format!(
            "본문 {}건을 {}에 저장했습니다",
            summary.saved(),
            summary.directory().display()
        ),
    }];
    if summary.incomplete() > 0 {
        parts.push(match lang {
            Language::English => format!("{} incomplete", summary.incomplete()),
            Language::Korean => format!("불완전 {}건", summary.incomplete()),
        });
    }
    if summary.failed() > 0 {
        parts.push(match lang {
            Language::English => format!("{} failed (see manifest.json)", summary.failed()),
            Language::Korean => format!("실패 {}건 (manifest.json 확인)", summary.failed()),
        });
    }
    if summary.cancelled() {
        parts.push(lang.pick("cancelled", "취소됨").to_owned());
    }
    parts.join(" · ")
}

/// Builds the bodies to save, skipping empty complete bodies and responses
/// that never arrived. Bodies the store never saw are kept so the manifest
/// can report them.
pub(super) fn bodies_to_save(
    records: &[&Exchange],
    choice: BulkChoice,
    store: &BodyStore,
    folder: &Path,
) -> Vec<SaveBody> {
    records
        .iter()
        .flat_map(|record| {
            choice
                .sides
                .sides()
                .iter()
                .map(move |&side| (*record, side))
        })
        .filter(|(record, side)| {
            store
                .body(record.id(), *side)
                .is_none_or(|stored| stored.stored_bytes() > 0 || !stored.is_complete())
        })
        .filter_map(|(record, side)| {
            SaveBody::new(record, side, choice.format, folder.to_path_buf())
        })
        .collect()
}

/// Returns a new folder name inside `directory` for one bulk save.
pub(super) fn new_folder(directory: &Path) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let base = format!("http-inspector-bodies-{seconds}");
    (0..10_000)
        .map(|attempt| {
            if attempt == 0 {
                directory.join(&base)
            } else {
                directory.join(format!("{base}-{attempt}"))
            }
        })
        .find(|path| !path.exists())
        .unwrap_or_else(|| directory.join(base))
}

impl InspectorApp {
    /// Shows the `Save bodies…` dialog and reports the user action.
    pub(crate) fn show_bulk_save(&mut self, context: &egui::Context) -> Option<BulkAction> {
        let counts = ScopeCounts {
            all: self.requests.records.len(),
            filtered: self.requests.filtered_records().len(),
            selected: usize::from(self.requests.selected.is_some()),
        };
        let lang = self.language();
        self.session.body_saving.bulk.show(context, counts, lang)
    }

    pub(crate) fn apply_bulk_action(&mut self, action: BulkAction) {
        if let Some(choice) = self.session.body_saving.bulk.finish(action) {
            self.save_bulk(choice);
        }
    }

    fn save_bulk(&mut self, choice: BulkChoice) {
        let directory = match self.dialogs.save.directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.session.body_saving.bulk.result =
                    Some(BulkResult::Failed(BulkFailure::Directory(error)));
                return;
            }
        };
        let Some(store) = self.session.body_saving.store().cloned() else {
            self.session.body_saving.bulk.result =
                Some(BulkResult::Failed(BulkFailure::Unavailable));
            return;
        };
        let records: Vec<&Exchange> = match choice.scope {
            BulkScope::All => self.requests.records.iter().collect(),
            BulkScope::Filtered => self.requests.filtered_records(),
            BulkScope::Selected => self
                .requests
                .records
                .iter()
                .filter(|record| Some(record.id()) == self.requests.selected)
                .collect(),
        };
        let folder = new_folder(&directory);
        let bodies = bodies_to_save(&records, choice, &store, &folder);
        self.session
            .body_saving
            .bulk
            .start(&store, bodies, choice.layout, folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_remembers_the_confirmed_choice_after_a_cancellation() {
        let choice = BulkChoice {
            sides: BulkSides::Both,
            ..BulkChoice::default()
        };
        let mut controls = BulkControls::default();
        controls.open();
        assert_eq!(controls.finish(BulkAction::Save(choice)), Some(choice));
        assert!(!controls.is_open());

        controls.open();
        assert_eq!(controls.dialog, Some(choice));
        controls.dialog = Some(BulkChoice::default());
        assert_eq!(controls.finish(BulkAction::Close), None);
        assert!(!controls.is_open());

        controls.open();
        assert_eq!(controls.dialog, Some(choice));
    }

    #[test]
    fn new_folders_never_reuse_an_existing_name() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let first = new_folder(directory.path());
        std::fs::create_dir(&first)?;
        let second = new_folder(directory.path());
        assert_ne!(first, second);
        assert!(second.starts_with(directory.path()));
        Ok(())
    }

    #[test]
    fn sides_cover_the_chosen_bodies() {
        assert_eq!(BulkSides::Responses.sides(), [BodySide::Response]);
        assert_eq!(BulkSides::Requests.sides(), [BodySide::Request]);
        assert_eq!(
            BulkSides::Both.sides(),
            [BodySide::Request, BodySide::Response]
        );
    }
}
