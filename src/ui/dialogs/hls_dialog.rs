//! Modal dialog for inspecting a captured HLS playlist and downloading MPEG-TS segments.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Key, RichText};
use egui_extras::{Column, TableBuilder};
use http_inspector_core::hls::{
    HlsBlockedReason, HlsDownloadConfig, HlsDownloadError, HlsDownloadHandle, HlsDownloadOutcome,
    HlsDownloadPhase, HlsDownloadProgress, HlsInspectError, HlsSessionPlan, HlsVariantEntry,
    MediaDownloadPlan, SegmentPlan, SegmentStatus, VariantMediaState, inspect_hls_session,
    start_hls_download,
};
use http_inspector_core::recording::BodyStore;
use http_inspector_exchange::{ByteSize, Exchange, RecordId};

use super::background_task::{BackgroundTask, TaskPoll};
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens, TypeScale};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const ROW_LEAD: f32 = Layout::SELECTION_BAR_WIDTH + Space::S;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HlsAction {
    SelectVariant(usize),
    Refresh,
    Download,
    CancelDownload,
    Close,
}

enum HlsInspectFailure {
    Inspect(HlsInspectError),
    WorkerDisconnected,
    Io(io::Error),
}

impl HlsInspectFailure {
    fn localized(&self, lang: Language) -> String {
        match self {
            Self::Inspect(error) => domain::hls_inspect_error(lang, error),
            Self::WorkerDisconnected => lang
                .pick(
                    "HLS inspection worker stopped unexpectedly",
                    "HLS 검사 작업자가 예기치 않게 중지되었습니다",
                )
                .to_owned(),
            Self::Io(error) => error.to_string(),
        }
    }
}

#[derive(Default)]
pub(crate) struct HlsDialog {
    source_record_id: Option<RecordId>,
    inspecting: Option<BackgroundTask<Result<HlsSessionPlan, HlsInspectError>>>,
    plan: Option<HlsSessionPlan>,
    inspect_error: Option<HlsInspectFailure>,
    selected_variant: usize,
    download: Option<ActiveDownload>,
    last_result: Option<FinishedDownload>,
}

struct ActiveDownload {
    variant_index: usize,
    handle: HlsDownloadHandle,
    progress: HlsDownloadProgress,
}

struct FinishedDownload {
    variant_index: usize,
    outcome: HlsDownloadOutcome,
    segment_statuses: Vec<SegmentStatus>,
}

impl HlsDialog {
    pub(crate) const fn is_open(&self) -> bool {
        self.source_record_id.is_some()
    }

    pub(crate) const fn source_record_id(&self) -> Option<RecordId> {
        self.source_record_id
    }

    pub(crate) const fn is_downloading(&self) -> bool {
        self.download.is_some()
    }

    /// Starts background inspection of `playlist_record` against the current session `records`.
    pub(crate) fn open(
        &mut self,
        playlist_record: Exchange,
        records: Vec<Exchange>,
        body_store: Option<BodyStore>,
        context: &egui::Context,
    ) {
        let same_record = self.source_record_id == Some(playlist_record.id());
        let previous_variant = same_record.then_some(self.selected_variant);
        self.source_record_id = Some(playlist_record.id());
        self.inspect_error = None;
        if !same_record {
            self.plan = None;
            self.download = None;
            self.last_result = None;
            self.selected_variant = 0;
        } else if !self.is_downloading() {
            self.last_result = None;
        }
        let task = BackgroundTask::start("hls-inspect", context, move || {
            Ok(inspect_hls_session(
                &playlist_record,
                &records,
                body_store.as_ref(),
            ))
        });
        self.inspecting = Some(task);
        if let Some(index) = previous_variant {
            self.selected_variant = index;
        }
    }

    /// Closes the dialog and cancels any in-flight download.
    pub(crate) fn close(&mut self) {
        if let Some(active) = self.download.take() {
            active.handle.cancel();
        }
        self.source_record_id = None;
        self.inspecting = None;
        self.plan = None;
        self.inspect_error = None;
        self.last_result = None;
        self.selected_variant = 0;
    }

    /// Switches the selected quality variant when no download is active.
    pub(crate) const fn select_variant(&mut self, index: usize) {
        if self.download.is_some() {
            return;
        }
        if let Some(plan) = &self.plan
            && index < plan.variants.len()
        {
            self.selected_variant = index;
        }
    }

    /// Starts downloading the currently selected variant into `output_directory`.
    pub(crate) fn start_download(
        &mut self,
        output_directory: PathBuf,
        body_store: Option<BodyStore>,
        proxy_addr: Option<SocketAddr>,
        context: &egui::Context,
    ) {
        if self.download.is_some() {
            return;
        }
        let Some(media_plan) = self
            .selected_entry()
            .and_then(HlsVariantEntry::media)
            .cloned()
        else {
            return;
        };
        let config = HlsDownloadConfig::new(media_plan, output_directory, body_store)
            .with_proxy_addr(proxy_addr);
        let wake_ctx = context.clone();
        match start_hls_download(config, move || wake_ctx.request_repaint()) {
            Ok(handle) => {
                let progress = handle.progress();
                self.last_result = None;
                self.download = Some(ActiveDownload {
                    variant_index: self.selected_variant,
                    handle,
                    progress,
                });
            }
            Err(error) => {
                self.last_result = Some(FinishedDownload {
                    variant_index: self.selected_variant,
                    outcome: HlsDownloadOutcome::Failed(error),
                    segment_statuses: Vec::new(),
                });
            }
        }
    }

    /// Cancels an active download without closing the dialog.
    pub(crate) fn cancel_download(&self) {
        if let Some(active) = &self.download {
            active.handle.cancel();
        }
    }

    /// Advances background inspection and download progress.
    pub(crate) fn poll(&mut self, context: &egui::Context) {
        self.poll_inspection(context);
        self.poll_download(context);
    }

    fn poll_inspection(&mut self, context: &egui::Context) {
        let Some(task) = self.inspecting.as_ref() else {
            return;
        };
        let result = match task.poll() {
            TaskPoll::Pending => {
                context.request_repaint_after(POLL_INTERVAL);
                return;
            }
            TaskPoll::Ready(result) => result,
            TaskPoll::Disconnected => {
                self.inspecting = None;
                self.inspect_error = Some(HlsInspectFailure::WorkerDisconnected);
                self.plan = None;
                return;
            }
        };
        self.inspecting = None;
        match result {
            Ok(Ok(plan)) => {
                let keep_selection =
                    self.plan.is_some() && self.selected_variant < plan.variants.len();
                if !keep_selection {
                    self.selected_variant = plan.default_variant;
                }
                self.plan = Some(plan);
                self.inspect_error = None;
                if !self.is_downloading() {
                    self.last_result = None;
                }
            }
            Ok(Err(error)) => {
                self.inspect_error = Some(HlsInspectFailure::Inspect(error));
                self.plan = None;
            }
            Err(error) => {
                self.inspect_error = Some(HlsInspectFailure::Io(error));
                self.plan = None;
            }
        }
    }

    fn poll_download(&mut self, context: &egui::Context) {
        let Some(active) = self.download.as_mut() else {
            return;
        };
        active.progress = active.handle.progress();
        if let Some(outcome) = active.handle.try_finish() {
            let progress = active.handle.progress();
            let variant_index = active.variant_index;
            self.download = None;
            self.last_result = Some(FinishedDownload {
                variant_index,
                outcome,
                segment_statuses: progress.segment_statuses,
            });
            context.request_repaint();
        } else {
            context.request_repaint_after(POLL_INTERVAL);
        }
    }

    fn selected_entry(&self) -> Option<&HlsVariantEntry> {
        self.plan
            .as_ref()
            .and_then(|plan| plan.variants.get(self.selected_variant))
    }

    fn segment_statuses_for_selected(&self) -> Option<&[SegmentStatus]> {
        if let Some(active) = &self.download
            && active.variant_index == self.selected_variant
        {
            return Some(&active.progress.segment_statuses);
        }
        if let Some(finished) = &self.last_result
            && finished.variant_index == self.selected_variant
            && !finished.segment_statuses.is_empty()
        {
            return Some(&finished.segment_statuses);
        }
        None
    }

    /// Renders the modal dialog when open and returns any user action.
    pub(crate) fn show(
        &self,
        context: &egui::Context,
        escape_enabled: bool,
        lang: Language,
    ) -> Option<HlsAction> {
        if !self.is_open() {
            return None;
        }
        let mut action = self.handle_keyboard(context);
        let viewport = context.content_rect();
        let width = Layout::HLS_DIALOG_WIDTH.fit(viewport.width());
        let max_height = Layout::HLS_DIALOG_HEIGHT.fit(viewport.height());

        let modal = egui::Modal::new(egui::Id::unique("hls-media-dialog")).show(context, |ui| {
            ui.set_width(width);
            ui.set_max_height(max_height);
            ui.spacing_mut().item_spacing.y = Space::SM;

            ui.heading(lang.pick("HLS media", "HLS 미디어"));
            if let Some(plan) = &self.plan {
                let palette = Palette::of(ui);
                ui.add(
                    egui::Label::new(
                        RichText::new(plan.source_url.as_str())
                            .monospace()
                            .size(TypeScale::STATUS)
                            .color(palette.secondary),
                    )
                    .truncate(),
                )
                .on_hover_text(plan.source_url.as_str());
            }

            ui.separator();
            if let Some(body_action) = self.render_body(ui, lang) {
                action = Some(body_action);
            }
            ui.separator();
            if let Some(footer_action) = self.render_footer(ui, lang) {
                action = Some(footer_action);
            }
        });

        let escaped = escape_enabled
            && (modal.should_close() || context.input(|input| input.key_pressed(Key::Escape)));
        action.or_else(|| escaped.then_some(HlsAction::Close))
    }

    fn handle_keyboard(&self, context: &egui::Context) -> Option<HlsAction> {
        if self.download.is_some() {
            return None;
        }
        let plan = self.plan.as_ref()?;
        if !plan.is_master || plan.variants.len() < 2 {
            return None;
        }
        let last = plan.variants.len().saturating_sub(1);
        context.input(|input| {
            if input.key_pressed(Key::ArrowUp) && self.selected_variant > 0 {
                Some(HlsAction::SelectVariant(self.selected_variant - 1))
            } else if input.key_pressed(Key::ArrowDown) && self.selected_variant < last {
                Some(HlsAction::SelectVariant(self.selected_variant + 1))
            } else {
                None
            }
        })
    }

    fn render_body(&self, ui: &mut egui::Ui, lang: Language) -> Option<HlsAction> {
        if self.inspecting.is_some() && self.plan.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(lang.pick("Inspecting playlist…", "재생목록 검사 중…"));
            });
            return None;
        }
        if let Some(error) = &self.inspect_error {
            let palette = Palette::of(ui);
            ui.add(
                egui::Label::new(RichText::new(error.localized(lang)).color(palette.error)).wrap(),
            );
            return None;
        }
        let plan = self.plan.as_ref()?;
        let mut action = None;

        if plan.is_master {
            if let Some(selected) = render_variants_table(
                ui,
                &plan.variants,
                self.selected_variant,
                self.is_downloading(),
                lang,
            ) {
                action = Some(HlsAction::SelectVariant(selected));
            }
            ui.add_space(Space::XS);
        }

        if let Some(entry) = self.selected_entry() {
            if let Some(media) = entry.media() {
                render_segments_section(
                    ui,
                    media,
                    self.segment_statuses_for_selected(),
                    self.is_downloading(),
                    lang,
                );
            }
            self.render_status_line(ui, entry, lang);
        }
        action
    }

    fn render_status_line(&self, ui: &mut egui::Ui, entry: &HlsVariantEntry, lang: Language) {
        if let Some(active) = &self.download {
            ui.add_space(Space::XS);
            render_progress_row(ui, &active.progress, lang);
            return;
        }
        if let Some(finished) = &self.last_result
            && finished.variant_index == self.selected_variant
        {
            ui.add_space(Space::XS);
            render_outcome_line(ui, &finished.outcome, lang);
            return;
        }
        if let Some(reason) = entry.blocked_reason() {
            ui.add_space(Space::XS);
            let palette = Palette::of(ui);
            ui.add(
                egui::Label::new(
                    RichText::new(domain::hls_blocked_reason(lang, &reason)).color(palette.warning),
                )
                .wrap(),
            );
        }
    }

    fn render_footer(&self, ui: &mut egui::Ui, lang: Language) -> Option<HlsAction> {
        let can_download = self
            .selected_entry()
            .is_some_and(HlsVariantEntry::can_download)
            && !self.is_downloading()
            && self.inspecting.is_none();

        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            let download_resp =
                primary_button(ui, lang.pick("Download .ts", ".ts 다운로드"), can_download);
            if download_resp.clicked() {
                action = Some(HlsAction::Download);
            }
            if self.is_downloading() {
                if ui.button(lang.text(Text::Cancel)).clicked() {
                    action = Some(HlsAction::CancelDownload);
                }
            } else if ui
                .add_enabled(
                    self.inspecting.is_none(),
                    egui::Button::new(lang.pick("Refresh", "새로고침")),
                )
                .clicked()
            {
                action = Some(HlsAction::Refresh);
            }
            if ui.button(lang.pick("Close", "닫기")).clicked() {
                action = Some(HlsAction::Close);
            }
        });
        action
    }
}

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let button = if enabled {
        let primary = tokens.primary_action;
        egui::Button::new(
            RichText::new(label)
                .size(TypeScale::CONTROL)
                .color(primary.text),
        )
        .fill(primary.fill)
        .stroke(egui::Stroke::new(StrokeWidth::STRUCTURAL, primary.stroke))
        .corner_radius(egui::CornerRadius::same(Radius::CONTROL))
    } else {
        egui::Button::new(RichText::new(label).size(TypeScale::CONTROL))
    };
    ui.add_enabled(enabled, button)
}

fn render_variants_table(
    ui: &mut egui::Ui,
    variants: &[HlsVariantEntry],
    selected: usize,
    locked: bool,
    lang: Language,
) -> Option<usize> {
    let palette = Palette::of(ui);
    ui.label(
        RichText::new(lang.pick("Media playlists", "미디어 재생목록"))
            .size(TypeScale::PANEL_TITLE)
            .color(palette.secondary),
    );

    let cols = Layout::HLS_VARIANT_COLUMNS;
    let mut clicked_row = None;
    let visible_rows = u8::try_from(variants.len().clamp(1, 4)).unwrap_or(4);
    let needs_scroll = variants.len() > 4;
    let body_height =
        (f32::from(visible_rows) * Layout::TABLE_ROW_HEIGHT).min(Layout::HLS_VARIANTS_HEIGHT);

    ui.push_id("hls-variants-table", |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        TableBuilder::new(ui)
            .striped(false)
            .vscroll(needs_scroll)
            .sense(egui::Sense::click())
            .auto_shrink([false, false])
            .min_scrolled_height(body_height)
            .max_scroll_height(body_height)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(cols.quality))
            .column(Column::exact(cols.bitrate))
            .column(Column::remainder().at_least(cols.playlist_min))
            .column(Column::exact(cols.status))
            .header(Layout::TABLE_HEADER_HEIGHT, |mut header| {
                header_col(
                    &mut header,
                    lang.pick("Quality", "화질"),
                    false,
                    ROW_LEAD,
                    false,
                );
                header_col(
                    &mut header,
                    lang.pick("Bitrate", "비트레이트"),
                    true,
                    Space::MD,
                    false,
                );
                header_col(
                    &mut header,
                    lang.pick("Playlist", "재생목록"),
                    false,
                    Space::S,
                    false,
                );
                header_col(
                    &mut header,
                    lang.text(Text::ColStatus),
                    false,
                    Space::S,
                    true,
                );
            })
            .body(|mut body| {
                body.ui_mut().style_mut().interaction.selectable_labels = false;
                body.rows(Layout::TABLE_ROW_HEIGHT, variants.len(), |mut row| {
                    let idx = row.index();
                    let Some(entry) = variants.get(idx) else {
                        return;
                    };
                    let is_selected = idx == selected;
                    row.set_selected(is_selected);
                    let cell_clicked =
                        render_variant_row(&mut row, entry, is_selected, &palette, lang);
                    if !locked && (row.response().clicked() || cell_clicked) {
                        clicked_row = Some(idx);
                    }
                });
            });
    });

    clicked_row
}

fn render_variant_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    entry: &HlsVariantEntry,
    is_selected: bool,
    palette: &Palette,
    lang: Language,
) -> bool {
    let mut cell_clicked = false;
    row.col(|ui| {
        paint_row_cell(ui, is_selected, false);
        ui.add_space(ROW_LEAD);
        let resp = ui.add(
            egui::Label::new(
                RichText::new(&entry.quality_label)
                    .monospace()
                    .size(TypeScale::DATA),
            )
            .truncate()
            .sense(egui::Sense::click()),
        );
        cell_clicked |= resp.clicked();
    });
    row.col(|ui| {
        paint_row_cell(ui, false, false);
        cell_clicked |= ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(Space::MD);
                ui.add(
                    egui::Label::new(
                        RichText::new(&entry.bandwidth_label)
                            .monospace()
                            .size(TypeScale::DATA)
                            .color(palette.secondary),
                    )
                    .truncate()
                    .sense(egui::Sense::click()),
                )
                .clicked()
            })
            .inner;
    });
    row.col(|ui| {
        paint_row_cell(ui, false, false);
        ui.add_space(Space::S);
        let resp = ui
            .add(
                egui::Label::new(
                    RichText::new(entry.playlist_url.path())
                        .monospace()
                        .size(TypeScale::DATA)
                        .color(palette.secondary),
                )
                .truncate()
                .sense(egui::Sense::click()),
            )
            .on_hover_text(entry.playlist_url.as_str());
        cell_clicked |= resp.clicked();
    });
    row.col(|ui| {
        paint_row_cell(ui, false, true);
        ui.add_space(Space::S);
        let (text, color) = variant_status_cell(entry, palette, lang);
        let resp = ui.add(
            egui::Label::new(
                RichText::new(text)
                    .monospace()
                    .size(TypeScale::DATA)
                    .color(color),
            )
            .truncate()
            .sense(egui::Sense::click()),
        );
        cell_clicked |= resp.clicked();
    });
    cell_clicked
}

const fn variant_status_cell(
    entry: &HlsVariantEntry,
    palette: &Palette,
    lang: Language,
) -> (&'static str, egui::Color32) {
    match &entry.state {
        VariantMediaState::NotCaptured => (lang.pick("Not captured", "미캡처"), palette.secondary),
        VariantMediaState::Failed(_) => (lang.pick("Parse error", "해석 오류"), palette.error),
        VariantMediaState::Ready(plan) => match &plan.blocked {
            None => (lang.pick("Ready", "준비됨"), palette.text),
            Some(HlsBlockedReason::MissingChromeSegmentRequest) => (
                lang.pick("Play in Chrome", "Chrome 재생 필요"),
                palette.secondary,
            ),
            Some(HlsBlockedReason::Unsupported(_)) => {
                (lang.pick("Unsupported", "미지원"), palette.secondary)
            }
            Some(HlsBlockedReason::NoSegments) => {
                (lang.pick("No segments", "세그먼트 없음"), palette.secondary)
            }
            Some(HlsBlockedReason::PlaylistNotCaptured) => {
                (lang.pick("Not captured", "미캡처"), palette.secondary)
            }
            Some(HlsBlockedReason::PlaylistParseFailed(_)) => {
                (lang.pick("Parse error", "해석 오류"), palette.error)
            }
        },
    }
}

fn segments_summary_text(
    media: &MediaDownloadPlan,
    live_statuses: Option<&[SegmentStatus]>,
    is_downloading: bool,
    lang: Language,
) -> Option<String> {
    if is_downloading {
        return None;
    }
    let Some(statuses) = live_statuses else {
        let total = media.segments.len();
        let cached = media.cached_count;
        let missing = media.missing_count;
        return Some(match lang {
            Language::English => format!("{total} total · {cached} cached · {missing} missing"),
            Language::Korean => format!("전체 {total} · 캐시 {cached} · 누락 {missing}"),
        });
    };

    let mut cached = 0usize;
    let mut fetched = 0usize;
    let mut failed = 0usize;
    let mut cancelled = 0usize;
    let mut pending = 0usize;
    for status in statuses {
        match status {
            SegmentStatus::Cached => cached += 1,
            SegmentStatus::Done => fetched += 1,
            SegmentStatus::Failed { .. } => failed += 1,
            SegmentStatus::Cancelled => cancelled += 1,
            SegmentStatus::Queued | SegmentStatus::Downloading => pending += 1,
        }
    }

    let mut parts = vec![match lang {
        Language::English => format!("{} total", statuses.len()),
        Language::Korean => format!("전체 {}", statuses.len()),
    }];
    if cached > 0 {
        parts.push(match lang {
            Language::English => format!("{cached} cached"),
            Language::Korean => format!("캐시 {cached}"),
        });
    }
    if fetched > 0 {
        parts.push(match lang {
            Language::English => format!("{fetched} fetched"),
            Language::Korean => format!("수신 {fetched}"),
        });
    }
    if failed > 0 {
        parts.push(match lang {
            Language::English => format!("{failed} failed"),
            Language::Korean => format!("실패 {failed}"),
        });
    }
    if cancelled > 0 {
        parts.push(match lang {
            Language::English => format!("{cancelled} cancelled"),
            Language::Korean => format!("취소 {cancelled}"),
        });
    }
    if pending > 0 {
        parts.push(match lang {
            Language::English => format!("{pending} remaining"),
            Language::Korean => format!("대기 {pending}"),
        });
    }
    Some(parts.join(" · "))
}

fn render_segments_section(
    ui: &mut egui::Ui,
    media: &MediaDownloadPlan,
    live_statuses: Option<&[SegmentStatus]>,
    is_downloading: bool,
    lang: Language,
) {
    let palette = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(lang.pick("Segments", "세그먼트"))
                .size(TypeScale::PANEL_TITLE)
                .color(palette.secondary),
        );
        if let Some(summary) = segments_summary_text(media, live_statuses, is_downloading, lang) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(summary)
                        .monospace()
                        .size(TypeScale::STATUS)
                        .color(palette.secondary),
                );
            });
        }
    });

    let cols = Layout::HLS_SEGMENT_COLUMNS;
    let visible_rows = u8::try_from(media.segments.len().clamp(1, 8)).unwrap_or(8);
    let needs_scroll = media.segments.len() > 8;
    let body_height =
        (f32::from(visible_rows) * Layout::TABLE_ROW_HEIGHT).min(Layout::HLS_SEGMENTS_HEIGHT);

    ui.push_id("hls-segments-table", |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        TableBuilder::new(ui)
            .striped(false)
            .vscroll(needs_scroll)
            .auto_shrink([false, false])
            .min_scrolled_height(body_height)
            .max_scroll_height(body_height)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(cols.index))
            .column(Column::remainder().at_least(cols.name_min))
            .column(Column::exact(cols.duration))
            .column(Column::exact(cols.status))
            .header(Layout::TABLE_HEADER_HEIGHT, |mut header| {
                header_col(&mut header, "#", true, Space::MD, false);
                header_col(
                    &mut header,
                    lang.pick("Segment", "세그먼트"),
                    false,
                    Space::S,
                    false,
                );
                header_col(
                    &mut header,
                    lang.pick("Duration", "길이"),
                    true,
                    Space::MD,
                    false,
                );
                header_col(
                    &mut header,
                    lang.text(Text::ColStatus),
                    false,
                    Space::S,
                    true,
                );
            })
            .body(|body| {
                body.rows(Layout::TABLE_ROW_HEIGHT, media.segments.len(), |mut row| {
                    let idx = row.index();
                    let Some(segment) = media.segments.get(idx) else {
                        return;
                    };
                    let live = live_statuses.and_then(|slice| slice.get(idx));
                    render_segment_row(&mut row, idx + 1, segment, live, &palette, lang);
                });
            });
    });
}

fn render_segment_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    number: usize,
    plan: &SegmentPlan,
    live_status: Option<&SegmentStatus>,
    palette: &Palette,
    lang: Language,
) {
    row.col(|ui| {
        paint_row_cell(ui, false, false);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(Space::MD);
            ui.label(
                RichText::new(number.to_string())
                    .monospace()
                    .size(TypeScale::DATA)
                    .color(palette.secondary),
            );
        });
    });
    row.col(|ui| {
        paint_row_cell(ui, false, false);
        ui.add_space(Space::S);
        let name = plan
            .segment
            .url
            .last_path_segment()
            .unwrap_or(plan.segment.raw_uri.as_str());
        ui.add(egui::Label::new(RichText::new(name).monospace().size(TypeScale::DATA)).truncate())
            .on_hover_text(plan.segment.url.as_str());
    });
    row.col(|ui| {
        paint_row_cell(ui, false, false);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(Space::MD);
            let duration = format!("{:.1}s", plan.segment.duration_secs);
            ui.label(
                RichText::new(duration)
                    .monospace()
                    .size(TypeScale::DATA)
                    .color(palette.secondary),
            );
        });
    });
    row.col(|ui| {
        paint_row_cell(ui, false, true);
        ui.add_space(Space::S);
        let (label, color, tooltip) = segment_status_cell(plan, live_status, palette, lang);
        let resp = ui.add(
            egui::Label::new(
                RichText::new(label)
                    .monospace()
                    .size(TypeScale::DATA)
                    .color(color),
            )
            .truncate(),
        );
        if let Some(tip) = tooltip {
            resp.on_hover_text(tip);
        }
    });
}

fn segment_status_cell(
    plan: &SegmentPlan,
    live: Option<&SegmentStatus>,
    palette: &Palette,
    lang: Language,
) -> (String, egui::Color32, Option<String>) {
    match live {
        Some(SegmentStatus::Cached) => {
            (lang.pick("Cached", "캐시됨").to_owned(), palette.text, None)
        }
        Some(SegmentStatus::Queued) => (
            lang.pick("Queued", "대기 중").to_owned(),
            palette.secondary,
            None,
        ),
        Some(SegmentStatus::Downloading) => (
            lang.pick("Downloading", "받는 중").to_owned(),
            palette.text,
            None,
        ),
        Some(SegmentStatus::Done) => (lang.pick("Done", "완료").to_owned(), palette.text, None),
        Some(SegmentStatus::Cancelled) => (
            lang.pick("Cancelled", "취소됨").to_owned(),
            palette.warning,
            None,
        ),
        Some(SegmentStatus::Failed { status, message }) => {
            let (label, color) = match status {
                Some(code) if *code >= 500 => (format!("HTTP {code}"), palette.error),
                Some(code) => (format!("HTTP {code}"), palette.warning),
                None => (lang.pick("Failed", "실패").to_owned(), palette.error),
            };
            (label, color, Some(message.clone()))
        }
        None => {
            if plan.is_cached() {
                (lang.pick("Cached", "캐시됨").to_owned(), palette.text, None)
            } else {
                (
                    lang.pick("Missing", "누락").to_owned(),
                    palette.secondary,
                    None,
                )
            }
        }
    }
}

fn render_progress_row(ui: &mut egui::Ui, progress: &HlsDownloadProgress, lang: Language) {
    let fraction = if progress.total_segments == 0 {
        0.0
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "segment counts in a single HLS playlist fit comfortably in f32 precision"
        )]
        {
            (progress.completed_segments as f32 / progress.total_segments as f32).clamp(0.0, 1.0)
        }
    };
    let phase_label = match progress.phase {
        HlsDownloadPhase::Downloading => lang.pick("Downloading", "다운로드 중"),
        HlsDownloadPhase::Merging => lang.pick("Merging", "병합 중"),
        HlsDownloadPhase::Finished => lang.pick("Finishing", "마무리 중"),
    };
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    ui.horizontal_wrapped(|ui| {
        ui.spinner();
        let radius = egui::CornerRadius::same(Radius::MICRO);
        let resp = ui
            .scope(|ui| {
                ui.visuals_mut().extreme_bg_color = tokens.surfaces.chrome;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(Layout::HLS_PROGRESS_WIDTH)
                        .fill(tokens.accent.primary)
                        .corner_radius(radius),
                )
            })
            .inner;
        ui.painter().rect_stroke(
            resp.rect,
            radius,
            egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.control),
            egui::StrokeKind::Inside,
        );
        let detail = format!(
            "{phase_label} {}/{} · {}",
            progress.completed_segments,
            progress.total_segments,
            ByteSize::new(progress.downloaded_bytes),
        );
        ui.label(RichText::new(detail).monospace().size(TypeScale::STATUS));
    });
}

fn render_outcome_line(ui: &mut egui::Ui, outcome: &HlsDownloadOutcome, lang: Language) {
    let palette = Palette::of(ui);
    match outcome {
        HlsDownloadOutcome::Completed(summary) => {
            let file_name = summary
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output.ts");
            let size = ByteSize::new(summary.total_bytes);
            let text = match lang {
                Language::English => format!("Saved {file_name} ({size})"),
                Language::Korean => format!("{file_name} 저장됨 ({size})"),
            };
            ui.add(egui::Label::new(RichText::new(text).color(palette.text)).wrap())
                .on_hover_text(summary.path.display().to_string());
        }
        HlsDownloadOutcome::Cancelled => {
            ui.add(
                egui::Label::new(
                    RichText::new(lang.pick(
                        "Download cancelled. No file was saved.",
                        "다운로드가 취소되어 파일이 저장되지 않았습니다.",
                    ))
                    .color(palette.warning),
                )
                .wrap(),
            );
        }
        HlsDownloadOutcome::Failed(error) => {
            let color = match error {
                HlsDownloadError::HttpStatus { status, .. } if *status < 500 => palette.warning,
                HlsDownloadError::NotReady(_) => palette.warning,
                _ => palette.error,
            };
            ui.add(
                egui::Label::new(
                    RichText::new(domain::hls_download_error(lang, error)).color(color),
                )
                .wrap(),
            );
        }
    }
}

fn header_col(
    header: &mut egui_extras::TableRow<'_, '_>,
    title: &str,
    numeric: bool,
    pad: f32,
    last: bool,
) {
    header.col(|ui| {
        paint_header_cell(ui, last);
        let secondary = ThemeTokens::for_theme(ui.ctx().theme()).text.secondary;
        let text = RichText::new(title)
            .size(TypeScale::COLUMN_HEADER)
            .color(secondary);
        if numeric {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if pad > 0.0 {
                    ui.add_space(pad);
                }
                ui.label(text);
            });
        } else {
            if pad > 0.0 {
                ui.add_space(pad);
            }
            ui.label(text);
        }
    });
}

fn cell_band(ui: &egui::Ui, last: bool) -> egui::Rect {
    let mut band = ui.max_rect();
    if last {
        band.max.x = band.max.x.max(ui.clip_rect().right());
    }
    band
}

fn paint_header_cell(ui: &egui::Ui, last: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let band = cell_band(ui, last);
    let painter = ui.painter();
    painter.rect_filled(band, Radius::PANE, tokens.surfaces.chrome);
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.structural),
    );
}

fn paint_row_cell(ui: &egui::Ui, selection_bar: bool, last: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let band = cell_band(ui, last);
    let painter = ui.painter();
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.subtle),
    );
    if selection_bar {
        let cell = ui.max_rect();
        let bar = egui::Rect::from_min_size(
            cell.left_top(),
            egui::vec2(Layout::SELECTION_BAR_WIDTH, cell.height()),
        );
        painter.rect_filled(bar, Radius::PANE, tokens.accent.primary);
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderName, HeaderValue, Method, StatusCode};
    use http_inspector_core::hls::HlsDownloadSummary;
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture, SessionId};

    use super::*;

    const CHROME_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    fn fixture_exchange(
        session: SessionId,
        url: &str,
        ua: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<Exchange, Box<dyn std::error::Error>> {
        let builder = ExchangeBuilder::http(Method::GET, url)?
            .session(session)
            .header(
                HeaderName::from_static("user-agent"),
                HeaderValue::from_str(ua)?,
            );
        let resp = ResponseFixture::new(StatusCode::OK)
            .header(
                HeaderName::from_static("content-type"),
                HeaderValue::from_str(content_type)?,
            )
            .body(body.to_vec(), BodyLifecycle::Complete);
        Ok(builder.respond(resp)?)
    }

    fn make_dialog_with_plan(
        records: &[Exchange],
        playlist: &Exchange,
    ) -> Result<HlsDialog, Box<dyn std::error::Error>> {
        let plan = inspect_hls_session(playlist, records, None)?;
        let default_variant = plan.default_variant;
        Ok(HlsDialog {
            source_record_id: Some(playlist.id()),
            inspecting: None,
            plan: Some(plan),
            inspect_error: None,
            selected_variant: default_variant,
            download: None,
            last_result: None,
        })
    }

    fn run_frame(ctx: &egui::Context, dialog: &HlsDialog) {
        for lang in [Language::English, Language::Korean] {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = dialog.show(ui.ctx(), true, lang);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn renders_blocked_ready_failed_and_cancelled_states_without_panicking()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let master = fixture_exchange(
            session,
            "http://cdn.test/live/master.m3u8",
            CHROME_UA,
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720\n720p/index.m3u8\n",
        )?;
        let media = fixture_exchange(
            session,
            "http://cdn.test/live/720p/index.m3u8",
            CHROME_UA,
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n",
        )?;
        let seg0 = fixture_exchange(
            session,
            "http://cdn.test/live/720p/seg0.ts",
            CHROME_UA,
            "video/mp2t",
            b"TS0",
        )?;

        let ctx = egui::Context::default();

        let blocked = make_dialog_with_plan(&[master.clone(), media.clone()], &master)?;
        assert!(
            !blocked
                .selected_entry()
                .is_some_and(HlsVariantEntry::can_download)
        );
        run_frame(&ctx, &blocked);

        let mut ready = make_dialog_with_plan(&[master.clone(), media, seg0], &master)?;
        assert!(
            ready
                .selected_entry()
                .is_some_and(HlsVariantEntry::can_download)
        );
        run_frame(&ctx, &ready);

        ready.last_result = Some(FinishedDownload {
            variant_index: 0,
            outcome: HlsDownloadOutcome::Failed(HlsDownloadError::HttpStatus {
                segment_index: 1,
                segment_number: 2,
                segment_name: "seg1.ts".to_owned(),
                segment_url: "http://cdn.test/live/720p/seg1.ts".to_owned(),
                status: 403,
                reason: "Forbidden".to_owned(),
            }),
            segment_statuses: vec![
                SegmentStatus::Cached,
                SegmentStatus::Failed {
                    status: Some(403),
                    message: "HTTP 403 Forbidden".to_owned(),
                },
            ],
        });
        run_frame(&ctx, &ready);

        ready.last_result = Some(FinishedDownload {
            variant_index: 0,
            outcome: HlsDownloadOutcome::Cancelled,
            segment_statuses: vec![SegmentStatus::Cached, SegmentStatus::Cancelled],
        });
        run_frame(&ctx, &ready);

        ready.last_result = Some(FinishedDownload {
            variant_index: 0,
            outcome: HlsDownloadOutcome::Completed(HlsDownloadSummary {
                path: PathBuf::from("/tmp/master-720p.ts"),
                total_bytes: 12_345,
                total_segments: 2,
                reused_segments: 1,
                downloaded_segments: 1,
            }),
            segment_statuses: vec![SegmentStatus::Cached, SegmentStatus::Done],
        });
        run_frame(&ctx, &ready);
        Ok(())
    }

    #[test]
    fn escape_reports_close_action() -> Result<(), Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let media = fixture_exchange(
            session,
            "http://cdn.test/index.m3u8",
            CHROME_UA,
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXT-X-ENDLIST\n",
        )?;
        let dialog = make_dialog_with_plan(std::slice::from_ref(&media), &media)?;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..egui::RawInput::default()
        };
        let mut action = None;
        let mut out = ctx.run_ui(input, |ui| {
            action = dialog.show(ui.ctx(), true, Language::English);
        });
        out.textures_delta.clear();
        assert_eq!(action, Some(HlsAction::Close));
        Ok(())
    }

    #[test]
    fn refresh_clears_previous_download_result() -> Result<(), Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let media = fixture_exchange(
            session,
            "http://cdn.test/live/index.m3u8",
            CHROME_UA,
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n",
        )?;
        let seg0 = fixture_exchange(
            session,
            "http://cdn.test/live/seg0.ts",
            CHROME_UA,
            "video/mp2t",
            &[0x47; 188],
        )?;
        let seg1 = fixture_exchange(
            session,
            "http://cdn.test/live/seg1.ts",
            CHROME_UA,
            "video/mp2t",
            &[0x47; 188],
        )?;

        let mut dialog = make_dialog_with_plan(&[media.clone(), seg0.clone()], &media)?;
        dialog.last_result = Some(FinishedDownload {
            variant_index: 0,
            outcome: HlsDownloadOutcome::Failed(HlsDownloadError::HttpStatus {
                segment_index: 1,
                segment_number: 2,
                segment_name: "seg1.ts".to_owned(),
                segment_url: "http://cdn.test/live/seg1.ts".to_owned(),
                status: 403,
                reason: "Forbidden".to_owned(),
            }),
            segment_statuses: vec![
                SegmentStatus::Cached,
                SegmentStatus::Failed {
                    status: Some(403),
                    message: "HTTP 403 Forbidden".to_owned(),
                },
            ],
        });
        assert!(dialog.segment_statuses_for_selected().is_some());

        // Simulate pressing Refresh on the same playlist after capturing seg1.ts.
        let ctx = egui::Context::default();
        dialog.open(media.clone(), vec![media, seg0, seg1], None, &ctx);
        assert!(
            dialog.last_result.is_none(),
            "Refresh must immediately clear stale download results"
        );
        assert!(dialog.segment_statuses_for_selected().is_none());

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while dialog.inspecting.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "HLS refresh inspection timed out"
            );
            dialog.poll(&ctx);
            std::thread::yield_now();
        }

        assert!(dialog.last_result.is_none());
        assert!(dialog.segment_statuses_for_selected().is_none());
        let refreshed_media = dialog
            .selected_entry()
            .and_then(HlsVariantEntry::media)
            .ok_or("missing refreshed media plan")?;
        assert_eq!(refreshed_media.cached_count, 2);
        assert_eq!(refreshed_media.missing_count, 0);
        run_frame(&ctx, &dialog);
        Ok(())
    }

    fn sample_media_plan() -> Result<MediaDownloadPlan, Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let media = fixture_exchange(
            session,
            "http://cdn.test/live/index.m3u8",
            CHROME_UA,
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n",
        )?;
        let seg0 = fixture_exchange(
            session,
            "http://cdn.test/live/seg0.ts",
            CHROME_UA,
            "video/mp2t",
            &[0x47; 188],
        )?;
        let plan = inspect_hls_session(&media, &[media.clone(), seg0], None)?;
        plan.variants
            .into_iter()
            .next()
            .and_then(|entry| match entry.state {
                VariantMediaState::Ready(plan) => Some(*plan),
                VariantMediaState::NotCaptured | VariantMediaState::Failed(_) => None,
            })
            .ok_or_else(|| "missing media plan".into())
    }

    #[test]
    fn segments_summary_reflects_live_and_final_states() -> Result<(), Box<dyn std::error::Error>> {
        let media_plan = sample_media_plan()?;

        assert_eq!(
            segments_summary_text(&media_plan, None, false, Language::English).as_deref(),
            Some("3 total · 1 cached · 2 missing")
        );
        assert_eq!(
            segments_summary_text(
                &media_plan,
                Some(&[
                    SegmentStatus::Cached,
                    SegmentStatus::Downloading,
                    SegmentStatus::Downloading,
                ]),
                true,
                Language::English,
            ),
            None
        );
        assert_eq!(
            segments_summary_text(
                &media_plan,
                Some(&[
                    SegmentStatus::Cached,
                    SegmentStatus::Done,
                    SegmentStatus::Done,
                ]),
                false,
                Language::English,
            )
            .as_deref(),
            Some("3 total · 1 cached · 2 fetched")
        );
        assert_eq!(
            segments_summary_text(
                &media_plan,
                Some(&[
                    SegmentStatus::Cached,
                    SegmentStatus::Failed {
                        status: Some(403),
                        message: "HTTP 403 Forbidden".to_owned(),
                    },
                    SegmentStatus::Done,
                ]),
                false,
                Language::English,
            )
            .as_deref(),
            Some("3 total · 1 cached · 1 fetched · 1 failed")
        );
        assert_eq!(
            segments_summary_text(
                &media_plan,
                Some(&[
                    SegmentStatus::Cached,
                    SegmentStatus::Cancelled,
                    SegmentStatus::Cancelled,
                ]),
                false,
                Language::English,
            )
            .as_deref(),
            Some("3 total · 1 cached · 2 cancelled")
        );
        Ok(())
    }

    #[test]
    fn segments_summary_localizes_in_korean() -> Result<(), Box<dyn std::error::Error>> {
        let media_plan = sample_media_plan()?;

        assert_eq!(
            segments_summary_text(&media_plan, None, false, Language::Korean).as_deref(),
            Some("전체 3 · 캐시 1 · 누락 2")
        );
        assert_eq!(
            segments_summary_text(
                &media_plan,
                Some(&[
                    SegmentStatus::Cached,
                    SegmentStatus::Failed {
                        status: Some(403),
                        message: "HTTP 403 Forbidden".to_owned(),
                    },
                    SegmentStatus::Done,
                ]),
                false,
                Language::Korean,
            )
            .as_deref(),
            Some("전체 3 · 캐시 1 · 수신 1 · 실패 1")
        );
        Ok(())
    }
}
