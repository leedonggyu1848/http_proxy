//! Right-hand detail pane for the selected URL map node.
//!
//! Privacy invariant: query parameter values are never shown anywhere in this
//! pane. Matching requests display path-only URLs with query parameter *names*
//! only.

use std::time::SystemTime;

use eframe::egui::{self, RichText};
use http_proxy::Exchange;
use http_proxy::url_map::{
    NodeStats, ViewNode, format_bytes, parse_exchange_target, percent_decode_display,
};

use super::MapAction;
use crate::ui::outcome_view::outcome_text;
use crate::ui::records::RecordList;
use crate::ui::theme::Palette;

/// Maximum matching retained requests listed directly inside the node detail pane.
const MAX_INLINE_REQUESTS: usize = 100;

pub(super) fn show(
    ui: &mut egui::Ui,
    node: Option<&ViewNode>,
    records: &RecordList<Exchange>,
) -> Option<MapAction> {
    let Some(node) = node else {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Select a host or path node to inspect its aggregates and requests.",
                    )
                    .weak(),
                )
                .wrap(),
            );
        });
        return None;
    };

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("url-map-detail-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let matching: Vec<&Exchange> = records
                .iter()
                .filter(|record| node.key.matches_exchange(record))
                .collect();
            let retained = matching.len();
            let evicted = node
                .subtree
                .request_count
                .saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));

            if let Some(header_action) = render_header(ui, node, retained, evicted) {
                action = Some(header_action);
            }
            ui.separator();
            render_stats_section(ui, node, retained, evicted);
            ui.separator();
            render_query_params_section(ui, node);
            ui.separator();
            if let Some(req_action) = render_requests_section(ui, &matching, evicted) {
                action = Some(req_action);
            }
        });
    action
}

fn render_header(
    ui: &mut egui::Ui,
    node: &ViewNode,
    retained: usize,
    evicted: u64,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    let mut action = None;

    ui.add(egui::Label::new(RichText::new(node.key.display_url()).strong().monospace()).wrap());

    ui.horizontal_wrapped(|ui| {
        if node.subtree.is_tunnel_only() {
            ui.label(RichText::new("[Tunnel] No path info").color(palette.info));
        }
        if node.is_folded_id {
            let kinds: Vec<&str> = node
                .subtree
                .observed_id_kinds
                .iter()
                .map(|kind| kind.label())
                .collect();
            let kind_text = if kinds.is_empty() {
                String::new()
            } else {
                format!(" · types: {}", kinds.join(", "))
            };
            ui.label(
                RichText::new(format!("Folded {} IDs{kind_text}", node.folded_count))
                    .color(palette.info)
                    .small(),
            );
        }
        if node.subtree.is_third_party() {
            ui.label(
                RichText::new(format!(
                    "3rd-party ({} hits)",
                    node.subtree.third_party_hits
                ))
                .color(palette.warning)
                .small(),
            );
        }
        if node.subtree.overflow_merged > 0 {
            ui.label(
                RichText::new(format!(
                    "{} deeper requests merged at node cap",
                    node.subtree.overflow_merged
                ))
                .color(palette.warning)
                .small(),
            );
        }
    });

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Show in Requests")
            .on_hover_text("Switch to Requests view filtered to this origin and path prefix.")
            .clicked()
        {
            action = Some(MapAction::ShowInRequests(node.key.clone()));
        }
        let retention_note = if evicted > 0 {
            format!("{retained} retained · {evicted} older removed")
        } else {
            format!("{retained} retained")
        };
        ui.label(RichText::new(retention_note).color(palette.muted).small());
    });
    action
}

fn render_stats_section(ui: &mut egui::Ui, node: &ViewNode, retained: usize, evicted: u64) {
    let palette = Palette::of(ui);
    let stats = &node.subtree;

    ui.label(RichText::new("Summary").strong());
    egui::Grid::new("url-map-stats-grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.weak("Requests");
            let req_text = if node.direct.request_count > 0
                && node.direct.request_count != stats.request_count
            {
                format!(
                    "{} total ({} direct, {retained} retained, {evicted} evicted)",
                    stats.request_count, node.direct.request_count
                )
            } else if evicted > 0 {
                format!(
                    "{} total ({retained} retained, {evicted} evicted)",
                    stats.request_count
                )
            } else {
                format!("{}", stats.request_count)
            };
            ui.add(egui::Label::new(req_text).wrap());
            ui.end_row();

            ui.weak("Methods");
            let methods = stats.methods.names().join(", ");
            ui.add(egui::Label::new(if methods.is_empty() { "—" } else { &methods }).wrap());
            ui.end_row();

            ui.weak("Status");
            ui.horizontal_wrapped(|ui| {
                render_status_counts(ui, stats, &palette);
            });
            ui.end_row();

            ui.weak("Content-Type");
            ui.add(egui::Label::new(format_content_types(stats)).wrap());
            ui.end_row();

            ui.weak("Transferred");
            ui.label(format_bytes(stats.total_bytes));
            ui.end_row();

            ui.weak("Response time");
            let timing_text = match (stats.avg_duration_ms(), stats.max_duration_ms()) {
                (Some(avg), Some(max)) => format!("avg {avg:.1} ms · max {max:.1} ms"),
                _ => "—".to_owned(),
            };
            ui.label(timing_text);
            ui.end_row();

            ui.weak("Observed");
            ui.label(format!(
                "{} – {}",
                format_wall_time(stats.first_seen),
                format_wall_time(stats.last_seen),
            ));
            ui.end_row();
        });
}

fn render_status_counts(ui: &mut egui::Ui, stats: &NodeStats, palette: &Palette) {
    let buckets = [
        ("2xx", stats.status.s2xx, palette.success),
        ("3xx", stats.status.s3xx, palette.warning),
        ("4xx", stats.status.s4xx, palette.error),
        ("5xx", stats.status.s5xx, palette.error),
        ("Failed", stats.status.failed, palette.error),
        ("Tunnel", stats.status.tunnel, palette.info),
        ("Pending", stats.status.pending, palette.muted),
    ];
    let mut shown = false;
    for (label, count, color) in buckets {
        if count > 0 {
            ui.label(RichText::new(format!("{label}: {count}")).color(color));
            shown = true;
        }
    }
    if !shown {
        ui.weak("—");
    }
}

fn format_content_types(stats: &NodeStats) -> String {
    if stats.content_types.is_empty() {
        return "—".to_owned();
    }
    let mut pairs: Vec<(&String, &u64)> = stats.content_types.iter().collect();
    pairs.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    pairs
        .into_iter()
        .take(5)
        .map(|(ct, count)| format!("{ct} ({count})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_query_params_section(ui: &mut egui::Ui, node: &ViewNode) {
    ui.label(RichText::new("Query Parameters (names only)").strong());
    let params = if node.direct.query_params.is_empty() {
        &node.subtree.query_params
    } else {
        &node.direct.query_params
    };
    if params.is_empty() {
        ui.weak("No query parameters observed.");
        return;
    }
    let mut pairs: Vec<(&String, &u64)> = params.iter().collect();
    pairs.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    egui::Grid::new("url-map-query-params")
        .num_columns(2)
        .spacing([16.0, 2.0])
        .show(ui, |ui| {
            for (name, count) in pairs.into_iter().take(40) {
                ui.monospace(percent_decode_display(name));
                ui.weak(format!("{count}×"));
                ui.end_row();
            }
        });
}

fn render_requests_section(
    ui: &mut egui::Ui,
    matching: &[&Exchange],
    evicted: u64,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    ui.label(RichText::new(format!("Requests under this node ({})", matching.len())).strong());
    if evicted > 0 {
        ui.label(
            RichText::new(format!(
                "{evicted} older request(s) were evicted from the screen buffer; aggregates above still include them."
            ))
            .color(palette.warning)
            .small(),
        );
    }
    if matching.is_empty() {
        ui.weak("No retained requests remain for this node.");
        return None;
    }

    let mut action = None;
    for record in matching.iter().rev().take(MAX_INLINE_REQUESTS) {
        let (status_label, status_color) = outcome_text(record.outcome(), &palette);
        let safe_path = safe_display_target(record);
        ui.horizontal(|ui| {
            ui.monospace(record.request().method().as_str());
            ui.label(RichText::new(status_label).color(status_color).small());
            let link = ui.add(
                egui::Label::new(RichText::new(safe_path).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .sense(egui::Sense::click()),
            );
            if link
                .on_hover_text("Open this request in Requests view.")
                .clicked()
            {
                action = Some(MapAction::SelectRequest(record.id()));
            }
        });
    }
    if matching.len() > MAX_INLINE_REQUESTS {
        ui.weak(format!(
            "Showing newest {MAX_INLINE_REQUESTS} of {} retained requests. Use Show in Requests to inspect all.",
            matching.len()
        ));
    }
    action
}

/// Formats an exchange target for the Map screen WITHOUT query values.
///
/// Preserves the normalized path and appends only query parameter names
/// (e.g. `/users?page,sort`), ensuring tokens in query values never appear in
/// the Map UI.
fn safe_display_target(record: &Exchange) -> String {
    let Some(url) = parse_exchange_target(record.request().target(), record.source()) else {
        return "(invalid target)".to_owned();
    };
    if url.is_tunnel() {
        return format!("{} (tunnel)", url.origin());
    }
    let path = if url.segments().is_empty() {
        "/".to_owned()
    } else {
        let mut out = String::new();
        for seg in url.segments() {
            out.push('/');
            out.push_str(&percent_decode_display(seg));
        }
        out
    };
    if url.query_param_names().is_empty() {
        path
    } else {
        let names: Vec<String> = url
            .query_param_names()
            .iter()
            .map(|name| percent_decode_display(name))
            .collect();
        format!("{path}?{}", names.join(","))
    }
}

fn format_wall_time(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "—".to_owned();
    };
    let Ok(duration) = time.duration_since(SystemTime::UNIX_EPOCH) else {
        return "—".to_owned();
    };
    let secs = duration.as_secs() % 86_400;
    let hours = secs / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02} UTC")
}
