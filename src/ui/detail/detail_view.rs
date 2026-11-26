//! Selected request details and body previews.

use crate::ui::AppAction;
use crate::ui::detail::body_cache::{BodySource, Capture};
use crate::ui::detail::body_render::BodyViewCache;
use crate::ui::detail::detail_tabs::{self, DetailTab};
use crate::ui::detail::image_view;
use crate::ui::detail::search::Query;
use crate::ui::detail::search::highlight;
use crate::ui::dialogs::scope_help;
use crate::ui::dialogs::{body_saving::BodySaving, copy_curl::CurlCopy};
use crate::ui::i18n::{Language, domain};
use crate::ui::requests::outcome_view;
use crate::ui::style::theme::MatchColors;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Layout;
use eframe::egui;
use http_inspector_content::detect;
use http_inspector_content::format::{BodyFormat, ViewMode, form};
use http_inspector_exchange::http_semantics::urlencoded;
use http_inspector_exchange::{
    BodyLifecycle, BodyPreview, BodyRetention, BodySide, BodySnapshot, Exchange, HeaderPreview,
    ImageFormat, MediaType, Outcome, RecordId, TunnelPreview,
};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct DetailWidgets {
    body: BodyViewCache,
    image: image_view::ImagePanel,
}

pub(crate) struct DetailView<'a> {
    pub(crate) record: Option<&'a Exchange>,
    pub(crate) search: Option<&'a Query>,
    pub(crate) tab: DetailTab,
    pub(crate) body_mode: ViewMode,
    pub(crate) curl: &'a CurlCopy,
    pub(crate) body_saving: &'a BodySaving,
    pub(crate) lang: Language,
}

impl DetailView<'_> {
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        widgets: &mut DetailWidgets,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.lang;
        let Some(record) = self.record else {
            ui.centered_and_justified(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            lang.pick("No request selected", "선택된 요청이 없습니다"),
                        )
                        .weak(),
                    )
                    .wrap(),
                );
            });
            return;
        };
        let mut tab = self.tab;
        if shortcuts_allowed {
            detail_tabs::apply_shortcuts(ui, &mut tab);
        }
        let request = record.request();
        let search = self.search;
        render_title(ui, record, search);
        detail_actions(ui, record, self.curl, lang, actions);
        detail_tabs::show(ui, &mut tab, lang);
        if tab != self.tab {
            actions.push(AppAction::DetailTab(tab));
        }
        let mut body_mode = self.body_mode;

        let copied = egui::ScrollArea::vertical()
            .id_salt(("details-scroll", tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_timing(ui, record, lang);
                ui.separator();
                let mut body = BodyPanel {
                    record: record.id(),
                    mode: self.body_mode,
                    cache: &mut widgets.body,
                    copied: None,
                    search,
                    lang,
                };
                match tab {
                    DetailTab::Headers => {
                        render_query(ui, request.target(), lang);
                        ui.strong(lang.pick("Request headers", "요청 헤더"));
                        render_headers(
                            ui,
                            "request-headers",
                            request.headers(),
                            body.headers_search(),
                            lang,
                        );
                        ui.separator();
                        render_outcome(ui, record, &mut body, None);
                    }
                    DetailTab::Body => render_body_tab(
                        ui,
                        record,
                        &mut body,
                        &mut body_mode,
                        &mut widgets.image,
                        |ui| actions.extend(self.body_saving.menus(ui, record, lang)),
                    ),
                }
                body.copied
            })
            .inner;
        actions.extend(copied.map(AppAction::CopyText));
        if body_mode != self.body_mode {
            actions.push(AppAction::BodyMode(body_mode));
        }
    }
}

fn detail_actions(
    ui: &mut egui::Ui,
    record: &Exchange,
    curl: &CurlCopy,
    lang: Language,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal_wrapped(|ui| {
        let (status, tone) = outcome_view::outcome_text(record.outcome(), lang);
        ui.colored_label(Palette::of(ui).status(tone), status);
        if ui
            .add_enabled(
                record.source() != http_inspector_exchange::CaptureSource::Connect,
                egui::Button::new(lang.pick("Edit and Resend", "편집 후 재전송")),
            )
            .clicked()
        {
            actions.push(AppAction::OpenReplay(record.id()));
        }
        curl.controls(ui, record, lang, actions);
        if let Some(original) = record.replay_of()
            && ui.link(replay_link(original, lang)).clicked()
        {
            actions.push(AppAction::SelectRecord(original));
        }
    });
}

/// Where and how the bodies of the selected exchange are shown.
struct BodyPanel<'a> {
    record: RecordId,
    mode: ViewMode,
    cache: &'a mut BodyViewCache,
    copied: Option<String>,
    /// The active search, whose matches are marked.
    search: Option<&'a Query>,
    lang: Language,
}

impl BodyPanel<'_> {
    fn headers_search(&self) -> Option<&Query> {
        self.search.filter(|query| query.scope.covers_headers())
    }
}

/// The method and URL, with URL matches of the active search marked.
fn render_title(ui: &mut egui::Ui, record: &Exchange, search: Option<&Query>) {
    let request = record.request();
    let title = format!("{} {}", request.method(), request.target());
    let label = match search.filter(|query| query.scope.covers_url()) {
        Some(query) => {
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            let color = Palette::of(ui).text;
            let colors = MatchColors::for_theme(ui.ctx().theme());
            let mut job = highlight::plain(&title, &query.matcher, font, color, colors);
            job.wrap.max_width = ui.available_width();
            egui::Label::new(job)
        }
        None => egui::Label::new(egui::RichText::new(title).strong().monospace()),
    };
    ui.add(label.wrap());
}

/// Whether a body of `media_type` may be shown as a tree: its `Content-Type`
/// names JSON or XML, or does not name a format and the body may sniff as JSON.
fn has_tree(media_type: Option<&MediaType>) -> bool {
    detect::from_content_type(media_type)
        .is_none_or(|detection| matches!(detection.format, BodyFormat::Json | BodyFormat::Xml))
}

/// Shows Tree only as an enabled option when either body may be JSON or XML.
fn view_mode_bar(
    ui: &mut egui::Ui,
    mode: &mut ViewMode,
    has_tree: bool,
    lang: Language,
) -> ViewMode {
    if !has_tree && *mode == ViewMode::Tree {
        *mode = ViewMode::Pretty;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(lang.pick("View:", "보기:"));
        for option in ViewMode::ALL {
            ui.add_enabled_ui(option != ViewMode::Tree || has_tree, |ui| {
                ui.selectable_value(mode, option, domain::view_mode(lang, option))
            })
            .inner
            .on_disabled_hover_text(lang.pick(
                "Tree requires a JSON or XML request or response body",
                "트리 보기는 JSON 또는 XML 요청·응답 본문에서만 사용할 수 있습니다",
            ));
        }
    });
    *mode
}

/// Column width bounds for two-column key/value grids inside the detail pane.
#[derive(Clone, Copy)]
struct GridBounds {
    available: f32,
    right: f32,
    key_max: f32,
}

impl GridBounds {
    fn from_ui(ui: &egui::Ui) -> Self {
        let available = ui.available_width().max(Layout::DETAIL_ROW_MIN_WIDTH);
        let right = ui.cursor().left() + available;
        let key_max =
            Layout::DETAIL_KEY_WIDTH.fit((available - ui.spacing().item_spacing.x) * 0.42);
        Self {
            available,
            right,
            key_max,
        }
    }

    fn value_max(self, ui: &egui::Ui) -> f32 {
        (self.right - ui.cursor().left()).max(48.0)
    }
}

fn wrapped_cell(ui: &mut egui::Ui, max_width: f32, text: impl Into<egui::WidgetText>) {
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.add(egui::Label::new(text).wrap());
    });
}

fn wrapped_highlight(
    text: &str,
    query: &Query,
    font: egui::FontId,
    color: egui::Color32,
    colors: MatchColors,
    max_width: f32,
) -> egui::text::LayoutJob {
    let mut job = highlight::plain(text, &query.matcher, font, color, colors);
    job.wrap.max_width = max_width;
    job
}

/// Lists decoded query parameters of the request target, if it has any.
fn render_query(ui: &mut egui::Ui, target: &str, lang: Language) {
    let Some(query) = urlencoded::split_target(target).query else {
        return;
    };
    let pairs = form::pairs(query);
    if pairs.is_empty() {
        return;
    }
    ui.strong(lang.pick("Query parameters", "쿼리 매개변수"));
    let key_color = Palette::of(ui).secondary;
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new("query-parameters")
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            for (key, value) in pairs {
                wrapped_cell(
                    ui,
                    bounds.key_max,
                    egui::RichText::new(form::visible(&key))
                        .monospace()
                        .color(key_color),
                );
                let value_max = bounds.value_max(ui);
                wrapped_cell(
                    ui,
                    value_max,
                    egui::RichText::new(form::visible(&value)).monospace(),
                );
                ui.end_row();
            }
        });
    ui.separator();
}

/// Header names in secondary text so that the values, which matter more,
/// stand out; the `:` after each name keeps them apart without color.
fn render_headers(
    ui: &mut egui::Ui,
    id: &str,
    headers: &HeaderPreview,
    search: Option<&Query>,
    lang: Language,
) {
    let name_color = Palette::of(ui).secondary;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = Palette::of(ui).text;
    let colors = MatchColors::for_theme(ui.ctx().theme());
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new(id)
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            for header in headers.entries() {
                let name = format!("{}:", header.name());
                if let Some(query) = search {
                    let name_job = wrapped_highlight(
                        &name,
                        query,
                        font.clone(),
                        name_color,
                        colors,
                        bounds.key_max,
                    );
                    wrapped_cell(ui, bounds.key_max, name_job);
                    let value_max = bounds.value_max(ui);
                    let value_job = wrapped_highlight(
                        header.value(),
                        query,
                        font.clone(),
                        text_color,
                        colors,
                        value_max,
                    );
                    wrapped_cell(ui, value_max, value_job);
                } else {
                    wrapped_cell(
                        ui,
                        bounds.key_max,
                        egui::RichText::new(name).monospace().color(name_color),
                    );
                    let value_max = bounds.value_max(ui);
                    wrapped_cell(
                        ui,
                        value_max,
                        egui::RichText::new(header.value()).monospace(),
                    );
                }
                ui.end_row();
            }
        });
    if headers.entries().is_empty() {
        ui.weak(lang.pick("No headers captured.", "캡처된 헤더가 없습니다."));
    }
    if headers.omitted() {
        ui.colored_label(
            Palette::of(ui).warning,
            lang.pick(
                "Additional headers were omitted.",
                "추가 헤더가 생략되었습니다.",
            ),
        );
    }
}

fn render_body_tab(
    ui: &mut egui::Ui,
    record: &Exchange,
    panel: &mut BodyPanel<'_>,
    mode: &mut ViewMode,
    image: &mut image_view::ImagePanel,
    show_save_menus: impl FnOnce(&mut egui::Ui),
) {
    let lang = panel.lang;
    let request = record.request();
    let mut request_body = request
        .body()
        .map(|body| BodyDetail::new(body, request.headers(), request.media_type(), None));
    let mut response_body = match record.outcome() {
        Outcome::Response(response) => {
            let media_type = response.media_type();
            let image = media_type
                .and_then(ImageFormat::from_media_type)
                .map(|format| (image, format));
            Some(BodyDetail::new(
                response.body(),
                response.headers(),
                media_type,
                image,
            ))
        }
        _ => None,
    };
    let mode_bodies = [request_body.as_ref(), response_body.as_ref()]
        .map(|body| body.filter(|body| body.uses_view_mode()));
    if mode_bodies.iter().any(Option::is_some) {
        let has_tree = mode_bodies
            .into_iter()
            .flatten()
            .any(|body| has_tree(body.media_type));
        panel.mode = view_mode_bar(ui, mode, has_tree, lang);
    }
    show_save_menus(ui);
    ui.strong(lang.pick("Request body", "요청 본문"));
    if let Some(request_body) = request_body.as_mut() {
        render_body(ui, request_body, panel, BodySide::Request);
    } else {
        ui.weak(lang.pick("Body was not observed.", "본문이 관측되지 않았습니다."));
    }
    ui.separator();
    render_outcome(ui, record, panel, response_body.as_mut());
}

struct BodyDetail<'a> {
    snapshot: Option<BodySnapshot>,
    headers: &'a HeaderPreview,
    media_type: Option<&'a MediaType>,
    image: Option<(&'a mut image_view::ImagePanel, ImageFormat)>,
}

impl<'a> BodyDetail<'a> {
    fn new(
        body: &BodyPreview,
        headers: &'a HeaderPreview,
        media_type: Option<&'a MediaType>,
        image: Option<(&'a mut image_view::ImagePanel, ImageFormat)>,
    ) -> Self {
        Self {
            snapshot: body.snapshot(),
            headers,
            media_type,
            image,
        }
    }

    fn uses_view_mode(&self) -> bool {
        self.image.is_none()
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.retention() != BodyRetention::Omitted && !snapshot.bytes().is_empty()
            })
    }
}

fn render_body(
    ui: &mut egui::Ui,
    body: &mut BodyDetail<'_>,
    panel: &mut BodyPanel<'_>,
    side: BodySide,
) {
    let lang = panel.lang;
    let Some(snapshot) = body.snapshot.as_ref() else {
        ui.weak(lang.pick("Preview is updating.", "미리보기를 업데이트하는 중입니다."));
        return;
    };
    let summary = match lang {
        Language::English => format!(
            "{} bytes observed · {} · {}",
            snapshot.observed_bytes(),
            lifecycle_text(snapshot.lifecycle(), lang),
            retention_text(snapshot, lang)
        ),
        Language::Korean => format!(
            "{}바이트 관측 · {} · {}",
            snapshot.observed_bytes(),
            lifecycle_text(snapshot.lifecycle(), lang),
            retention_text(snapshot, lang)
        ),
    };
    ui.label(summary);
    if snapshot.retention() == BodyRetention::Omitted {
        ui.colored_label(
            Palette::of(ui).warning,
            lang.pick(
                "Preview was omitted to keep forwarding nonblocking.",
                "전달 지연을 막기 위해 미리보기가 생략되었습니다.",
            ),
        );
        return;
    }
    if snapshot.is_complete() && snapshot.bytes().is_empty() {
        ui.weak(lang.pick("Empty body.", "빈 본문입니다."));
        return;
    }
    let encodings = body.headers.get("content-encoding").collect::<Vec<_>>();
    if let Some((image, format)) = body.image.as_mut() {
        image.show(
            ui,
            &super::image_view::ImageBody {
                record: panel.record,
                snapshot,
                encodings: &encodings,
                declared: *format,
            },
            lang,
        );
        return;
    }
    let source = BodySource {
        record: panel.record.value(),
        side,
        bytes: snapshot.bytes(),
        encodings: &encodings,
        media_type: body.media_type,
        capture: Capture::of(snapshot),
        search: panel.search.filter(|query| query.scope.covers_body(side)),
    };
    if let Some(text) = panel.cache.show(ui, &source, panel.mode, lang) {
        panel.copied = Some(text);
    }
}

const fn lifecycle_text(lifecycle: BodyLifecycle, lang: Language) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => lang.pick("streaming", "스트리밍 중"),
        BodyLifecycle::Complete => lang.pick("complete", "완료"),
        BodyLifecycle::Interrupted => lang.pick("interrupted by the sender", "송신 측에서 중단됨"),
        BodyLifecycle::Cancelled => lang.pick("cancelled by the receiver", "수신 측에서 취소됨"),
    }
}

fn retention_text(snapshot: &BodySnapshot, lang: Language) -> &'static str {
    match snapshot.retention() {
        BodyRetention::Prefix if snapshot.is_complete() => {
            lang.pick("full preview", "전체 미리보기")
        }
        BodyRetention::Prefix => lang.pick("partial preview", "일부 미리보기"),
        BodyRetention::Truncated => lang.pick("truncated preview", "잘린 미리보기"),
        BodyRetention::Omitted => lang.pick("preview omitted", "미리보기 생략"),
    }
}

fn render_outcome(
    ui: &mut egui::Ui,
    record: &Exchange,
    body: &mut BodyPanel<'_>,
    response_body: Option<&mut BodyDetail<'_>>,
) {
    let lang = body.lang;
    match record.outcome() {
        Outcome::Response(response) => match response_body {
            None => {
                let heading = match lang {
                    Language::English => format!(
                        "Response headers · {} {} ({}) · {:?}",
                        response.status().as_str(),
                        response.reason(),
                        outcome_view::status_class(response.status().as_u16(), lang),
                        response.version()
                    ),
                    Language::Korean => format!(
                        "응답 헤더 · {} {} ({}) · {:?}",
                        response.status().as_str(),
                        response.reason(),
                        outcome_view::status_class(response.status().as_u16(), lang),
                        response.version()
                    ),
                };
                ui.strong(heading);
                let search = body.headers_search();
                render_headers(ui, "response-headers", response.headers(), search, lang);
            }
            Some(response_body) => {
                ui.strong(lang.pick("Response body", "응답 본문"));
                render_body(ui, response_body, body, BodySide::Response);
            }
        },
        Outcome::Tunnel(tunnel) => {
            ui.label(scope_help::tunnel_note(lang));
            render_tunnel(ui, tunnel, lang);
        }
        outcome => {
            let (text, tone) = outcome_view::detail_text(outcome, lang);
            let color = Palette::of(ui).status(tone);
            if outcome.is_in_flight() {
                ui.weak(text);
            } else if matches!(outcome, Outcome::InspectionEstablished) {
                ui.label(text);
            } else {
                ui.colored_label(color, text);
            }
        }
    }
}

/// Link to the request a replay was sent from. Links keep the text color
/// and are told apart by their underline.
fn replay_link(original: RecordId, lang: Language) -> egui::RichText {
    let label = match lang {
        Language::English => format!("Replay of #{} — show original", original.value()),
        Language::Korean => format!("#{} 재전송 — 원본 보기", original.value()),
    };
    egui::RichText::new(label).underline()
}

fn render_timing(ui: &mut egui::Ui, record: &Exchange, lang: Language) {
    let timing = record.timing();
    let settled_label = outcome_view::timing_label(record.outcome(), lang);
    let settled_value = if record.outcome().is_in_flight() {
        elapsed_text(record)
    } else {
        optional_millis(timing.settled(), lang)
    };
    let total = timing.complete().map_or_else(
        || lang.pick("in progress", "진행 중").to_owned(),
        |duration| format!("{:.1} ms", outcome_view::elapsed_millis(duration)),
    );
    let rows = [
        (
            lang.pick("Connect", "연결"),
            optional_millis(timing.connect(), lang),
        ),
        (
            lang.pick("Send", "전송"),
            optional_millis(timing.send(), lang),
        ),
        (
            lang.pick("Wait (TTFB)", "대기 (TTFB)"),
            optional_millis(timing.wait(), lang),
        ),
        (
            lang.pick("Receive", "수신"),
            optional_millis(timing.receive(), lang),
        ),
        (settled_label, settled_value),
        (lang.pick("Total", "합계"), total),
    ];
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new("timing")
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            for (label, value) in rows {
                wrapped_cell(ui, bounds.key_max, label);
                let value_max = bounds.value_max(ui);
                wrapped_cell(ui, value_max, egui::RichText::new(value).monospace());
                ui.end_row();
            }
        });
}

fn optional_millis(duration: Option<Duration>, lang: Language) -> String {
    duration.map_or_else(
        || lang.pick("not observed", "관측되지 않음").to_owned(),
        |duration| format!("{:.1} ms", outcome_view::elapsed_millis(duration)),
    )
}

fn elapsed_text(record: &Exchange) -> String {
    let millis = outcome_view::elapsed_millis(record.elapsed());
    if record.outcome().is_in_flight() {
        format!("{millis:.0} ms…")
    } else {
        format!("{millis:.1} ms")
    }
}

fn render_tunnel(ui: &mut egui::Ui, tunnel: &TunnelPreview, lang: Language) {
    let snapshot = tunnel.snapshot();
    let state = domain::tunnel_state(lang, snapshot.state());
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new("tunnel-details")
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            wrapped_cell(ui, bounds.key_max, lang.pick("State", "상태"));
            let value_max = bounds.value_max(ui);
            wrapped_cell(ui, value_max, state);
            ui.end_row();
            wrapped_cell(
                ui,
                bounds.key_max,
                lang.pick("Sent to upstream", "업스트림으로 보낸 양"),
            );
            let value_max = bounds.value_max(ui);
            let sent = match lang {
                Language::English => format!("{} bytes", snapshot.client_to_upstream()),
                Language::Korean => format!("{}바이트", snapshot.client_to_upstream()),
            };
            wrapped_cell(ui, value_max, egui::RichText::new(sent).monospace());
            ui.end_row();
            wrapped_cell(
                ui,
                bounds.key_max,
                lang.pick("Received from upstream", "업스트림에서 받은 양"),
            );
            let value_max = bounds.value_max(ui);
            let received = match lang {
                Language::English => format!("{} bytes", snapshot.upstream_to_client()),
                Language::Korean => format!("{}바이트", snapshot.upstream_to_client()),
            };
            wrapped_cell(ui, value_max, egui::RichText::new(received).monospace());
            ui.end_row();
            wrapped_cell(ui, bounds.key_max, lang.pick("Duration", "소요 시간"));
            let value_max = bounds.value_max(ui);
            wrapped_cell(
                ui,
                value_max,
                egui::RichText::new(format!(
                    "{:.1} ms",
                    outcome_view::elapsed_millis(snapshot.duration())
                ))
                .monospace(),
            );
            ui.end_row();
        });
}
