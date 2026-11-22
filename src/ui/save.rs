use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, ColorImage};
use http_proxy::{Exchange, FullLog};

use crate::ui::har::{self, HarOptions};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

type SaveCompletion = io::Result<PathBuf>;

pub(in crate::ui) struct SaveControls {
    directory: String,
    pending: Option<Receiver<SaveCompletion>>,
    status: Option<SaveCompletion>,
}

impl SaveControls {
    pub(in crate::ui) fn new() -> Self {
        Self {
            directory: std::env::current_dir()
                .map_or_else(|_| String::new(), |path| path.display().to_string()),
            pending: None,
            status: None,
        }
    }

    /// Shows the `Save` menu in the toolbar and reports whether the user
    /// asked to export a HAR file.
    pub(in crate::ui) fn menu(&mut self, ui: &mut egui::Ui, log: Option<&FullLog>) -> bool {
        self.poll();
        if self.pending.is_some() {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add_enabled_ui(self.pending.is_none(), |ui| {
            ui.menu_button("Save ▼", |ui| self.menu_items(ui, log))
                .inner
                .unwrap_or_default()
        })
        .inner
    }

    /// Reports whether a save is still running.
    pub(in crate::ui) const fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Suggests a new file in the save directory, without checking it.
    pub(in crate::ui) fn suggested_path(&self, extension: &str) -> String {
        self.destination(extension)
            .map_or_else(|_| String::new(), |path| path.display().to_string())
    }

    /// Writes `records` as HAR to `path` on a worker thread.
    pub(in crate::ui) fn export_har(
        &mut self,
        context: &egui::Context,
        records: Vec<Exchange>,
        options: HarOptions,
        path: PathBuf,
    ) {
        self.status = None;
        let (sender, receiver) = mpsc::channel();
        let worker_context = context.clone();
        let spawned = std::thread::Builder::new()
            .name("export-har".to_owned())
            .spawn(move || {
                let document = har::document(&records, options, SystemTime::now());
                drop(records);
                let _receiver_closed = sender.send(har::write(&path, &document));
                worker_context.request_repaint();
            });
        match spawned {
            Ok(_worker) => self.pending = Some(receiver),
            Err(error) => self.status = Some(Err(error)),
        }
    }

    fn menu_items(&mut self, ui: &mut egui::Ui, log: Option<&FullLog>) -> bool {
        ui.label("Save directory");
        ui.add(
            egui::TextEdit::singleline(&mut self.directory)
                .desired_width(320.0)
                .hint_text("Existing directory"),
        );
        if ui
            .button("Save window PNG")
            .on_hover_text("Save the visible app content, without the OS window border.")
            .clicked()
        {
            ui.close();
            self.start_screenshot(ui.ctx());
        }
        if ui
            .add_enabled(log.is_some(), egui::Button::new("Save all logs"))
            .on_hover_text("Save all captured records and full bodies, regardless of the filter.")
            .clicked()
            && let Some(log) = log
        {
            ui.close();
            self.status = None;
            match self.destination("log").and_then(|path| log.export(path)) {
                Ok(receiver) => self.pending = Some(receiver),
                Err(error) => self.status = Some(Err(error)),
            }
        }
        let export_har = ui
            .button("Export HAR…")
            .on_hover_text(
                "Save the requests in the list as a HAR 1.2 file. Bodies and sensitive headers are left out unless you include them.",
            )
            .clicked();
        if export_har {
            ui.close();
        }
        export_har
    }

    /// Shows the result of the last save in the status bar.
    pub(in crate::ui) fn status(&self, ui: &mut egui::Ui) {
        match &self.status {
            Some(Ok(path)) => {
                ui.label(format!("Saved: {}", path.display()));
            }
            Some(Err(error)) => {
                ui.colored_label(
                    crate::ui::theme::Palette::of(ui).error,
                    format!("Save failed: {error}"),
                );
            }
            None => {}
        }
    }

    fn poll(&mut self) {
        let Some(receiver) = &self.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err(io::Error::other("The save worker stopped unexpectedly"))
            }
        };
        self.pending = None;
        self.status = Some(result);
    }

    fn destination(&self, extension: &str) -> io::Result<PathBuf> {
        let directory = Path::new(&self.directory);
        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Choose an existing save directory",
            ));
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        Ok(directory.join(format!(
            "http-inspector-{}-{:09}-{}-{sequence}.{extension}",
            timestamp.as_secs(),
            timestamp.subsec_nanos(),
            std::process::id(),
        )))
    }

    fn start_screenshot(&mut self, context: &egui::Context) {
        self.status = None;
        let path = match self.destination("png") {
            Ok(path) => path,
            Err(error) => {
                self.status = Some(Err(error));
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        let context = context.clone();
        context.clone().request_screenshot(move |image| {
            let worker_sender = sender.clone();
            let worker_context = context.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("save-window-png".to_owned())
                .spawn(move || {
                    let result = save_png(&path, &image).map(|()| path);
                    let _receiver_closed = worker_sender.send(result);
                    worker_context.request_repaint();
                })
            {
                let _receiver_closed = sender.send(Err(error));
                context.request_repaint();
            }
        });
    }
}

fn save_png(path: &Path, image: &ColorImage) -> io::Result<()> {
    let width = u32::try_from(image.size[0]).map_err(io::Error::other)?;
    let height = u32::try_from(image.size[1]).map_err(io::Error::other)?;
    let pixels: Vec<u8> = image.pixels.iter().flat_map(Color32::to_array).collect();
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Save path has no directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    {
        let mut encoder = png::Encoder::new(temporary.as_file_mut(), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        writer.write_image_data(&pixels).map_err(io::Error::other)?;
        writer.finish().map_err(io::Error::other)?;
    }
    temporary.flush()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(())
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
}
