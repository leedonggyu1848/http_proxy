//! Right-hand detail pane for the selected URL map node.
//!
//! Privacy invariant: query parameter values are never shown anywhere in this
//! pane. Matching requests display path-only URLs with query parameter *names*
//! only.

use std::collections::HashSet;
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use http_proxy::analysis::endpoint_schema::EndpointSchema;
use http_proxy::analysis::filter::RecordList;
use http_proxy::analysis::url::{parse_exchange_target, percent_decode_display};
use http_proxy::analysis::url_map::{NodeStats, RequestScope, ViewNode};
use http_proxy::exchange::{ByteSize, Exchange, RecordId};

use super::{MapAction, endpoint_view};
use crate::ui::requests::outcome_view::outcome_text;
use crate::ui::style::theme::Palette;

/// Maximum matching retained requests listed directly inside the node detail pane.
const MAX_INLINE_REQUESTS: usize = 100;

/// Caches which retained [`Exchange`]s in [`RecordList`] fall in a
/// [`RequestScope`] so steady UI frames never re-parse up to 2,000 request URLs.
#[derive(Default)]
pub(crate) struct RecordMatchCache {
    key: Option<RequestScope>,
    revision: Option<u64>,
    has_previews: bool,
    matched_count: usize,
    matched_ids: HashSet<RecordId>,
    newest_previews: Vec<(usize, String)>,
}

impl RecordMatchCache {
    pub(crate) fn clear(&mut self) {
        self.key = None;
        self.revision = None;
        self.has_previews = false;
        self.matched_count = 0;
        self.matched_ids.clear();
        self.newest_previews.clear();
    }

    pub(crate) fn sync(
        &mut self,
        key: &RequestScope,
        records: &RecordList<Exchange>,
        build_previews: bool,
    ) {
        let revision = records.revision();
        let previews_ready = !build_previews || self.has_previews;
        if self.key.as_ref() == Some(key) && self.revision == Some(revision) && previews_ready {
            return;
        }
        self.key = Some(key.clone());
        self.revision = Some(revision);
        self.has_previews = build_previews;
        self.matched_ids.clear();
        self.newest_previews.clear();
        let mut matched_indices = Vec::new();
        for (idx, record) in records.iter().enumerate() {
            if key.matches_exchange(record) {
                let _ = self.matched_ids.insert(record.id());
                matched_indices.push(idx);
            }
        }
        self.matched_count = matched_indices.len();
        if build_previews {
            for idx in matched_indices.into_iter().rev().take(MAX_INLINE_REQUESTS) {
                if let Some(record) = records.get(idx) {
                    self.newest_previews
                        .push((idx, safe_display_target(record)));
                }
            }
        }
    }

    pub(crate) const fn matched_count(&self) -> usize {
        self.matched_count
    }

    pub(super) fn is_fresh_for(&self, key: &RequestScope, records: &RecordList<Exchange>) -> bool {
        self.key.as_ref() == Some(key) && self.revision == Some(records.revision())
    }

    pub(crate) fn fresh_ids(
        &self,
        key: &RequestScope,
        records: &RecordList<Exchange>,
    ) -> Option<&HashSet<RecordId>> {
        self.is_fresh_for(key, records).then_some(&self.matched_ids)
    }

    #[cfg(test)]
    pub(super) fn contains(&self, id: RecordId) -> bool {
        self.matched_ids.contains(&id)
    }
}

/// Endpoint aggregates merged for the selected node.
pub(super) struct Endpoints<'a> {
    pub(super) merged: &'a [(String, EndpointSchema)],
    /// Requests never analyzed because the analysis queue was full.
    pub(super) skipped: u64,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    node: Option<&ViewNode>,
    records: &RecordList<Exchange>,
    cache: &mut RecordMatchCache,
    endpoints: &Endpoints<'_>,
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

    cache.sync(&RequestScope::subtree(node.key.clone()), records, true);
    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("url-map-detail-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let retained = cache.matched_count();
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
            if let Some(endpoint_action) =
                endpoint_view::show(ui, node, endpoints.merged, endpoints.skipped)
            {
                action = Some(endpoint_action);
            }
            ui.separator();
            render_query_params_section(ui, node);
            ui.separator();
            if let Some(req_action) = render_requests_section(ui, records, cache, evicted) {
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
            action = Some(MapAction::ShowInRequests(RequestScope::subtree(
                node.key.clone(),
            )));
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
            ui.label(ByteSize::new(stats.total_bytes).to_string());
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
    records: &RecordList<Exchange>,
    cache: &RecordMatchCache,
    evicted: u64,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    let total_retained = cache.matched_count();
    ui.label(RichText::new(format!("Requests under this node ({total_retained})")).strong());
    if evicted > 0 {
        ui.label(
            RichText::new(format!(
                "{evicted} older request(s) were evicted from the screen buffer; aggregates above still include them."
            ))
            .color(palette.warning)
            .small(),
        );
    }
    if total_retained == 0 {
        ui.weak("No retained requests remain for this node.");
        return None;
    }

    let mut action = None;
    for (idx, safe_path) in &cache.newest_previews {
        let Some(record) = records.get(*idx) else {
            continue;
        };
        let (status_label, status_color) = outcome_text(record.outcome(), &palette);
        ui.horizontal(|ui| {
            ui.monospace(record.request().method().as_str());
            ui.label(RichText::new(status_label).color(status_color).small());
            let link = ui.add(
                egui::Label::new(RichText::new(safe_path.as_str()).monospace())
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
    if total_retained > MAX_INLINE_REQUESTS {
        ui.weak(format!(
            "Showing newest {MAX_INLINE_REQUESTS} of {total_retained} retained requests. Use Show in Requests to inspect all."
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use http_proxy::analysis::filter::RecordFilter;
    use http_proxy::analysis::url::{OriginKey, PathSegmentPattern};
    use http_proxy::analysis::url_map::ViewNodeKey;
    use http_proxy::interception::{ProxyConfig, ProxyHandle};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::timeout;

    use super::*;
    use crate::ui::detail::search::RequestSearch;
    use crate::ui::requests::filter::visible_records;

    async fn capture_paths(
        paths: &[&str],
    ) -> Result<(Vec<Exchange>, OriginKey), Box<dyn std::error::Error>> {
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let mut exchanges = Vec::with_capacity(paths.len());
        for path in paths {
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            let req = format!(
                "GET http://{address}{path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            );
            client.write_all(req.as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let record = proxy.receive().await.ok_or("missing exchange")?;
            let mut head = Vec::new();
            let mut byte = [0; 1];
            while !head.ends_with(b"\r\n\r\n") {
                server.read_exact(&mut byte).await?;
                head.extend_from_slice(&byte);
            }
            server
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await?;
            drop(server);
            let mut sink = Vec::new();
            client.read_to_end(&mut sink).await?;
            exchanges.push(record);
        }
        proxy.stop()?;
        let origin = OriginKey::new("http", "127.0.0.1", Some(address.port()));
        Ok((exchanges, origin))
    }

    fn literal_key(origin: &OriginKey, segs: &[&str]) -> RequestScope {
        RequestScope::subtree(ViewNodeKey {
            origin: origin.clone(),
            path: segs
                .iter()
                .map(|s| PathSegmentPattern::Literal((*s).to_owned()))
                .collect(),
        })
    }

    #[tokio::test]
    async fn record_match_cache_invalidates_on_push_eviction_clear_key_and_preview_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let (exchanges, origin) = timeout(
            Duration::from_secs(5),
            capture_paths(&[
                "/api/users?token=SECRET_CACHE_1",
                "/api/orders?page=1",
                "/api/users?token=SECRET_CACHE_2",
            ]),
        )
        .await??;
        let [first, second, third]: [Exchange; 3] = exchanges
            .try_into()
            .map_err(|_| "expected exactly 3 exchanges")?;
        let users_key = literal_key(&origin, &["api", "users"]);
        let orders_key = literal_key(&origin, &["api", "orders"]);

        let mut records = RecordList::new(2);
        let (first_id, second_id, third_id) = (first.id(), second.id(), third.id());
        let _ = records.push(first);
        let _ = records.push(second);

        let mut cache = RecordMatchCache::default();
        assert!(!cache.is_fresh_for(&users_key, &records));

        // Hit without previews, then transition build_previews false -> true.
        cache.sync(&users_key, &records, false);
        assert!(cache.is_fresh_for(&users_key, &records));
        assert_eq!(cache.matched_count(), 1);
        assert!(cache.contains(first_id) && !cache.contains(second_id));
        assert_eq!(cache.newest_previews.len(), 0);

        cache.sync(&users_key, &records, true);
        assert!(cache.has_previews && cache.newest_previews.len() == 1);
        assert!(
            cache
                .newest_previews
                .iter()
                .all(|(_, text)| !text.contains("SECRET_CACHE_1") && text.contains("?token"))
        );

        // Overflow eviction invalidates cache; stale visible_records falls back accurately.
        assert_eq!(records.push(third).map(|e| e.id()), Some(first_id));
        assert!(!cache.is_fresh_for(&users_key, &records));
        let search = RequestSearch::new(&egui::Context::default());
        let fallback = visible_records(
            &records,
            RecordFilter::All,
            &search,
            Some(&users_key),
            Some(&cache),
        );
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback.first().map(|r| r.id()), Some(third_id));

        cache.sync(&users_key, &records, true);
        assert!(cache.is_fresh_for(&users_key, &records));
        assert!(!cache.contains(first_id) && cache.contains(third_id));
        assert!(
            cache
                .newest_previews
                .iter()
                .all(|(_, text)| !text.contains("SECRET_CACHE_2"))
        );

        // Key change + clear invalidation.
        assert!(!cache.is_fresh_for(&orders_key, &records));
        cache.sync(&orders_key, &records, false);
        assert_eq!(cache.matched_count(), 1);
        assert!(cache.contains(second_id));

        records.clear(0);
        assert!(!cache.is_fresh_for(&orders_key, &records));
        cache.sync(&orders_key, &records, true);
        assert_eq!(cache.matched_count(), 0);
        assert_eq!(cache.newest_previews.len(), 0);
        Ok(())
    }
}
