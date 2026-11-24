//! Saving captured bodies to files from the session body store.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_proxy::exchange::{BodySide, ByteSize, Exchange};
use http_proxy::recording::{
    BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES, BodyStore, SaveBody, SaveFormat, SavedBody,
    StoredBody,
};

use crate::ui::dialogs::save::SaveControls;
use crate::ui::style::theme::Palette;

use super::background_task::{BackgroundTask, TaskPoll};

const MIB: u64 = 1024 * 1024;
const SETTINGS_FILE: &str = "body-store";
const LIMIT_KEY: &str = "limit_mib";
const MAX_LIMIT_MIB: u64 = 1024 * 1024;
pub(in crate::ui) const PRIVACY_NOTICE: &str = "Saved bodies can contain cookies, tokens and personal data. \
     Files are created readable only by you; keep them private.";

/// A save waiting for the user to confirm a warning.
struct Confirmation {
    request: SaveBody,
    incomplete: Option<String>,
    show_privacy: bool,
}

/// Outcome of the most recent single-body save attempt.
#[derive(Debug, Eq, PartialEq)]
enum SaveStatus {
    Saved(SavedBody),
    Failed(String),
}

/// Body store ownership, its settings, and the state of body saves.
pub(in crate::ui) struct BodySaving {
    store: Option<BodyStore>,
    init_error: Option<String>,
    limit_mib: u64,
    settings_path: Option<PathBuf>,
    settings_error: Option<String>,
    pending: Option<BackgroundTask<SavedBody>>,
    status: Option<SaveStatus>,
    confirmation: Option<Confirmation>,
    privacy_acknowledged: bool,
    /// The `Save bodies…` dialog and a running bulk save.
    pub(in crate::ui) bulk: crate::ui::dialogs::bulk_save::BulkControls,
    /// The `Merge ranges…` dialog.
    pub(in crate::ui) merge: crate::ui::dialogs::range_merge::MergeControls,
}

impl BodySaving {
    /// Opens this session's body store with the saved limit.
    pub(in crate::ui) fn open() -> Self {
        let settings_path =
            http_proxy::settings::paths::settings_directory(|name| std::env::var_os(name))
                .map(|directory| directory.join(SETTINGS_FILE));
        let limit_mib = settings_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| parse_limit(&text))
            .unwrap_or(BODY_STORE_LIMIT_BYTES / MIB);
        let opened =
            http_proxy::settings::paths::body_store_directory(|name| std::env::var_os(name))
                .ok_or_else(|| io::Error::other("no cache directory; check HOME"))
                .and_then(|root| {
                    BodyStore::with_limits(&root, limit_mib * MIB, BODY_STORE_MIN_FREE_BYTES)
                });
        let (store, init_error) = match opened {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(format!("Saving bodies is unavailable: {error}"))),
        };
        Self {
            store,
            init_error,
            limit_mib,
            settings_path,
            settings_error: None,
            pending: None,
            status: None,
            confirmation: None,
            privacy_acknowledged: false,
            bulk: crate::ui::dialogs::bulk_save::BulkControls::default(),
            merge: crate::ui::dialogs::range_merge::MergeControls::default(),
        }
    }

    #[cfg(test)]
    pub(in crate::ui) fn without_store() -> Self {
        Self {
            store: None,
            init_error: None,
            limit_mib: BODY_STORE_LIMIT_BYTES / MIB,
            settings_path: None,
            settings_error: None,
            pending: None,
            status: None,
            confirmation: None,
            privacy_acknowledged: false,
            bulk: crate::ui::dialogs::bulk_save::BulkControls::default(),
            merge: crate::ui::dialogs::range_merge::MergeControls::default(),
        }
    }

    /// Reports whether a body save is running or waiting for confirmation;
    /// another save cannot start until it finishes.
    pub(in crate::ui) const fn is_busy(&self) -> bool {
        self.pending.is_some() || self.confirmation.is_some() || self.merge.is_open()
    }

    /// Opens the `Save bodies…` dialog.
    pub(in crate::ui) fn open_bulk(&mut self) {
        self.bulk.open();
    }

    pub(in crate::ui) const fn store(&self) -> Option<&BodyStore> {
        self.store.as_ref()
    }

    /// The request and response `Save … body ▼` menus of the Body tab.
    pub(in crate::ui) fn menus(
        &mut self,
        ui: &mut egui::Ui,
        record: &Exchange,
        save: &SaveControls,
    ) {
        ui.horizontal_wrapped(|ui| {
            for side in [BodySide::Request, BodySide::Response] {
                self.menu(ui, record, side, save);
            }
        });
    }

    /// One `Save … body ▼` menu of the Body tab.
    fn menu(&mut self, ui: &mut egui::Ui, record: &Exchange, side: BodySide, save: &SaveControls) {
        let stored = self
            .store
            .as_ref()
            .and_then(|store| store.body(record.id(), side));
        let label = match side {
            BodySide::Request => "Save request body ▼",
            BodySide::Response => "Save response body ▼",
        };
        let enabled = stored.is_some() && !self.is_busy();
        let response = ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(label, |ui| {
                if let Some(stored) = stored {
                    ui.weak(stored_text(&stored));
                }
                for format in [SaveFormat::Decoded, SaveFormat::AsReceived] {
                    let (text, hover) = format_text(format);
                    if ui.button(text).on_hover_text(hover).clicked() {
                        ui.close();
                        self.request(record, side, format, save.directory());
                    }
                }
                if side == BodySide::Response && self.merge.menu_items(ui, record) {
                    ui.close();
                }
            })
        });
        response
            .response
            .on_disabled_hover_text(self.unavailable_text());
    }

    fn unavailable_text(&self) -> &str {
        if self.is_busy() {
            "Another body is being saved."
        } else if let Some(error) = self.init_error.as_deref() {
            error
        } else {
            "This body was not stored."
        }
    }

    /// Starts saving one body, asking first when it is incomplete or on the
    /// first save of the session.
    pub(in crate::ui) fn request(
        &mut self,
        record: &Exchange,
        side: BodySide,
        format: SaveFormat,
        directory: io::Result<PathBuf>,
    ) {
        // Starting another save would replace the pending receiver and lose
        // the running save's result, so callers disable their controls.
        if self.is_busy() {
            return;
        }
        self.status = None;
        let result = directory
            .map_err(|error| error.to_string())
            .and_then(|directory| {
                let store = self
                    .store
                    .as_ref()
                    .ok_or_else(|| self.unavailable_text().to_owned())?;
                let stored = store
                    .body(record.id(), side)
                    .ok_or_else(|| "This body was not stored.".to_owned())?;
                let request = SaveBody::new(record, side, format, directory)
                    .ok_or_else(|| "No response was received.".to_owned())?;
                Ok((request, stored))
            });
        match result {
            Ok((request, stored)) => {
                let incomplete = stored.incomplete_reason();
                if incomplete.is_some() || !self.privacy_acknowledged {
                    self.confirmation = Some(Confirmation {
                        request,
                        incomplete,
                        show_privacy: !self.privacy_acknowledged,
                    });
                } else {
                    self.start(request);
                }
            }
            Err(error) => self.status = Some(SaveStatus::Failed(error)),
        }
    }

    /// Shows the result of a save started elsewhere, such as a range merge.
    pub(super) fn track(&mut self, task: BackgroundTask<SavedBody>) {
        self.status = None;
        self.pending = Some(task);
    }

    fn start(&mut self, request: SaveBody) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.save(request) {
            Ok(receiver) => self.pending = Some(BackgroundTask::track(receiver)),
            Err(error) => self.status = Some(SaveStatus::Failed(error.to_string())),
        }
    }

    /// Shows the confirmation dialog and collects finished saves; call once per frame.
    pub(in crate::ui) fn update(&mut self, ctx: &egui::Context) {
        self.poll();
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let Some(confirmation) = self.confirmation.as_ref() else {
            return;
        };
        let mut choice = None;
        let max_width = (ctx.content_rect().width() - 48.0).clamp(240.0, 420.0);
        egui::Modal::new(egui::Id::unique("confirm-body-save")).show(ctx, |ui| {
            ui.set_max_width(max_width);
            ui.heading("Save body");
            if let Some(reason) = confirmation.incomplete.as_deref() {
                ui.colored_label(
                    Palette::of(ui).warning,
                    format!(
                        "This body is incomplete. {reason} The file name will end in .partial."
                    ),
                );
            }
            if confirmation.show_privacy {
                ui.label(PRIVACY_NOTICE);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if choice == Some(true)
            && let Some(confirmation) = self.confirmation.take()
        {
            self.privacy_acknowledged |= confirmation.show_privacy;
            self.start(confirmation.request);
        } else if choice == Some(false) {
            self.confirmation = None;
        }
    }

    fn poll(&mut self) {
        let Some(task) = self.pending.as_ref() else {
            return;
        };
        let status = match task.poll() {
            TaskPoll::Ready(Ok(saved)) => SaveStatus::Saved(saved),
            TaskPoll::Ready(Err(error)) => SaveStatus::Failed(error.to_string()),
            TaskPoll::Pending => return,
            TaskPoll::Disconnected => {
                SaveStatus::Failed("The save worker stopped unexpectedly".to_owned())
            }
        };
        self.pending = None;
        self.status = Some(status);
    }

    /// The body store limit inside the Settings menu.
    pub(in crate::ui) fn settings_menu(&mut self, ui: &mut egui::Ui) {
        let max_width = (ui.ctx().content_rect().width() - 48.0).clamp(220.0, 380.0);
        ui.set_max_width(max_width);
        ui.label("Body store limit");
        let changed = ui
            .add(
                egui::DragValue::new(&mut self.limit_mib)
                    .range(1..=MAX_LIMIT_MIB)
                    .suffix(" MiB"),
            )
            .on_hover_text(
                "Most body bytes kept for saving in one session. Later bodies are not stored once it is reached.",
            )
            .changed();
        if changed {
            if let Some(store) = self.store.as_ref() {
                store.set_limit_bytes(self.limit_mib * MIB);
            }
            self.settings_error = self
                .save_settings()
                .err()
                .map(|error| format!("Could not save the body store limit: {error}"));
        }
        if let Some(store) = self.store.as_ref() {
            ui.weak(format!(
                "{} used · stored in {}",
                ByteSize::new(store.used_bytes()),
                store.directory().display()
            ));
        }
    }

    fn save_settings(&self) -> io::Result<()> {
        use std::io::Write as _;
        let path = self
            .settings_path
            .as_ref()
            .ok_or_else(|| io::Error::other("settings directory unavailable"))?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid settings path"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        writeln!(file, "{LIMIT_KEY}={}", self.limit_mib)?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Body store warnings and the result of the last body save, for the status bar.
    pub(in crate::ui) fn status(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        for warning in self.warnings() {
            ui.separator();
            ui.colored_label(palette.warning, warning);
        }
        if let Some(error) = self.init_error.as_ref().or(self.settings_error.as_ref()) {
            ui.separator();
            ui.colored_label(palette.error, error);
        }
        if self.pending.is_some() {
            ui.separator();
            ui.spinner();
            ui.label("Saving body…");
        }
        self.bulk.status(ui);
        match &self.status {
            Some(SaveStatus::Saved(saved)) => {
                ui.separator();
                let suffix = if saved.complete() {
                    ""
                } else {
                    " (incomplete)"
                };
                ui.label(format!(
                    "Saved body{suffix}: {} ({})",
                    saved.path().display(),
                    ByteSize::new(saved.bytes())
                ));
            }
            Some(SaveStatus::Failed(error)) => {
                ui.separator();
                ui.colored_label(palette.error, format!("Body save failed: {error}"));
            }
            None => {}
        }
    }

    fn warnings(&self) -> Vec<String> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        let mut warnings = Vec::new();
        if let Some(error) = store.error() {
            warnings.push(format!("{error}. Later bodies are not stored."));
        }
        if store.limit_reached() {
            warnings.push(format!(
                "The body store reached its {} limit. Later bodies are not stored; raise the limit in Settings.",
                ByteSize::new(store.limit_bytes())
            ));
        }
        if store.low_disk_space() {
            warnings.push(format!(
                "Free disk space is below {}. Bodies are not stored until space is freed.",
                ByteSize::new(store.min_free_bytes())
            ));
        }
        warnings
    }
}

fn parse_limit(text: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == LIMIT_KEY)
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
        .filter(|limit| (1..=MAX_LIMIT_MIB).contains(limit))
}

const fn format_text(format: SaveFormat) -> (&'static str, &'static str) {
    match format {
        SaveFormat::Decoded => (
            "Decoded",
            "Remove gzip, deflate, br and zstd Content-Encoding so the file holds the actual content.",
        ),
        SaveFormat::AsReceived => (
            "As received",
            "Keep the exact bytes that were received, including any Content-Encoding.",
        ),
    }
}

fn stored_text(stored: &StoredBody) -> String {
    let size = ByteSize::new(stored.stored_bytes());
    stored.incomplete_reason().map_or_else(
        || format!("{size} stored · complete"),
        |reason| format!("{size} stored · incomplete: {reason}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_valid_limits() {
        assert_eq!(parse_limit("limit_mib=512\n"), Some(512));
        assert_eq!(parse_limit("other=1\n limit_mib = 64 \n"), Some(64));
        for text in ["", "limit_mib=0", "limit_mib=x", "limit_mib=99999999"] {
            assert_eq!(parse_limit(text), None, "{text}");
        }
    }

    #[test]
    fn a_running_save_blocks_a_new_request() -> io::Result<()> {
        let (task, _starter) = BackgroundTask::deferred();
        let mut saving = BodySaving {
            store: None,
            init_error: None,
            limit_mib: 1,
            settings_path: None,
            settings_error: None,
            pending: Some(task),
            status: Some(SaveStatus::Failed("earlier".to_owned())),
            confirmation: None,
            privacy_acknowledged: true,
            bulk: crate::ui::dialogs::bulk_save::BulkControls::default(),
            merge: crate::ui::dialogs::range_merge::MergeControls::default(),
        };
        assert!(saving.is_busy());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let exchange = runtime.block_on(sample_exchange())?;
        saving.request(
            &exchange,
            BodySide::Request,
            SaveFormat::Decoded,
            Ok(PathBuf::new()),
        );
        assert!(saving.pending.is_some(), "the running save is kept");
        assert_eq!(
            saving.status,
            Some(SaveStatus::Failed("earlier".to_owned()))
        );
        Ok(())
    }

    /// Captures one rejected request through a real proxy.
    async fn sample_exchange() -> io::Result<Exchange> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut proxy = http_proxy::interception::ProxyHandle::start(
            http_proxy::interception::ProxyConfig::loopback(0),
        )
        .map_err(io::Error::other)?;
        let mut client = tokio::net::TcpStream::connect(proxy.local_addr()).await?;
        client
            .write_all(b"GET ftp://example.invalid/ HTTP/1.1\r\nHost: example.invalid\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        client.read_to_end(&mut response).await?;
        let exchange = proxy
            .receive()
            .await
            .ok_or_else(|| io::Error::other("no capture"))?;
        proxy.stop().map_err(io::Error::other)?;
        Ok(exchange)
    }
}
