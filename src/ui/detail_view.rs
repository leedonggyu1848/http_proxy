//! Selected request details and body previews.

use crate::ui::InspectorApp;
use crate::ui::body_display::{BodySide, BodySource, BodyViewCache};
use crate::ui::body_format::{BodyFormat, ViewMode, form, highlight::SyntaxPalette};
use crate::ui::detail_tabs::{self, DetailTab};
use crate::ui::theme::Palette;
use crate::ui::{image_view, outcome_view, replay_panel, scope_help};
use eframe::egui;
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, Exchange, HeaderPreview, Outcome, RecordId,
    TunnelPreview,
};
use std::time::Duration;

impl InspectorApp {
    pub(in crate::ui) fn detail_panel(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let Some(selected) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a request to inspect headers and body previews.");
            });
            return;
        };
        let Some(record) = self.records.iter().find(|record| record.id() == selected) else {
            return;
        };

        if shortcuts_allowed {
            detail_tabs::apply_shortcuts(ui, &mut self.detail_tab);
        }
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
        if let Some(original) = record.replay_of()
            && ui
                .link(format!("Replay of #{} — show original", original.value()))
                .clicked()
        {
            self.selected = Some(original);
        }
        detail_tabs::show(ui, &mut self.detail_tab);

        egui::ScrollArea::vertical()
            .id_salt(("details-scroll", self.detail_tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let request = record.request();
                ui.heading(format!("{} {}", request.method(), request.target()));
                render_timing(ui, record);
                ui.separator();
                let mut body = BodyPanel {
                    record: record.id(),
                    mode: self.body_mode,
                    cache: &mut self.body_cache,
                };
                match self.detail_tab {
                    DetailTab::Headers => {
                        render_query(ui, request.target());
                        ui.strong("Request headers");
                        render_headers(ui, "request-headers", request.headers());
                    }
                    DetailTab::Body => {
                        let has_tree = tree_headers(request.headers())
                            || matches!(record.outcome(),
                            Outcome::Response(response) if tree_headers(response.headers()));
                        body.mode = view_mode_bar(ui, &mut self.body_mode, has_tree);
                        ui.strong("Request body");
                        render_optional_body(ui, request.body(), request.headers(), &mut body);
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
}

fn tree_headers(headers: &HeaderPreview) -> bool {
    matches!(
        BodyFormat::from_content_type(headers.get("content-type").next()),
        BodyFormat::Json | BodyFormat::Xml
    )
}

/// Shows Tree only as an enabled option when either body has JSON or XML content type.
fn view_mode_bar(ui: &mut egui::Ui, mode: &mut ViewMode, has_tree: bool) -> ViewMode {
    if !has_tree && *mode == ViewMode::Tree {
        *mode = ViewMode::Pretty;
    }
    ui.horizontal(|ui| {
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
    egui::Grid::new("query-parameters")
        .striped(true)
        .show(ui, |ui| {
            for (key, value) in pairs {
                ui.label(
                    egui::RichText::new(form::visible(&key))
                        .monospace()
                        .color(key_color),
                );
                ui.monospace(form::visible(&value));
                ui.end_row();
            }
        });
    ui.separator();
}

fn render_headers(ui: &mut egui::Ui, id: &str, headers: &HeaderPreview) {
    let name_color = SyntaxPalette::for_theme(ui.ctx().theme()).key;
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for header in headers.entries() {
            ui.label(
                egui::RichText::new(format!("{}:", header.name()))
                    .monospace()
                    .color(name_color),
            );
            ui.monospace(header.value());
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

fn render_optional_body(
    ui: &mut egui::Ui,
    body: Option<&BodyPreview>,
    headers: &HeaderPreview,
    panel: &mut BodyPanel<'_>,
) {
    match body {
        Some(body) => render_body(ui, body, headers, (panel, BodySide::Request), None),
        None => {
            ui.weak("Body was not observed.");
        }
    }
}

/// Draws a body preview; `image` draws image bodies of that record as images.
fn render_body(
    ui: &mut egui::Ui,
    body: &BodyPreview,
    headers: &HeaderPreview,
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
    let content_type = headers.get("content-type").next();
    if let Some(image) = image
        && let Some(format) = content_type.and_then(http_proxy::ImageFormat::from_media_type)
    {
        image.show(ui, panel.record, &snapshot, &encodings, format);
        return;
    }
    let complete = snapshot.lifecycle() == BodyLifecycle::Complete
        && snapshot.retention() == BodyRetention::Prefix;
    let source = BodySource {
        record: panel.record.value(),
        side,
        bytes: snapshot.bytes(),
        encodings: &encodings,
        content_type,
        complete,
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
                render_headers(ui, "response-headers", response.headers());
            }
            DetailTab::Body => {
                ui.strong("Response body");
                render_body(
                    ui,
                    response.body(),
                    response.headers(),
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
    egui::Grid::new("timing").num_columns(2).show(ui, |ui| {
        for (label, value) in rows {
            ui.label(label);
            ui.monospace(value);
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
    egui::Grid::new("tunnel-details")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("State");
            ui.label(state);
            ui.end_row();
            ui.label("Sent to upstream");
            ui.monospace(format!("{} bytes", snapshot.client_to_upstream()));
            ui.end_row();
            ui.label("Received from upstream");
            ui.monospace(format!("{} bytes", snapshot.upstream_to_client()));
            ui.end_row();
            ui.label("Duration");
            ui.monospace(format!(
                "{:.1} ms",
                outcome_view::elapsed_millis(snapshot.duration())
            ));
            ui.end_row();
        });
}
