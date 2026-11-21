//! Selected request details and body previews.

use crate::ui::InspectorApp;
use crate::ui::{body_view, image_view, outcome_view, replay_panel, scope_help};
use eframe::egui::{self, Color32};
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, Exchange, HeaderPreview, Outcome, RecordId,
    TunnelPreview,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum DetailTab {
    Headers,
    Body,
}

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

        if shortcuts_allowed
            && ui.input_mut(|input| {
                !input.modifiers.any() && input.consume_key(egui::Modifiers::NONE, egui::Key::F6)
            })
        {
            self.detail_tab = match self.detail_tab {
                DetailTab::Headers => DetailTab::Body,
                DetailTab::Body => DetailTab::Headers,
            };
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
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.detail_tab, DetailTab::Headers, "Headers");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Body, "Body");
        });

        egui::ScrollArea::vertical()
            .id_salt(("details-scroll", self.detail_tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let request = record.request();
                ui.heading(format!("{} {}", request.method(), request.target()));
                render_timing(ui, record);
                ui.separator();
                match self.detail_tab {
                    DetailTab::Headers => {
                        ui.strong("Request headers");
                        render_headers(ui, "request-headers", request.headers());
                    }
                    DetailTab::Body => {
                        ui.strong("Request body");
                        render_optional_body(ui, request.body(), request.headers());
                    }
                }
                ui.separator();
                render_outcome(ui, record, self.detail_tab, &mut self.image);
            });
    }
}

fn render_headers(ui: &mut egui::Ui, id: &str, headers: &HeaderPreview) {
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for header in headers.entries() {
            ui.monospace(header.name());
            ui.monospace(header.value());
            ui.end_row();
        }
    });
    if headers.entries().is_empty() {
        ui.weak("No headers captured.");
    }
    if headers.omitted() {
        ui.colored_label(Color32::YELLOW, "Additional headers were omitted.");
    }
}

fn render_optional_body(ui: &mut egui::Ui, body: Option<&BodyPreview>, headers: &HeaderPreview) {
    match body {
        Some(body) => render_body(ui, body, headers, None),
        None => {
            ui.weak("Body was not observed.");
        }
    }
}

/// Draws a body preview; `image` draws PNG bodies of that record as images.
fn render_body(
    ui: &mut egui::Ui,
    body: &BodyPreview,
    headers: &HeaderPreview,
    image: Option<(&mut image_view::ImagePanel, RecordId)>,
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
            Color32::YELLOW,
            "Preview was omitted to keep forwarding nonblocking.",
        );
        return;
    }
    if snapshot.bytes().is_empty() {
        ui.weak("Empty body.");
        return;
    }
    let encodings = headers.get("content-encoding").collect::<Vec<_>>();
    if let Some((panel, record)) = image
        && headers
            .get("content-type")
            .next()
            .is_some_and(http_proxy::is_png_media_type)
    {
        panel.show(ui, record, &snapshot, &encodings);
        return;
    }
    let complete = snapshot.lifecycle() == BodyLifecycle::Complete
        && snapshot.retention() == BodyRetention::Prefix;
    let mut preview = match body_view::decode(snapshot.bytes(), &encodings, complete) {
        body_view::BodyText::Ready { text, notice } => {
            if let Some(notice) = notice {
                ui.weak(notice);
            }
            text
        }
        body_view::BodyText::Unavailable(message) => {
            ui.colored_label(Color32::YELLOW, message);
            return;
        }
    };
    ui.add(
        egui::TextEdit::multiline(&mut preview)
            .font(egui::TextStyle::Monospace)
            .desired_rows(8)
            .interactive(false),
    );
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
    image: &mut image_view::ImagePanel,
) {
    match record.outcome() {
        Outcome::Response(response) => match tab {
            DetailTab::Headers => {
                ui.strong(format!(
                    "Response headers · {} {} · {:?}",
                    response.status().as_str(),
                    response.reason(),
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
                    Some((image, record.id())),
                );
            }
        },
        Outcome::Tunnel(tunnel) => {
            ui.label(scope_help::TUNNEL_NOTE);
            render_tunnel(ui, tunnel);
        }
        outcome => {
            let (text, color) = outcome_view::detail_text(outcome);
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

pub(in crate::ui) fn configure_fonts(context: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/usr/share/fonts/truetype/nanum/NanumGothicCoding.ttf",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    ];
    let Some(data) = candidates
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let name = "cjk-fallback".to_owned();
    fonts
        .font_data
        .insert(name.clone(), egui::FontData::from_owned(data).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    context.set_fonts(fonts);
}
