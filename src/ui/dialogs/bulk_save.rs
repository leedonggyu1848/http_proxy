//! The `Save bodies…` dialog and the progress of a running bulk save.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use http_inspector_core::recording::{
    BodyLayout, BodyStore, BulkSave, BulkSummary, SaveBody, SaveFormat,
};
use http_inspector_exchange::{BodySide, ByteSize, Exchange};

use crate::ui::InspectorApp;
use crate::ui::dialogs::body_saving::PRIVACY_NOTICE;
use crate::ui::style::theme::Palette;

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
pub(super) struct BulkChoice {
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

/// How many requests each scope covers, shown next to the choices.
#[derive(Clone, Copy, Debug)]
pub(super) struct ScopeCounts {
    pub(crate) all: usize,
    pub(crate) filtered: usize,
    pub(super) selected: usize,
}

/// Outcome of the most recent bulk save.
enum BulkResult {
    Finished(BulkSummary),
    Failed(String),
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

    /// Shows the dialog and returns the choice once the user confirms it.
    fn show(&mut self, context: &egui::Context, counts: ScopeCounts) -> Option<BulkChoice> {
        let mut choice = self.dialog?;
        let mut confirmed = None;
        let viewport = context.content_rect();
        let max_width = (viewport.width() - 48.0).clamp(240.0, 460.0);
        let max_scroll_height = (viewport.height() - 140.0).max(120.0);
        let modal = egui::Modal::new(egui::Id::unique("save-bodies")).show(context, |ui| {
            ui.set_max_width(max_width);
            ui.heading("Save bodies");
            egui::ScrollArea::vertical()
                .max_height(max_scroll_height)
                .show(ui, |ui| choice_options(ui, &mut choice, counts));
            confirmed = confirm_buttons(ui);
        });
        if modal.should_close() {
            confirmed = Some(false);
        }
        match confirmed {
            Some(true) => {
                self.dialog = None;
                self.last_choice = Some(choice);
                Some(choice)
            }
            Some(false) => {
                self.dialog = None;
                None
            }
            None => {
                self.dialog = Some(choice);
                None
            }
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
                self.result = Some(BulkResult::Failed(error.to_string()));
                self.running = None;
            }
            None => context.request_repaint_after(Duration::from_millis(200)),
        }
    }

    /// Shows progress with a Cancel button, or the last result, in the status bar.
    pub(super) fn status(&self, ui: &mut egui::Ui) {
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
                    .desired_width(crate::ui::style::tokens::Layout::BULK_FOLDER_INPUT_WIDTH)
                    .text(format!("{}/{}", progress.done(), progress.total())),
            );
            ui.label(format!(
                "Saving bodies · {}",
                ByteSize::new(progress.bytes())
            ));
            if progress.is_cancelled() {
                ui.weak("Cancelling…");
            } else if ui.small_button("Cancel").clicked() {
                running.cancel();
            }
            return;
        }
        match &self.result {
            Some(BulkResult::Finished(summary)) => {
                ui.separator();
                ui.label(summary_text(summary));
            }
            Some(BulkResult::Failed(error)) => {
                ui.separator();
                ui.colored_label(
                    Palette::of(ui).error,
                    format!("Saving bodies failed: {error}"),
                );
            }
            None => {}
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
            Err(error) => self.result = Some(BulkResult::Failed(error.to_string())),
        }
    }
}

fn choice_options(ui: &mut egui::Ui, choice: &mut BulkChoice, counts: ScopeCounts) {
    ui.label("Requests");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut choice.scope,
            BulkScope::Filtered,
            format!("Filtered list ({})", counts.filtered),
        );
        ui.radio_value(
            &mut choice.scope,
            BulkScope::All,
            format!("All ({})", counts.all),
        );
        ui.add_enabled_ui(counts.selected > 0, |ui| {
            ui.radio_value(&mut choice.scope, BulkScope::Selected, "Selected request");
        });
    });
    ui.label("Bodies");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut choice.sides, BulkSides::Responses, "Responses");
        ui.radio_value(&mut choice.sides, BulkSides::Requests, "Requests");
        ui.radio_value(&mut choice.sides, BulkSides::Both, "Both");
    });
    ui.label("Format");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut choice.format, SaveFormat::Decoded, "Decoded")
            .on_hover_text("Remove gzip, deflate, br and zstd Content-Encoding.");
        ui.radio_value(&mut choice.format, SaveFormat::AsReceived, "As received")
            .on_hover_text("Keep the exact bytes that were received.");
    });
    ui.label("Folders");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut choice.layout, BodyLayout::Flat, "One folder");
        ui.radio_value(
            &mut choice.layout,
            BodyLayout::HostPath,
            "host/path folders",
        );
    });
    ui.weak("Files go into a new folder inside the Save directory, with a manifest.json that lists each URL, status, size and SHA-256. Empty bodies are skipped.");
    ui.label(PRIVACY_NOTICE);
}

fn confirm_buttons(ui: &mut egui::Ui) -> Option<bool> {
    let mut confirmed = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Save").clicked() {
            confirmed = Some(true);
        }
        if ui.button("Cancel").clicked() {
            confirmed = Some(false);
        }
    });
    confirmed
}

fn summary_text(summary: &BulkSummary) -> String {
    let mut parts = vec![format!(
        "Saved {} bodies to {}",
        summary.saved(),
        summary.directory().display()
    )];
    if summary.incomplete() > 0 {
        parts.push(format!("{} incomplete", summary.incomplete()));
    }
    if summary.failed() > 0 {
        parts.push(format!("{} failed (see manifest.json)", summary.failed()));
    }
    if summary.cancelled() {
        parts.push("cancelled".to_owned());
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
    /// Shows the `Save bodies…` dialog and starts the save it confirms.
    pub(crate) fn bulk_body_save(&mut self, context: &egui::Context) {
        self.session.body_saving.bulk.poll(context);
        let counts = ScopeCounts {
            all: self.requests.records.len(),
            filtered: self.requests.filtered_records().len(),
            selected: usize::from(self.requests.selected.is_some()),
        };
        let Some(choice) = self.session.body_saving.bulk.show(context, counts) else {
            return;
        };
        let directory = match self.dialogs.save.directory() {
            Ok(directory) => directory,
            Err(error) => {
                self.session.body_saving.bulk.result = Some(BulkResult::Failed(error.to_string()));
                return;
            }
        };
        let Some(store) = self.session.body_saving.store().cloned() else {
            self.session.body_saving.bulk.result = Some(BulkResult::Failed(
                "Saving bodies is unavailable".to_owned(),
            ));
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
