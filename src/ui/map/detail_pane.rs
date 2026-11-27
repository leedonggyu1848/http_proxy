//! Right-hand detail pane for the selected URL map node.
//!
//! Privacy invariant: query parameter values are never shown anywhere in this
//! pane. Matching requests display path-only URLs with query parameter *names*
//! only.

use std::collections::HashSet;
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use http_inspector_analysis::endpoint_schema::EndpointSchema;
use http_inspector_analysis::filter::RecordList;
use http_inspector_analysis::url::{parse_exchange_target, percent_decode_display};
use http_inspector_analysis::url_map::{NodeStats, RequestScope, ViewNode};
use http_inspector_exchange::{ByteSize, Exchange, RecordId};

use super::{MapAction, endpoint_view};
use crate::ui::i18n::{self, Language, domain};
use crate::ui::requests::outcome_view::outcome_text;
use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Layout;
use crate::ui::workbench::text;
use crate::ui::workbench::{chrome, controls, tree};

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
    newest_previews: Vec<(usize, TargetPreview)>,
}

enum TargetPreview {
    Invalid,
    Tunnel(String),
    Path(String),
}

impl TargetPreview {
    fn display(&self, lang: Language) -> String {
        match self {
            Self::Invalid => lang.map().invalid_target.to_owned(),
            Self::Tunnel(origin) => i18n::map::tunnel_target(lang, origin),
            Self::Path(path) => path.clone(),
        }
    }

    #[cfg(test)]
    fn as_english(&self) -> String {
        self.display(Language::English)
    }
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
    lang: Language,
) -> Option<MapAction> {
    let map_text = lang.map();
    let Some(node) = node else {
        chrome::pane_header(ui, "map-detail-header", |ui| {
            pane_title(ui, map_text.node);
        });
        ui.centered_and_justified(|ui| {
            ui.add(egui::Label::new(RichText::new(map_text.no_node_selected).weak()).wrap());
        });
        return None;
    };

    cache.sync(&RequestScope::subtree(node.key.clone()), records, true);
    let mut action = header(ui, node, lang);
    egui::ScrollArea::vertical()
        .id_salt("url-map-detail-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let retained = cache.matched_count();
            let evicted = node
                .subtree
                .request_count
                .saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));
            let key_width = tree::key_width(ui.available_width());
            tree::group(ui, "map-summary", map_text.summary, None, |ui| {
                render_stats_section(ui, node, retained, evicted, key_width, lang);
            });
            if let Some(endpoint_action) =
                endpoint_view::show(ui, node, endpoints.merged, endpoints.skipped, lang)
            {
                action = Some(endpoint_action);
            }
            render_query_params_section(ui, node, key_width, lang);
            if let Some(req_action) = render_requests_section(ui, records, cache, evicted, lang) {
                action = Some(req_action);
            }
        });
    action
}

/// The pane header: the node's URL as the pane name and Show in Requests at
/// the right (D22).
fn header(ui: &mut egui::Ui, node: &ViewNode, lang: Language) -> Option<MapAction> {
    let map_text = lang.map();
    chrome::pane_header(ui, "map-detail-header", |ui| {
        let action = ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                controls::ghost_style(ui);
                let clicked = ui
                    .add(controls::ghost(
                        map_text.show_in_requests,
                        Layout::PANE_BUTTON_HEIGHT,
                    ))
                    .on_hover_text(map_text.tooltips.show_in_requests_hint)
                    .clicked();
                let url = node.key.display_url();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&url)
                                .font(fonts::data())
                                .color(Palette::of(ui).text),
                        )
                        .truncate(),
                    )
                    .on_hover_text(url);
                });
                clicked
            })
            .inner;
        action.then(|| MapAction::ShowInRequests(RequestScope::subtree(node.key.clone())))
    })
}

fn pane_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .font(fonts::panel_title())
            .color(Palette::of(ui).text),
    );
}

/// One key-value row under a group.
fn pair(ui: &mut egui::Ui, key_width: f32, key: &str, value: RichText) {
    tree::row(ui, tree::TreeRow::new(1, key), |ui| {
        tree::key_value(ui, key_width, tree::key_text(ui, key), value);
    });
}

fn render_stats_section(
    ui: &mut egui::Ui,
    node: &ViewNode,
    retained: usize,
    evicted: u64,
    key_width: f32,
    lang: Language,
) {
    let shared = lang.shared();
    let map_text = lang.map();
    let palette = Palette::of(ui);
    let stats = &node.subtree;
    let data = |text: String| RichText::new(text).font(fonts::data()).color(palette.text);

    pair(
        ui,
        key_width,
        shared.requests,
        data(i18n::map::node_requests(
            lang,
            stats.request_count,
            node.direct.request_count,
            retained,
            evicted,
        )),
    );
    let methods = stats.methods.names().join(" ");
    pair(
        ui,
        key_width,
        map_text.methods,
        data(if methods.is_empty() {
            shared.not_observed.to_owned()
        } else {
            methods
        }),
    );
    let counts = status_counts(ui, stats, lang);
    tree::row(ui, tree::TreeRow::new(1, shared.status), |ui| {
        tree::key_value(ui, key_width, tree::key_text(ui, shared.status), counts);
    });
    pair(
        ui,
        key_width,
        map_text.content_type,
        data(format_content_types(stats)),
    );
    pair(
        ui,
        key_width,
        map_text.transferred,
        data(ByteSize::new(stats.total_bytes).to_string()),
    );
    let timing_text = match (stats.avg_duration_ms(), stats.max_duration_ms()) {
        (Some(avg), Some(max)) => i18n::map::avg_max(lang, avg, max),
        _ => shared.not_observed.to_owned(),
    };
    pair(ui, key_width, map_text.response_time, data(timing_text));
    pair(
        ui,
        key_width,
        map_text.observed,
        data(format!(
            "{} – {}",
            format_wall_time(stats.first_seen),
            format_wall_time(stats.last_seen),
        )),
    );
    render_node_flags(ui, node, key_width, lang);
}

/// Rows that only some nodes have: party, folded IDs, tunnel, and merged
/// requests.
fn render_node_flags(ui: &mut egui::Ui, node: &ViewNode, key_width: f32, lang: Language) {
    let shared = lang.shared();
    let map_text = lang.map();
    let palette = Palette::of(ui);
    let stats = &node.subtree;
    let data = |text: String| RichText::new(text).font(fonts::data()).color(palette.text);
    if stats.is_third_party() {
        pair(
            ui,
            key_width,
            map_text.party,
            data(i18n::map::third_party_hits(lang, stats.third_party_hits)),
        );
    }
    if node.is_folded_id {
        let kinds: Vec<&str> = stats
            .observed_id_kinds
            .iter()
            .map(|kind| domain::id_kind(lang, *kind))
            .collect();
        pair(
            ui,
            key_width,
            map_text.folded_ids,
            data(text::folded_kinds(node.folded_count, &kinds)),
        );
    }
    if stats.is_tunnel_only() {
        pair(
            ui,
            key_width,
            shared.tunnel,
            RichText::new(map_text.no_path_info)
                .font(fonts::data())
                .color(palette.secondary),
        );
    }
    if stats.overflow_merged > 0 {
        pair(
            ui,
            key_width,
            map_text.merged,
            RichText::new(i18n::map::merged_at_cap(lang, stats.overflow_merged))
                .font(fonts::data())
                .color(palette.warning),
        );
    }
}

/// Status buckets with requests, such as `2xx 4 · 5xx 2`. Only 4xx and
/// failures take a status color; the rest keep the text color (S7, D1).
fn status_counts(ui: &egui::Ui, stats: &NodeStats, lang: Language) -> egui::text::LayoutJob {
    let shared = lang.shared();
    let palette = Palette::of(ui);
    let buckets = [
        ("2xx", stats.status.s2xx, palette.text),
        ("3xx", stats.status.s3xx, palette.text),
        ("4xx", stats.status.s4xx, palette.warning),
        ("5xx", stats.status.s5xx, palette.error),
        (shared.failed, stats.status.failed, palette.error),
        (shared.tunnel, stats.status.tunnel, palette.text),
        (shared.pending, stats.status.pending, palette.text),
    ];
    let mut job = egui::text::LayoutJob::default();
    for (label, count, color) in buckets.into_iter().filter(|(_, count, _)| *count > 0) {
        if !job.text.is_empty() {
            job.append(
                " · ",
                0.0,
                egui::TextFormat::simple(fonts::data(), palette.secondary),
            );
        }
        job.append(
            &format!("{label} {count}"),
            0.0,
            egui::TextFormat::simple(fonts::data(), color),
        );
    }
    if job.text.is_empty() {
        job.append(
            shared.not_observed,
            0.0,
            egui::TextFormat::simple(fonts::data(), palette.secondary),
        );
    }
    job
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

/// Query parameter names seen under the node, most frequent first. Values
/// are never shown.
fn render_query_params_section(ui: &mut egui::Ui, node: &ViewNode, key_width: f32, lang: Language) {
    let params = if node.direct.query_params.is_empty() {
        &node.subtree.query_params
    } else {
        &node.direct.query_params
    };
    let mut pairs: Vec<(&String, &u64)> = params.iter().collect();
    pairs.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    tree::group(
        ui,
        "map-query",
        lang.map().query_names,
        Some(pairs.len()),
        |ui| {
            for (name, count) in pairs.into_iter().take(MAX_QUERY_NAMES) {
                let value = RichText::new(text::times(*count))
                    .font(fonts::data())
                    .color(Palette::of(ui).secondary);
                tree::row(ui, tree::TreeRow::new(1, name), |ui| {
                    tree::key_value(
                        ui,
                        key_width,
                        RichText::new(percent_decode_display(name))
                            .font(fonts::data())
                            .color(Palette::of(ui).text),
                        value,
                    );
                });
            }
        },
    );
}

/// Query parameter names listed before the rest are left out.
const MAX_QUERY_NAMES: usize = 40;

/// The newest retained requests under the node, each opening the request in
/// the Requests view.
fn render_requests_section(
    ui: &mut egui::Ui,
    records: &RecordList<Exchange>,
    cache: &RecordMatchCache,
    evicted: u64,
    lang: Language,
) -> Option<MapAction> {
    let shared = lang.shared();
    let map_text = lang.map();
    let palette = Palette::of(ui);
    let total_retained = cache.matched_count();
    let mut action = None;
    tree::group(
        ui,
        "map-requests",
        shared.requests,
        Some(total_retained),
        |ui| {
            if evicted > 0 {
                tree::row(ui, tree::TreeRow::new(1, ""), |ui| {
                    ui.label(
                        RichText::new(i18n::toolbar::older_removed(lang, evicted))
                            .color(palette.warning),
                    );
                });
            }
            for (idx, preview) in &cache.newest_previews {
                let Some(record) = records.get(*idx) else {
                    continue;
                };
                let (status_label, status_tone) = outcome_text(record.outcome(), lang);
                let safe_path = preview.display(lang);
                let shown = tree::row(
                    ui,
                    tree::TreeRow::new(1, safe_path.as_str()).clickable(),
                    |ui| {
                        let data = |text: &str, color| {
                            egui::Label::new(RichText::new(text).font(fonts::data()).color(color))
                                .selectable(false)
                        };
                        ui.add(data(record.request().method().as_str(), palette.secondary));
                        ui.add(data(&status_label, palette.status(status_tone)));
                        ui.add(data(safe_path.as_str(), palette.text).truncate());
                    },
                );
                if shown
                    .response
                    .on_hover_text(map_text.tooltips.open_in_requests)
                    .clicked()
                {
                    action = Some(MapAction::SelectRequest(record.id()));
                }
            }
            if total_retained > MAX_INLINE_REQUESTS {
                tree::row(ui, tree::TreeRow::new(1, ""), |ui| {
                    ui.label(
                        RichText::new(i18n::map::newest_of(
                            lang,
                            MAX_INLINE_REQUESTS,
                            total_retained,
                        ))
                        .color(palette.secondary),
                    );
                });
            }
        },
    );
    action
}

/// Formats an exchange target for the Map screen WITHOUT query values.
///
/// Preserves the normalized path and appends only query parameter names
/// (e.g. `/users?page,sort`), ensuring tokens in query values never appear in
/// the Map UI.
fn safe_display_target(record: &Exchange) -> TargetPreview {
    let Some(url) = parse_exchange_target(record.request().target(), record.source()) else {
        return TargetPreview::Invalid;
    };
    if url.is_tunnel() {
        return TargetPreview::Tunnel(url.origin().to_string());
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
        TargetPreview::Path(path)
    } else {
        let names: Vec<String> = url
            .query_param_names()
            .iter()
            .map(|name| percent_decode_display(name))
            .collect();
        TargetPreview::Path(format!("{path}?{}", names.join(",")))
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

    use http_inspector_analysis::filter::RecordFilter;
    use http_inspector_analysis::url::{OriginKey, PathSegmentPattern};
    use http_inspector_analysis::url_map::ViewNodeKey;
    use http_inspector_core::interception::{ProxyConfig, ProxyHandle};
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
        assert!(cache.newest_previews.iter().all(|(_, text)| {
            let rendered = text.as_english();
            !rendered.contains("SECRET_CACHE_1") && rendered.contains("?token")
        }));

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
                .all(|(_, text)| !text.as_english().contains("SECRET_CACHE_2"))
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
