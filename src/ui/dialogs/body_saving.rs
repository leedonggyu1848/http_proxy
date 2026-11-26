//! Saving captured bodies to files from the session body store.

use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::tokens::Layout;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::recording::{
    BODY_STORE_LIMIT_BYTES, BODY_STORE_MIN_FREE_BYTES, BodyStore, SaveBody, SaveFormat, SavedBody,
    StoredBody,
};
use http_inspector_exchange::{BodySide, ByteSize, Exchange};
use http_inspector_file::{ExistingFile, publish};

use crate::ui::AppAction;
use crate::ui::style::indicator;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Tone;

use super::background_task::{BackgroundTask, TaskPoll};

const MIB: u64 = 1024 * 1024;
const SETTINGS_FILE: &str = "body-store";
const LIMIT_KEY: &str = "limit_mib";
const MAX_LIMIT_MIB: u64 = 1024 * 1024;

pub(super) const fn privacy_notice(lang: Language) -> &'static str {
    lang.pick(
        "Saved bodies can contain cookies, tokens and personal data. \
         Files are created readable only by you; keep them private.",
        "저장된 본문에는 쿠키, 토큰, 개인정보가 포함될 수 있습니다. \
         파일은 현재 사용자만 읽을 수 있도록 생성됩니다. 안전하게 보관하세요.",
    )
}

/// A save waiting for the user to confirm a warning.
struct Confirmation {
    request: SaveBody,
    stored: StoredBody,
    show_privacy: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum InitError {
    NoCacheDirectory,
    Open(String),
}

impl InitError {
    fn localized(&self, lang: Language) -> String {
        match (self, lang) {
            (Self::NoCacheDirectory, Language::English) => {
                "Saving bodies is unavailable: no cache directory; check HOME".to_owned()
            }
            (Self::NoCacheDirectory, Language::Korean) => {
                "본문 저장을 사용할 수 없습니다: 캐시 폴더를 찾을 수 없습니다(HOME을 확인하세요)"
                    .to_owned()
            }
            (Self::Open(detail), Language::English) => {
                format!("Saving bodies is unavailable: {detail}")
            }
            (Self::Open(detail), Language::Korean) => {
                format!("본문 저장을 사용할 수 없습니다: {detail}")
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum BodySaveFailure {
    Directory(String, bool),
    Init(InitError),
    NotStored,
    NoResponse,
    WorkerStopped,
    Other(String),
}

impl BodySaveFailure {
    fn from_dir_error(error: &io::Error) -> Self {
        let is_missing = error.kind() == io::ErrorKind::NotADirectory
            && error.to_string() == "Choose an existing save directory in the Save menu";
        Self::Directory(error.to_string(), is_missing)
    }

    fn localized(&self, lang: Language) -> String {
        match self {
            Self::Directory(_, true) => lang
                .pick(
                    "Choose an existing save directory in the Save menu",
                    "저장 메뉴에서 존재하는 저장 폴더를 선택하세요",
                )
                .to_owned(),
            Self::Directory(detail, false) | Self::Other(detail) => detail.clone(),
            Self::Init(err) => err.localized(lang),
            Self::NotStored => lang
                .pick(
                    "This body was not stored.",
                    "이 본문은 저장되지 않았습니다.",
                )
                .to_owned(),
            Self::NoResponse => lang
                .pick("No response was received.", "수신된 응답이 없습니다.")
                .to_owned(),
            Self::WorkerStopped => lang
                .pick(
                    "The save worker stopped unexpectedly",
                    "저장 작업자가 예기치 않게 중지되었습니다",
                )
                .to_owned(),
        }
    }
}

/// Outcome of the most recent single-body save attempt.
#[derive(Debug, Eq, PartialEq)]
enum SaveStatus {
    Saved(SavedBody),
    Failed(BodySaveFailure),
}

/// Body store ownership, its settings, and the state of body saves.
pub(crate) struct BodySaving {
    store: Option<BodyStore>,
    init_error: Option<InitError>,
    limit_mib: u64,
    settings_path: Option<PathBuf>,
    settings_error: Option<String>,
    pending: Option<BackgroundTask<SavedBody>>,
    status: Option<SaveStatus>,
    confirmation: Option<Confirmation>,
    privacy_acknowledged: bool,
    /// The `Save bodies…` dialog and a running bulk save.
    pub(super) bulk: crate::ui::dialogs::bulk_save::BulkControls,
    /// The `Merge ranges…` dialog.
    pub(super) merge: crate::ui::dialogs::range_merge::MergeControls,
}

impl BodySaving {
    /// Opens this session's body store with the saved limit.
    pub(crate) fn open() -> Self {
        let settings_path =
            http_inspector_core::settings::paths::settings_directory(|name| std::env::var_os(name))
                .map(|directory| directory.join(SETTINGS_FILE));
        let limit_mib = settings_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| parse_limit(&text))
            .unwrap_or(BODY_STORE_LIMIT_BYTES / MIB);
        let cache_dir = http_inspector_core::settings::paths::body_store_directory(|name| {
            std::env::var_os(name)
        });
        let (store, init_error) = cache_dir.map_or_else(
            || (None, Some(InitError::NoCacheDirectory)),
            |root| match BodyStore::with_limits(&root, limit_mib * MIB, BODY_STORE_MIN_FREE_BYTES) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(InitError::Open(error.to_string()))),
            },
        );
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
    pub(crate) fn without_store() -> Self {
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
    pub(crate) const fn is_busy(&self) -> bool {
        self.pending.is_some() || self.confirmation.is_some() || self.merge.is_open()
    }

    pub(crate) const fn dialog_is_open(&self) -> bool {
        self.confirmation.is_some() || self.bulk.is_open() || self.merge.is_open()
    }

    /// Opens the `Save bodies…` dialog.
    pub(crate) fn open_bulk(&mut self) {
        self.bulk.open();
    }

    pub(crate) const fn store(&self) -> Option<&BodyStore> {
        self.store.as_ref()
    }

    /// The request and response `Save … body ▼` menus of the Body tab.
    pub(crate) fn menus(
        &self,
        ui: &mut egui::Ui,
        record: &Exchange,
        lang: Language,
    ) -> Option<AppAction> {
        ui.horizontal_wrapped(|ui| {
            let request = self.menu(ui, record, BodySide::Request, lang);
            let response = self.menu(ui, record, BodySide::Response, lang);
            request.or(response)
        })
        .inner
    }

    /// One `Save … body ▼` menu of the Body tab.
    fn menu(
        &self,
        ui: &mut egui::Ui,
        record: &Exchange,
        side: BodySide,
        lang: Language,
    ) -> Option<AppAction> {
        let stored = self
            .store
            .as_ref()
            .and_then(|store| store.body(record.id(), side));
        let label = match side {
            BodySide::Request => lang.pick("Save request body ▼", "요청 본문 저장 ▼"),
            BodySide::Response => lang.pick("Save response body ▼", "응답 본문 저장 ▼"),
        };
        let enabled = stored.is_some() && !self.is_busy();
        let response = ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(label, |ui| {
                if let Some(stored) = stored {
                    ui.weak(stored_text(&stored, lang));
                }
                for format in [SaveFormat::Decoded, SaveFormat::AsReceived] {
                    let (text, hover) = format_text(format, lang);
                    if ui.button(text).on_hover_text(hover).clicked() {
                        ui.close();
                        return Some(AppAction::SaveBody(record.id(), side, format));
                    }
                }
                if side == BodySide::Response && super::range_merge::menu_items(ui, record, lang) {
                    ui.close();
                    return Some(AppAction::PlanRanges(record.id()));
                }
                None
            })
        });
        response
            .response
            .on_disabled_hover_text(self.unavailable_text(lang));
        response.inner.inner.flatten()
    }

    fn unavailable_text(&self, lang: Language) -> String {
        if self.is_busy() {
            lang.pick(
                "Another body is being saved.",
                "다른 본문을 저장하는 중입니다.",
            )
            .to_owned()
        } else if let Some(error) = self.init_error.as_ref() {
            error.localized(lang)
        } else {
            lang.pick(
                "This body was not stored.",
                "이 본문은 저장되지 않았습니다.",
            )
            .to_owned()
        }
    }

    /// Starts saving one body, asking first when it is incomplete or on the
    /// first save of the session.
    pub(crate) fn request(
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
            .map_err(|error| BodySaveFailure::from_dir_error(&error))
            .and_then(|directory| {
                let store = self.store.as_ref().ok_or_else(|| {
                    self.init_error
                        .clone()
                        .map_or(BodySaveFailure::NotStored, BodySaveFailure::Init)
                })?;
                let stored = store
                    .body(record.id(), side)
                    .ok_or(BodySaveFailure::NotStored)?;
                let request = SaveBody::new(record, side, format, directory)
                    .ok_or(BodySaveFailure::NoResponse)?;
                Ok((request, stored))
            });
        match result {
            Ok((request, stored)) => {
                let is_incomplete = stored.incomplete_reason().is_some();
                if is_incomplete || !self.privacy_acknowledged {
                    self.confirmation = Some(Confirmation {
                        request,
                        stored,
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
            Err(error) => {
                self.status = Some(SaveStatus::Failed(BodySaveFailure::Other(
                    error.to_string(),
                )));
            }
        }
    }

    /// Shows the pending confirmation without starting or cancelling a save.
    pub(crate) fn show_confirmation(&self, ctx: &egui::Context, lang: Language) -> Option<bool> {
        let confirmation = self.confirmation.as_ref()?;
        let mut choice = None;
        let max_width = Layout::BODY_CONFIRM_WIDTH.fit(ctx.content_rect().width());
        let modal = egui::Modal::new(egui::Id::unique("confirm-body-save")).show(ctx, |ui| {
            ui.set_max_width(max_width);
            ui.heading(lang.pick("Save body", "본문 저장"));
            if let Some(reason) = domain::stored_body_incomplete_reason(lang, &confirmation.stored)
            {
                let warning = match lang {
                    Language::English => format!(
                        "This body is incomplete. {reason} The file name will end in .partial."
                    ),
                    Language::Korean => {
                        format!(
                            "이 본문은 불완전합니다. {reason} 파일 이름 끝에 .partial이 붙습니다."
                        )
                    }
                };
                ui.colored_label(Palette::of(ui).warning, warning);
            }
            if confirmation.show_privacy {
                ui.label(privacy_notice(lang));
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button(lang.text(Text::Save)).clicked() {
                    choice = Some(true);
                }
                if ui.button(lang.text(Text::Cancel)).clicked() {
                    choice = Some(false);
                }
            });
        });
        if modal.should_close() {
            Some(false)
        } else {
            choice
        }
    }

    pub(crate) fn apply_confirmation(&mut self, confirmed: bool) {
        if let Some(confirmation) = self.confirmation.take()
            && confirmed
        {
            self.privacy_acknowledged |= confirmation.show_privacy;
            self.start(confirmation.request);
        }
    }

    pub(crate) fn poll(&mut self, context: &egui::Context) {
        self.bulk.poll(context);
        if self.pending.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
        let Some(task) = self.pending.as_ref() else {
            return;
        };
        let status = match task.poll() {
            TaskPoll::Ready(Ok(saved)) => SaveStatus::Saved(saved),
            TaskPoll::Ready(Err(error)) => {
                SaveStatus::Failed(BodySaveFailure::Other(error.to_string()))
            }
            TaskPoll::Pending => return,
            TaskPoll::Disconnected => SaveStatus::Failed(BodySaveFailure::WorkerStopped),
        };
        self.pending = None;
        self.status = Some(status);
    }

    /// The body store limit inside the Settings menu.
    pub(crate) fn settings_menu(&self, ui: &mut egui::Ui, lang: Language) -> Option<u64> {
        let max_width = Layout::BODY_SETTINGS_WIDTH.fit(ui.ctx().content_rect().width());
        ui.set_max_width(max_width);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.label(lang.pick("Body store limit", "본문 저장소 한도"));
            let mut limit = self.limit_mib;
            let changed = ui
                .add(
                    egui::DragValue::new(&mut limit)
                        .range(1..=MAX_LIMIT_MIB)
                        .suffix(" MiB"),
                )
                .on_hover_text(lang.pick(
                    "Maximum body bytes kept for saving in one session; later bodies are not stored once reached.",
                    "한 세션에서 저장용으로 보관할 최대 본문 용량이며, 한도에 도달하면 이후 본문은 저장되지 않습니다.",
                ))
                .changed();
            if let Some(store) = self.store.as_ref() {
                let used = ByteSize::new(store.used_bytes());
                let dir = store.directory().display();
                let summary = match lang {
                    Language::English => format!("{used} used · stored in {dir}"),
                    Language::Korean => format!("{used} 사용 중 · 저장 위치: {dir}"),
                };
                ui.weak(summary);
            }
            changed.then_some(limit)
        })
        .inner
    }

    pub(crate) fn set_limit(&mut self, limit_mib: u64) {
        self.limit_mib = limit_mib;
        if let Some(store) = self.store.as_ref() {
            store.set_limit_bytes(limit_mib * MIB);
        }
        self.settings_error = self.save_settings().err().map(|error| error.to_string());
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
        publish(path, ExistingFile::Replace, |file| {
            writeln!(file, "{LIMIT_KEY}={}", self.limit_mib)
        })
    }

    /// Body store warnings and the result of the last body save, for the status bar.
    pub(crate) fn status(&self, ui: &mut egui::Ui, lang: Language) -> bool {
        for warning in self.warnings(lang) {
            ui.separator();
            indicator::dot_label(ui, Tone::Warning, warning);
        }
        if let Some(error) = self.init_error.as_ref() {
            ui.separator();
            indicator::dot_label(ui, Tone::Danger, error.localized(lang));
        } else if let Some(detail) = self.settings_error.as_deref() {
            ui.separator();
            let text = match lang {
                Language::English => format!("Could not save the body store limit: {detail}"),
                Language::Korean => format!("본문 저장소 한도를 저장하지 못했습니다: {detail}"),
            };
            indicator::dot_label(ui, Tone::Danger, text);
        }
        if self.pending.is_some() {
            ui.separator();
            ui.spinner();
            ui.label(lang.pick("Saving body…", "본문 저장 중…"));
        }
        let cancel_bulk = self.bulk.status(ui, lang);
        match &self.status {
            Some(SaveStatus::Saved(saved)) => {
                ui.separator();
                let path = saved.path().display();
                let size = ByteSize::new(saved.bytes());
                let text = match (lang, saved.complete()) {
                    (Language::English, true) => format!("Saved body: {path} ({size})"),
                    (Language::English, false) => {
                        format!("Saved body (incomplete): {path} ({size})")
                    }
                    (Language::Korean, true) => format!("본문 저장됨: {path} ({size})"),
                    (Language::Korean, false) => format!("본문 저장됨(불완전): {path} ({size})"),
                };
                ui.label(text);
            }
            Some(SaveStatus::Failed(error)) => {
                ui.separator();
                let detail = error.localized(lang);
                let text = match lang {
                    Language::English => format!("Body save failed: {detail}"),
                    Language::Korean => format!("본문 저장 실패: {detail}"),
                };
                indicator::dot_label(ui, Tone::Danger, text);
            }
            None => {}
        }
        cancel_bulk
    }

    pub(crate) fn cancel_bulk(&self) {
        self.bulk.cancel();
    }

    fn warnings(&self, lang: Language) -> Vec<String> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        let mut warnings = Vec::new();
        if let Some(error) = store.error() {
            warnings.push(match lang {
                Language::English => format!("{error}. Later bodies are not stored."),
                Language::Korean => format!("{error}. 이후 본문은 저장되지 않습니다."),
            });
        }
        if store.limit_reached() {
            let limit = ByteSize::new(store.limit_bytes());
            warnings.push(match lang {
                Language::English => format!(
                    "The body store reached its {limit} limit. Later bodies are not stored; raise the limit in Settings."
                ),
                Language::Korean => format!(
                    "본문 저장소가 {limit} 한도에 도달했습니다. 이후 본문은 저장되지 않으므로 설정에서 한도를 높이세요."
                ),
            });
        }
        if store.low_disk_space() {
            let min_free = ByteSize::new(store.min_free_bytes());
            warnings.push(match lang {
                Language::English => format!(
                    "Free disk space is below {min_free}. Bodies are not stored until space is freed."
                ),
                Language::Korean => format!(
                    "디스크 여유 공간이 {min_free} 미만입니다. 공간을 확보할 때까지 본문이 저장되지 않습니다."
                ),
            });
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

const fn format_text(format: SaveFormat, lang: Language) -> (&'static str, &'static str) {
    match format {
        SaveFormat::Decoded => (
            lang.pick("Decoded", "디코딩됨"),
            lang.pick(
                "Remove gzip, deflate, br and zstd Content-Encoding so the file holds the actual content.",
                "gzip, deflate, br, zstd Content-Encoding을 해제해 실제 콘텐츠를 파일에 저장합니다.",
            ),
        ),
        SaveFormat::AsReceived => (
            lang.pick("As received", "수신 원본 그대로"),
            lang.pick(
                "Keep the exact bytes that were received, including any Content-Encoding.",
                "Content-Encoding을 포함해 수신한 바이트 그대로 저장합니다.",
            ),
        ),
    }
}

fn stored_text(stored: &StoredBody, lang: Language) -> String {
    let size = ByteSize::new(stored.stored_bytes());
    domain::stored_body_incomplete_reason(lang, stored).map_or_else(
        || match lang {
            Language::English => format!("{size} stored · complete"),
            Language::Korean => format!("{size} 저장됨 · 완료"),
        },
        |reason| match lang {
            Language::English => format!("{size} stored · incomplete: {reason}"),
            Language::Korean => format!("{size} 저장됨 · 불완전: {reason}"),
        },
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
    fn saves_and_replaces_limit_settings() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("settings").join(SETTINGS_FILE);
        let mut saving = BodySaving {
            store: None,
            init_error: None,
            limit_mib: 64,
            settings_path: Some(path.clone()),
            settings_error: None,
            pending: None,
            status: None,
            confirmation: None,
            privacy_acknowledged: false,
            bulk: crate::ui::dialogs::bulk_save::BulkControls::default(),
            merge: crate::ui::dialogs::range_merge::MergeControls::default(),
        };
        for limit in [64, 128] {
            saving.limit_mib = limit;
            saving.save_settings()?;
            assert_eq!(parse_limit(&std::fs::read_to_string(&path)?), Some(limit));
        }
        Ok(())
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
            status: Some(SaveStatus::Failed(BodySaveFailure::Other(
                "earlier".to_owned(),
            ))),
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
            Some(SaveStatus::Failed(BodySaveFailure::Other(
                "earlier".to_owned(),
            )))
        );
        Ok(())
    }

    /// Captures one rejected request through a real proxy.
    async fn sample_exchange() -> io::Result<Exchange> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut proxy = http_inspector_core::interception::ProxyHandle::start(
            http_inspector_core::interception::ProxyConfig::loopback(0),
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

    fn collect_text_shapes(shapes: &[egui::Shape], out: &mut Vec<(String, bool)>) {
        for shape in shapes {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.galley.job.justify));
                }
                egui::Shape::Vec(nested) => collect_text_shapes(nested, out),
                _ => {}
            }
        }
    }

    #[test]
    fn settings_menu_text_is_not_justified_inside_menu_layout() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = BodyStore::with_limits(directory.path(), 64 * MIB, 0)?;
        let saving = BodySaving {
            store: Some(store),
            init_error: None,
            limit_mib: 64,
            settings_path: None,
            settings_error: None,
            pending: None,
            status: None,
            confirmation: None,
            privacy_acknowledged: false,
            bulk: crate::ui::dialogs::bulk_save::BulkControls::default(),
            merge: crate::ui::dialogs::range_merge::MergeControls::default(),
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                let _ = saving.settings_menu(ui, Language::English);
            });
        });
        output.textures_delta.clear();
        let shapes: Vec<_> = output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect();
        let mut texts = Vec::new();
        collect_text_shapes(&shapes, &mut texts);
        assert!(
            texts.iter().any(|(text, _)| text.contains("stored in")),
            "expected body store summary text to render"
        );
        assert!(
            texts.iter().all(|(_, justified)| !justified),
            "expected no justified text in settings menu: {texts:?}"
        );
        Ok(())
    }
}
