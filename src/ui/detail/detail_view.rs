//! Selected request details and body previews.

use crate::ui::AppAction;
use crate::ui::detail::body_cache::{BodySource, Capture};
use crate::ui::detail::body_render::BodyViewCache;
use crate::ui::detail::detail_tabs::{self, DetailTab};
use crate::ui::detail::image_view;
use crate::ui::detail::search::Query;
use crate::ui::detail::search::highlight;
use crate::ui::dialogs::scope_help;
use crate::ui::dialogs::{body_saving::BodySaving, copy_curl};
use crate::ui::requests::{filter_menu, outcome_view};
use crate::ui::style::theme::MatchColors;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};
use crate::ui::workbench::{chrome, controls};
use eframe::egui;
use http_inspector_analysis::filter::FilterKey;
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
    pub(crate) field_menu_enabled: bool,
    pub(crate) body_saving: &'a BodySaving,
}

impl DetailView<'_> {
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        widgets: &mut DetailWidgets,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) {
        let mut tab = self.tab;
        let Some(record) = self.record else {
            chrome::pane_header(ui, "details-header", |ui| detail_tabs::show(ui, &mut tab));
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new("No request selected").weak()).wrap());
            });
            if tab != self.tab {
                actions.push(AppAction::DetailTab(tab));
            }
            return;
        };
        if shortcuts_allowed {
            detail_tabs::apply_shortcuts(ui, &mut tab);
        }
        let request = record.request();
        let search = self.search;
        header(ui, record, search, &mut tab, actions);
        if tab != self.tab {
            actions.push(AppAction::DetailTab(tab));
        }
        let mut body_mode = self.body_mode;

        let (copied, filter) = egui::ScrollArea::vertical()
            .id_salt(("details-scroll", tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Layout::PANE_ROW_MARGIN)
                    .show(ui, |ui| {
                        render_timing(ui, record);
                        ui.separator();
                        let mut body = BodyPanel {
                            record: record.id(),
                            mode: self.body_mode,
                            cache: &mut widgets.body,
                            copied: None,
                            filter: None,
                            field_menu_enabled: self.field_menu_enabled,
                            search,
                        };
                        match tab {
                            DetailTab::Headers => {
                                render_query(ui, request.target());
                                ui.strong("Request headers");
                                body.filter = render_headers(
                                    ui,
                                    BodySide::Request,
                                    request.headers(),
                                    body.headers_search(),
                                    body.field_menu_enabled,
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
                                |ui| actions.extend(self.body_saving.menus(ui, record)),
                            ),
                        }
                        (body.copied, body.filter)
                    })
                    .inner
            })
            .inner;
        actions.extend(copied.map(AppAction::CopyText));
        actions.extend(filter);
        if body_mode != self.body_mode {
            actions.push(AppAction::BodyMode(body_mode));
        }
    }
}

/// The pane header with the tabs and, when they fit, the record actions,
/// then the method, URL, and outcome of `record` under it.
fn header(
    ui: &mut egui::Ui,
    record: &Exchange,
    search: Option<&Query>,
    tab: &mut DetailTab,
    actions: &mut Vec<AppAction>,
) {
    let actions_in_header = chrome::pane_header(ui, "details-header", |ui| {
        detail_tabs::show(ui, tab);
        let fits = ui.available_width() >= header_actions_width(ui) + Space::LG;
        if fits {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                header_actions(ui, record, actions);
            });
        }
        fits
    });
    let title = egui::Frame::new()
        .inner_margin(Layout::PANE_ROW_MARGIN)
        .show(ui, |ui| {
            render_title(ui, record, search);
            summary_row(ui, record, actions_in_header, actions);
        });
    ui.painter().hline(
        ui.max_rect().x_range(),
        title.response.rect.bottom(),
        egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle),
    );
}

/// Edit and Resend and Copy as cURL, laid out right to left in the pane
/// header, or left to right under the title when the header is too narrow.
fn header_actions(ui: &mut egui::Ui, record: &Exchange, actions: &mut Vec<AppAction>) {
    controls::ghost_style(ui);
    let resend = |ui: &mut egui::Ui, actions: &mut Vec<AppAction>| {
        let button = controls::ghost(RESEND_LABEL, Layout::PANE_BUTTON_HEIGHT);
        if ui
            .add_enabled(
                record.source() != http_inspector_exchange::CaptureSource::Connect,
                button,
            )
            .clicked()
        {
            actions.push(AppAction::OpenReplay(record.id()));
        }
    };
    let copy = |ui: &mut egui::Ui, actions: &mut Vec<AppAction>| {
        if copy_curl::copy_button(ui, record).clicked() {
            actions.push(AppAction::CopyCurl(record.id()));
        }
    };
    if ui.layout().prefer_right_to_left() {
        copy(ui, actions);
        resend(ui, actions);
    } else {
        resend(ui, actions);
        copy(ui, actions);
    }
}

const RESEND_LABEL: &str = "Edit and Resend";

/// Width the header actions need, so the header can tell whether they fit.
fn header_actions_width(ui: &egui::Ui) -> f32 {
    [RESEND_LABEL, copy_curl::COPY_LABEL]
        .iter()
        .map(|label| controls::button_width(ui, label) + ui.spacing().item_spacing.x)
        .sum()
}

/// The outcome and, for a resent request, a link to the original. Holds the
/// header actions too when the header has no room for them.
fn summary_row(
    ui: &mut egui::Ui,
    record: &Exchange,
    actions_in_header: bool,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal_wrapped(|ui| {
        let (status, tone) = outcome_view::outcome_text(record.outcome());
        ui.colored_label(Palette::of(ui).status(tone), status);
        if !actions_in_header {
            header_actions(ui, record, actions);
        }
        if let Some(original) = record.replay_of()
            && ui.link(replay_link(original)).clicked()
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
    filter: Option<AppAction>,
    field_menu_enabled: bool,
    /// The active search, whose matches are marked.
    search: Option<&'a Query>,
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
            let mut job = highlight::plain(&title, &query.terms, font, color, colors);
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
fn view_mode_bar(ui: &mut egui::Ui, mode: &mut ViewMode, has_tree: bool) -> ViewMode {
    if !has_tree && *mode == ViewMode::Tree {
        *mode = ViewMode::Pretty;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("View:");
        for option in ViewMode::ALL {
            ui.add_enabled_ui(option != ViewMode::Tree || has_tree, |ui| {
                ui.selectable_value(mode, option, option.label())
            })
            .inner
            .on_disabled_hover_text("Tree requires a JSON or XML request or response body");
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
    let mut job = highlight::plain(text, &query.terms, font, color, colors);
    job.wrap.max_width = max_width;
    job
}

/// Lists decoded query parameters of the request target, if it has any.
fn render_query(ui: &mut egui::Ui, target: &str) {
    let Some(query) = urlencoded::split_target(target).query else {
        return;
    };
    let pairs = form::pairs(query);
    if pairs.is_empty() {
        return;
    }
    ui.strong("Query parameters");
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
    side: BodySide,
    headers: &HeaderPreview,
    search: Option<&Query>,
    field_menu_enabled: bool,
) -> Option<AppAction> {
    let name_color = Palette::of(ui).secondary;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = Palette::of(ui).text;
    let colors = MatchColors::for_theme(ui.ctx().theme());
    let bounds = GridBounds::from_ui(ui);
    let key = match side {
        BodySide::Request => FilterKey::RequestHeader,
        BodySide::Response => FilterKey::ResponseHeader,
    };
    let mut action = None;
    egui::Grid::new(("headers", side))
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            for header in headers.entries() {
                let name = format!("{}:", header.name());
                let name_label = search.map_or_else(
                    || {
                        egui::WidgetText::from(
                            egui::RichText::new(&name).monospace().color(name_color),
                        )
                    },
                    |query| {
                        wrapped_highlight(
                            &name,
                            query,
                            font.clone(),
                            name_color,
                            colors,
                            bounds.key_max,
                        )
                        .into()
                    },
                );
                let name_response = header_cell(ui, bounds.key_max, name_label);
                let value_max = bounds.value_max(ui);
                let value_label = search.map_or_else(
                    || egui::WidgetText::from(egui::RichText::new(header.value()).monospace()),
                    |query| {
                        wrapped_highlight(
                            header.value(),
                            query,
                            font.clone(),
                            text_color,
                            colors,
                            value_max,
                        )
                        .into()
                    },
                );
                let value_response = header_cell(ui, value_max, value_label);
                name_response.union(value_response).context_menu(|ui| {
                    let value = format!("{}={}", header.name(), header.value());
                    action = filter_menu::items(ui, key, &value, field_menu_enabled);
                });
                ui.end_row();
            }
        });
    if headers.entries().is_empty() {
        ui.weak("No headers captured.");
    }
    if headers.omitted() {
        ui.colored_label(Palette::of(ui).warning, "Additional headers were omitted.");
    }
    action
}

fn header_cell(ui: &mut egui::Ui, max_width: f32, text: egui::WidgetText) -> egui::Response {
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.add(egui::Label::new(text).wrap().sense(egui::Sense::click()))
    })
    .inner
}

fn render_body_tab(
    ui: &mut egui::Ui,
    record: &Exchange,
    panel: &mut BodyPanel<'_>,
    mode: &mut ViewMode,
    image: &mut image_view::ImagePanel,
    show_save_menus: impl FnOnce(&mut egui::Ui),
) {
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
        panel.mode = view_mode_bar(ui, mode, has_tree);
    }
    show_save_menus(ui);
    ui.strong("Request body");
    if let Some(request_body) = request_body.as_mut() {
        render_body(ui, request_body, panel, BodySide::Request);
    } else {
        ui.weak("Body was not observed.");
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
    let Some(snapshot) = body.snapshot.as_ref() else {
        ui.weak("Preview is updating.");
        return;
    };
    ui.label(format!(
        "{} bytes observed · {} · {}",
        snapshot.observed_bytes(),
        lifecycle_text(snapshot.lifecycle()),
        retention_text(snapshot)
    ));
    if snapshot.retention() == BodyRetention::Omitted {
        ui.colored_label(
            Palette::of(ui).warning,
            "Preview was omitted to keep forwarding nonblocking.",
        );
        return;
    }
    if snapshot.is_complete() && snapshot.bytes().is_empty() {
        ui.weak("Empty body.");
        return;
    }
    let encodings = body.headers.get("content-encoding").collect::<Vec<_>>();
    if let Some((image, format)) = body.image.as_mut() {
        image.show(ui, panel.record, snapshot, &encodings, *format);
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
    if let Some(text) = panel.cache.show(ui, &source, panel.mode) {
        panel.copied = Some(text);
    }
}

const fn lifecycle_text(lifecycle: BodyLifecycle) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => "streaming",
        BodyLifecycle::Complete => "complete",
        BodyLifecycle::Interrupted => "interrupted by the sender",
        BodyLifecycle::Cancelled => "cancelled by the receiver",
    }
}

fn retention_text(snapshot: &BodySnapshot) -> &'static str {
    match snapshot.retention() {
        BodyRetention::Prefix if snapshot.is_complete() => "full preview",
        BodyRetention::Prefix => "partial preview",
        BodyRetention::Truncated => "truncated preview",
        BodyRetention::Omitted => "preview omitted",
    }
}

fn render_outcome(
    ui: &mut egui::Ui,
    record: &Exchange,
    body: &mut BodyPanel<'_>,
    response_body: Option<&mut BodyDetail<'_>>,
) {
    match record.outcome() {
        Outcome::Response(response) => match response_body {
            None => {
                ui.strong(format!(
                    "Response headers · {} {} ({}) · {:?}",
                    response.status().as_str(),
                    response.reason(),
                    outcome_view::status_class(response.status().as_u16()),
                    response.version()
                ));
                let search = body.headers_search();
                if let Some(action) = render_headers(
                    ui,
                    BodySide::Response,
                    response.headers(),
                    search,
                    body.field_menu_enabled,
                ) {
                    body.filter = Some(action);
                }
            }
            Some(response_body) => {
                ui.strong("Response body");
                render_body(ui, response_body, body, BodySide::Response);
            }
        },
        Outcome::Tunnel(tunnel) => {
            ui.label(scope_help::TUNNEL_NOTE);
            render_tunnel(ui, tunnel);
        }
        outcome => {
            let (text, tone) = outcome_view::detail_text(outcome);
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
fn replay_link(original: RecordId) -> egui::RichText {
    egui::RichText::new(format!("Replay of #{} — show original", original.value())).underline()
}

fn render_timing(ui: &mut egui::Ui, record: &Exchange) {
    let timing = record.timing();
    let settled_label = outcome_view::timing_label(record.outcome());
    let settled_value = if record.outcome().is_in_flight() {
        elapsed_text(record)
    } else {
        optional_millis(timing.settled())
    };
    let total = timing.complete().map_or_else(
        || "in progress".to_owned(),
        |duration| format!("{:.1} ms", outcome_view::elapsed_millis(duration)),
    );
    let rows = [
        ("Connect", optional_millis(timing.connect())),
        ("Send", optional_millis(timing.send())),
        ("Wait (TTFB)", optional_millis(timing.wait())),
        ("Receive", optional_millis(timing.receive())),
        (settled_label, settled_value),
        ("Total", total),
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

fn optional_millis(duration: Option<Duration>) -> String {
    duration.map_or_else(
        || "not observed".to_owned(),
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

fn render_tunnel(ui: &mut egui::Ui, tunnel: &TunnelPreview) {
    let snapshot = tunnel.snapshot();
    let state = snapshot.state().to_string();
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new("tunnel-details")
        .num_columns(2)
        .max_col_width(bounds.available)
        .show(ui, |ui| {
            wrapped_cell(ui, bounds.key_max, "State");
            let value_max = bounds.value_max(ui);
            wrapped_cell(ui, value_max, state);
            ui.end_row();
            wrapped_cell(ui, bounds.key_max, "Sent to upstream");
            let value_max = bounds.value_max(ui);
            wrapped_cell(
                ui,
                value_max,
                egui::RichText::new(format!("{} bytes", snapshot.client_to_upstream())).monospace(),
            );
            ui.end_row();
            wrapped_cell(ui, bounds.key_max, "Received from upstream");
            let value_max = bounds.value_max(ui);
            wrapped_cell(
                ui,
                value_max,
                egui::RichText::new(format!("{} bytes", snapshot.upstream_to_client())).monospace(),
            );
            ui.end_row();
            wrapped_cell(ui, bounds.key_max, "Duration");
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
