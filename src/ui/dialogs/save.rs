use crate::ui::style::tokens::Layout;
use crate::ui::workbench::controls;
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::ShellText;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, ColorImage};
use http_inspector_core::recording::FullLog;
use http_inspector_exchange::Exchange;
use http_inspector_file::{ExistingFile, publish};
use http_inspector_interop::har::{self, HarOptions};

use super::background_task::{BackgroundTask, TaskPoll};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

type SaveCompletion = io::Result<PathBuf>;

/// A `Save` menu item that opens a dialog owned by the app.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SaveMenuAction {
    Directory(String),
    SavePng,
    SaveLogs,
    ExportHar,
    SaveBodies,
}

pub(crate) struct SaveControls {
    directory: String,
    pending: Option<BackgroundTask<PathBuf>>,
    status: Option<SaveCompletion>,
}

impl SaveControls {
    pub(crate) fn new() -> Self {
        Self {
            directory: std::env::current_dir()
                .map_or_else(|_| String::new(), |path| path.display().to_string()),
            pending: None,
            status: None,
        }
    }

    /// Shows the Save menu on the toolbar, with a spinner beside it while a
    /// save runs. Laid out right to left, so the menu comes first.
    pub(crate) fn menu(&self, ui: &mut egui::Ui, log_available: bool) -> Vec<SaveMenuAction> {
        let actions = ui
            .add_enabled_ui(self.pending.is_none(), |ui| {
                let label = format!("{} ▼", ShellText::Save.text());
                controls::ghost_menu(ui, label, Layout::TOOLBAR_BUTTON_HEIGHT, |ui| {
                    self.menu_items(ui, log_available)
                })
                .inner
                .unwrap_or_default()
            })
            .inner;
        if self.pending.is_some() {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        actions
    }

    pub(crate) fn set_directory(&mut self, directory: String) {
        self.directory = directory;
    }

    pub(crate) fn save_logs(&mut self, log: &FullLog) {
        self.status = None;
        match self.destination("log").and_then(|path| log.export(path)) {
            Ok(receiver) => self.pending = Some(BackgroundTask::track(receiver)),
            Err(error) => self.status = Some(Err(error)),
        }
    }

    /// Reports whether a save is still running.
    pub(crate) const fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Suggests a new file in the save directory, without checking it.
    pub(crate) fn suggested_path(&self, extension: &str) -> String {
        self.destination(extension)
            .map_or_else(|_| String::new(), |path| path.display().to_string())
    }

    /// Writes `records` as HAR to `path` on a worker thread.
    pub(crate) fn export_har(
        &mut self,
        context: &egui::Context,
        records: Vec<Exchange>,
        options: HarOptions,
        path: PathBuf,
    ) {
        self.status = None;
        self.pending = Some(BackgroundTask::start("export-har", context, move || {
            let document = har::document(&records, options, SystemTime::now());
            drop(records);
            har::write(&path, &document)
        }));
    }

    /// Writes `content` to a new, non-clobbering file inside the configured
    /// `Save directory` on a worker thread.
    pub(crate) fn save_text_file(
        &mut self,
        context: &egui::Context,
        prefix: &str,
        extension: &str,
        text: String,
    ) {
        self.status = None;
        let path = match self.destination_with_prefix(prefix, extension) {
            Ok(path) => path,
            Err(error) => {
                self.status = Some(Err(error));
                return;
            }
        };
        self.pending = Some(BackgroundTask::start("save-url-map", context, move || {
            save_text_atomic(&path, text.as_bytes()).map(|()| path)
        }));
    }

    fn menu_items(&self, ui: &mut egui::Ui, log_available: bool) -> Vec<SaveMenuAction> {
        let mut actions = Vec::new();
        ui.label("Save directory");
        let input_width = Layout::SAVE_DIRECTORY_WIDTH.fit(ui.ctx().content_rect().width());
        let mut directory = self.directory.clone();
        let field = Field::new(input_width).hint("Existing directory");
        if field::single_line(ui, &mut directory, field, |_| {}).changed() {
            actions.push(SaveMenuAction::Directory(directory));
        }
        if ui
            .button("Save window PNG")
            .on_hover_text("Save the visible app content, without the OS window border.")
            .clicked()
        {
            actions.push(SaveMenuAction::SavePng);
            ui.close();
        }
        if ui
            .add_enabled(log_available, egui::Button::new("Save all logs"))
            .on_hover_text("Save all captured records and full bodies, regardless of the filter.")
            .clicked()
        {
            actions.push(SaveMenuAction::SaveLogs);
            ui.close();
        }
        if ui
            .button("Export HAR…")
            .on_hover_text(
                "Save the listed requests as a HAR 1.2 file, without bodies or sensitive headers unless you include them.",
            )
            .clicked()
        {
            actions.push(SaveMenuAction::ExportHar);
            ui.close();
        }
        if ui
            .button("Save bodies…")
            .on_hover_text(
                "Save the request and response bodies of many requests into a new folder with a manifest.json.",
            )
            .clicked()
        {
            actions.push(SaveMenuAction::SaveBodies);
            ui.close();
        }
        actions
    }

    /// Shows the result of the last save in the status bar.
    pub(crate) fn status(&self, ui: &mut egui::Ui) {
        match &self.status {
            Some(Ok(path)) => {
                ui.label(format!("Saved: {}", path.display()));
            }
            Some(Err(error)) => {
                crate::ui::style::indicator::dot_label(
                    ui,
                    crate::ui::style::tokens::Tone::Danger,
                    format!("Save failed: {error}"),
                );
            }
            None => {}
        }
    }

    pub(crate) fn poll(&mut self) {
        let Some(task) = &self.pending else {
            return;
        };
        let result = match task.poll() {
            TaskPoll::Ready(result) => result,
            TaskPoll::Pending => return,
            TaskPoll::Disconnected => Err(io::Error::other("The save worker stopped unexpectedly")),
        };
        self.pending = None;
        self.status = Some(result);
    }

    /// Returns the chosen save directory when it exists.
    pub(crate) fn directory(&self) -> io::Result<PathBuf> {
        let directory = Path::new(&self.directory);
        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Choose an existing save directory in the Save menu",
            ));
        }
        Ok(directory.to_path_buf())
    }

    fn destination(&self, extension: &str) -> io::Result<PathBuf> {
        self.destination_with_prefix("http-inspector", extension)
    }

    fn destination_with_prefix(&self, prefix: &str, extension: &str) -> io::Result<PathBuf> {
        let directory = self.directory()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        Ok(directory.join(format!(
            "{prefix}-{}-{:09}-{}-{sequence}.{extension}",
            timestamp.as_secs(),
            timestamp.subsec_nanos(),
            std::process::id(),
        )))
    }

    pub(crate) fn start_screenshot(&mut self, context: &egui::Context) {
        self.status = None;
        let path = match self.destination("png") {
            Ok(path) => path,
            Err(error) => {
                self.status = Some(Err(error));
                return;
            }
        };
        let (task, starter) = BackgroundTask::deferred();
        self.pending = Some(task);
        let context = context.clone();
        context.clone().request_screenshot(move |image| {
            starter.start("save-window-png", &context, move || {
                save_png(&path, &image).map(|()| path)
            });
        });
    }
}

fn save_png(path: &Path, image: &ColorImage) -> io::Result<()> {
    let width = u32::try_from(image.size[0]).map_err(io::Error::other)?;
    let height = u32::try_from(image.size[1]).map_err(io::Error::other)?;
    let pixels: Vec<u8> = image.pixels.iter().flat_map(Color32::to_array).collect();
    publish(path, ExistingFile::Preserve, |file| {
        let mut encoder = png::Encoder::new(&mut *file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        writer.write_image_data(&pixels).map_err(io::Error::other)?;
        writer.finish().map_err(io::Error::other)?;
        file.flush()
    })
}

fn save_text_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    publish(path, ExistingFile::Preserve, |file| {
        file.write_all(bytes)?;
        file.flush()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_preserves_pixels_and_existing_files() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("window.png");
        let image = ColorImage::new([2, 1], vec![Color32::RED, Color32::BLUE]);
        save_png(&path, &image)?;
        let original = std::fs::read(&path)?;
        let decoder = png::Decoder::new(io::Cursor::new(&original));
        let mut reader = decoder.read_info().map_err(io::Error::other)?;
        let mut pixels = vec![0; 8];
        let info = reader.next_frame(&mut pixels).map_err(io::Error::other)?;
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(pixels, [255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(save_png(&path, &image).is_err());
        assert_eq!(std::fs::read(&path)?, original);
        Ok(())
    }

    #[test]
    fn text_save_preserves_content_and_existing_files() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("url-map.txt");
        save_text_atomic(&path, b"first version\n")?;
        assert_eq!(std::fs::read_to_string(&path)?, "first version\n");
        assert!(save_text_atomic(&path, b"overwrite attempt\n").is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "first version\n");
        Ok(())
    }
}
