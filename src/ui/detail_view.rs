//! Selected request details and body previews.

use crate::ui::InspectorApp;
use crate::ui::body_display::{BodySide, BodySource, BodyViewCache, Capture};
use crate::ui::detail_tabs::{self, DetailTab};
use crate::ui::highlight::SyntaxPalette;
use crate::ui::search::Query;
use crate::ui::search::highlight::{self, MatchColors};
use crate::ui::theme::Palette;
use crate::ui::{image_view, outcome_view, replay_panel, scope_help};
use eframe::egui;
use http_proxy::content::format::{BodyFormat, ViewMode, form};
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, CapturedRequest, Exchange, HeaderPreview, MediaType,
    Outcome, RecordId, TunnelPreview,
};
use std::time::Duration;

impl InspectorApp {
    pub(in crate::ui) fn detail_panel(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let Some(selected) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.add(
                    egui::Label::new("Select a request to inspect headers and body previews.")
                        .wrap(),
                );
            });
            return;
        };
        let Some(record) = self.records.iter().find(|record| record.id() == selected) else {
            return;
        };

        if shortcuts_allowed {
            detail_tabs::apply_shortcuts(ui, &mut self.detail_tab);
        }
        let request = record.request();
        let search = self.search.active_query().map(AsRef::as_ref);
        render_title(ui, record, search);
        ui.horizontal_wrapped(|ui| {
            let (status, color) = outcome_view::outcome_text(record.outcome(), &Palette::of(ui));
            ui.colored_label(color, status);
            if ui
                .add_enabled(
                    record.source() != http_proxy::CaptureSource::Connect,
                    egui::Button::new("Edit and Resend"),
                )
                .clicked()
            {
                match replay_panel::ReplayPanel::new(record.clone()) {
                    Ok(panel) => self.replay = Some(panel),
                    Err(error) => self.error = Some(error),
                }
            }
            self.curl.controls(ui, record);
            if let Some(original) = record.replay_of()
                && ui
                    .link(format!("Replay of #{} — show original", original.value()))
                    .clicked()
            {
                self.selected = Some(original);
            }
        });
        detail_tabs::show(ui, &mut self.detail_tab);

        egui::ScrollArea::vertical()
            .id_salt(("details-scroll", self.detail_tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_timing(ui, record);
                ui.separator();
                let mut body = BodyPanel {
                    record: record.id(),
                    mode: self.body_mode,
                    cache: &mut self.body_cache,
                    search,
                };
                match self.detail_tab {
                    DetailTab::Headers => {
                        render_query(ui, request.target());
                        ui.strong("Request headers");
                        render_headers(
                            ui,
                            "request-headers",
                            request.headers(),
                            body.headers_search(),
                        );
                    }
                    DetailTab::Body => {
                        let has_tree = has_tree(request.media_type())
                            || matches!(record.outcome(),
                            Outcome::Response(response) if has_tree(response.media_type()));
                        body.mode = view_mode_bar(ui, &mut self.body_mode, has_tree);
                        self.body_saving.menus(ui, record, &self.save);
                        ui.strong("Request body");
                        render_optional_body(ui, request, &mut body);
                    }
                }
                ui.separator();
                render_outcome(ui, record, self.detail_tab, &mut body, &mut self.image);
            });
    }
}

/// Where and how the bodies of the selected exchange are shown.
struct BodyPanel<'a> {
    record: RecordId,
    mode: ViewMode,
    cache: &'a mut BodyViewCache,
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
            let color = ui.visuals().strong_text_color();
            let colors = MatchColors::for_theme(ui.ctx().theme());
            let mut job = highlight::plain(&title, &query.matcher, font, color, colors);
            job.wrap.max_width = ui.available_width();
            egui::Label::new(job)
        }
        None => egui::Label::new(egui::RichText::new(title).strong().monospace()),
    };
    ui.add(label.wrap());
}

fn has_tree(media_type: Option<&MediaType>) -> bool {
    matches!(
        BodyFormat::from_media_type(media_type),
        BodyFormat::Json | BodyFormat::Xml
    )
}

/// Shows Tree only as an enabled option when either body has JSON or XML content type.
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
        let available = ui.available_width().max(120.0);
        let right = ui.cursor().left() + available;
        let key_max = ((available - ui.spacing().item_spacing.x) * 0.42).clamp(72.0, 220.0);
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
fn render_query(ui: &mut egui::Ui, target: &str) {
    let Some((_, query)) = target.split_once('?') else {
        return;
    };
    let query = query.split_once('#').map_or(query, |(query, _)| query);
    let pairs = form::pairs(query);
    if pairs.is_empty() {
        return;
    }
    ui.strong("Query parameters");
    let key_color = SyntaxPalette::for_theme(ui.ctx().theme()).key;
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new("query-parameters")
        .num_columns(2)
        .striped(true)
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

fn render_headers(ui: &mut egui::Ui, id: &str, headers: &HeaderPreview, search: Option<&Query>) {
    let name_color = SyntaxPalette::for_theme(ui.ctx().theme()).key;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = ui.visuals().text_color();
    let colors = MatchColors::for_theme(ui.ctx().theme());
    let bounds = GridBounds::from_ui(ui);
    egui::Grid::new(id)
        .num_columns(2)
        .striped(true)
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
        ui.weak("No headers captured.");
    }
    if headers.omitted() {
        ui.colored_label(Palette::of(ui).warning, "Additional headers were omitted.");
    }
}

fn render_optional_body(ui: &mut egui::Ui, request: &CapturedRequest, panel: &mut BodyPanel<'_>) {
    match request.body() {
        Some(body) => {
            let message = (request.headers(), request.media_type());
            render_body(ui, body, message, (panel, BodySide::Request), None);
        }
        None => {
            ui.weak("Body was not observed.");
        }
    }
}

/// Draws a body preview; `image` draws image bodies of that record as images.
fn render_body(
    ui: &mut egui::Ui,
    body: &BodyPreview,
    (headers, media_type): (&HeaderPreview, Option<&MediaType>),
    (panel, side): (&mut BodyPanel<'_>, BodySide),
    image: Option<&mut image_view::ImagePanel>,
) {
    let Some(snapshot) = body.snapshot() else {
        ui.weak("Preview is updating.");
        return;
    };
    ui.label(format!(
        "{} bytes observed · {} · {}",
        snapshot.observed_bytes(),
        lifecycle_text(snapshot.lifecycle()),
        retention_text(snapshot.retention())
    ));
    if snapshot.retention() == BodyRetention::Omitted {
        ui.colored_label(
            Palette::of(ui).warning,
            "Preview was omitted to keep forwarding nonblocking.",
        );
        return;
    }
    if snapshot.bytes().is_empty() {
        ui.weak("Empty body.");
        return;
    }
    let encodings = headers.get("content-encoding").collect::<Vec<_>>();
    if let Some(image) = image
        && let Some(format) = media_type.and_then(http_proxy::ImageFormat::from_media_type)
    {
        image.show(ui, panel.record, &snapshot, &encodings, format);
        return;
    }
    let source = BodySource {
        record: panel.record.value(),
        side,
        bytes: snapshot.bytes(),
        encodings: &encodings,
        media_type,
        capture: Capture::of(snapshot.lifecycle(), snapshot.retention()),
        search: panel.search.filter(|query| query.scope.covers_body(side)),
    };
    panel.cache.show(ui, &source, panel.mode);
}

const fn lifecycle_text(lifecycle: BodyLifecycle) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => "streaming",
        BodyLifecycle::Complete => "complete",
        BodyLifecycle::Interrupted => "interrupted by the sender",
        BodyLifecycle::Cancelled => "cancelled by the receiver",
    }
}

const fn retention_text(retention: BodyRetention) -> &'static str {
    match retention {
        BodyRetention::Prefix => "full preview",
        BodyRetention::Truncated => "truncated preview",
        BodyRetention::Omitted => "preview omitted",
    }
}

fn render_outcome(
    ui: &mut egui::Ui,
    record: &Exchange,
    tab: DetailTab,
    body: &mut BodyPanel<'_>,
    image: &mut image_view::ImagePanel,
) {
    match record.outcome() {
        Outcome::Response(response) => match tab {
            DetailTab::Headers => {
                ui.strong(format!(
                    "Response headers · {} {} ({}) · {:?}",
                    response.status().as_str(),
                    response.reason(),
                    outcome_view::status_class(response.status().as_u16()),
                    response.version()
                ));
                let search = body.headers_search();
                render_headers(ui, "response-headers", response.headers(), search);
            }
            DetailTab::Body => {
                ui.strong("Response body");
                render_body(
                    ui,
                    response.body(),
                    (response.headers(), response.media_type()),
                    (body, BodySide::Response),
                    Some(image),
                );
            }
        },
        Outcome::Tunnel(tunnel) => {
            ui.label(scope_help::TUNNEL_NOTE);
            render_tunnel(ui, tunnel);
        }
        outcome => {
            let (text, color) = outcome_view::detail_text(outcome, &Palette::of(ui));
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
    let state = outcome_view::tunnel_detail(snapshot.state());
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
