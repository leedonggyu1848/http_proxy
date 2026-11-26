//! Right-hand detail pane for the selected URL map node.
//!
//! Privacy invariant: query parameter values are never shown anywhere in this
//! pane. Matching requests display path-only URLs with query parameter *names*
//! only.

use crate::ui::style::tokens::Space;
use std::collections::HashSet;
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use http_inspector_analysis::endpoint_schema::EndpointSchema;
use http_inspector_analysis::filter::RecordList;
use http_inspector_analysis::url::{parse_exchange_target, percent_decode_display};
use http_inspector_analysis::url_map::{NodeStats, RequestScope, ViewNode};
use http_inspector_exchange::{ByteSize, Exchange, RecordId};

use super::{MapAction, endpoint_view};
use crate::ui::i18n::{Language, domain};
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
            Self::Invalid => lang.pick("(invalid target)", "(잘못된 대상)").to_owned(),
            Self::Tunnel(origin) => match lang {
                Language::English => format!("{origin} (tunnel)"),
                Language::Korean => format!("{origin} (터널)"),
            },
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
    let Some(node) = node else {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(lang.pick(
                        "Select a host or path node to inspect its aggregates and requests.",
                        "집계와 요청 목록을 확인하려면 호스트나 경로 노드를 선택하세요.",
                    ))
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

            if let Some(header_action) = render_header(ui, node, retained, evicted, lang) {
                action = Some(header_action);
            }
            ui.separator();
            render_stats_section(ui, node, retained, evicted, lang);
            ui.separator();
            if let Some(endpoint_action) =
                endpoint_view::show(ui, node, endpoints.merged, endpoints.skipped, lang)
            {
                action = Some(endpoint_action);
            }
            ui.separator();
            render_query_params_section(ui, node, lang);
            ui.separator();
            if let Some(req_action) = render_requests_section(ui, records, cache, evicted, lang) {
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
    lang: Language,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    let mut action = None;

    ui.add(egui::Label::new(RichText::new(node.key.display_url()).strong().monospace()).wrap());

    render_header_flags(ui, node, &palette, lang);

    ui.add_space(Space::XS);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(lang.pick("Show in Requests", "요청 목록에서 보기"))
            .on_hover_text(lang.pick(
                "Switch to Requests view filtered to this origin and path prefix.",
                "이 출처와 경로 접두사로 필터링된 요청 목록으로 전환합니다.",
            ))
            .clicked()
        {
            action = Some(MapAction::ShowInRequests(RequestScope::subtree(
                node.key.clone(),
            )));
        }
        let retention_note = if evicted > 0 {
            match lang {
                Language::English => format!("{retained} retained · {evicted} older removed"),
                Language::Korean => format!("{retained}건 보관 · 오래된 {evicted}건 제거됨"),
            }
        } else {
            match lang {
                Language::English => format!("{retained} retained"),
                Language::Korean => format!("{retained}건 보관"),
            }
        };
        ui.label(
            RichText::new(retention_note)
                .color(palette.secondary)
                .small(),
        );
    });
    action
}

fn render_header_flags(ui: &mut egui::Ui, node: &ViewNode, palette: &Palette, lang: Language) {
    ui.horizontal_wrapped(|ui| {
        if node.subtree.is_tunnel_only() {
            ui.label(
                RichText::new(lang.pick("[Tunnel] No path info", "[터널] 경로 정보 없음"))
                    .color(palette.secondary),
            );
        }
        if node.is_folded_id {
            let kinds: Vec<&str> = node
                .subtree
                .observed_id_kinds
                .iter()
                .map(|kind| domain::id_kind(lang, *kind))
                .collect();
            let kind_text = if kinds.is_empty() {
                String::new()
            } else {
                match lang {
                    Language::English => format!(" · types: {}", kinds.join(", ")),
                    Language::Korean => format!(" · 형식: {}", kinds.join(", ")),
                }
            };
            let folded = match lang {
                Language::English => format!("Folded {} IDs{kind_text}", node.folded_count),
                Language::Korean => format!("ID {}개 묶임{kind_text}", node.folded_count),
            };
            ui.label(RichText::new(folded).color(palette.secondary).small());
        }
        if node.subtree.is_third_party() {
            let third = match lang {
                Language::English => format!("3rd-party ({} hits)", node.subtree.third_party_hits),
                Language::Korean => format!("서드파티 ({}회)", node.subtree.third_party_hits),
            };
            ui.label(RichText::new(third).color(palette.warning).small());
        }
        if node.subtree.overflow_merged > 0 {
            let merged = match lang {
                Language::English => format!(
                    "{} deeper requests merged at node cap",
                    node.subtree.overflow_merged
                ),
                Language::Korean => format!(
                    "노드 한도로 하위 요청 {}건이 병합됨",
                    node.subtree.overflow_merged
                ),
            };
            ui.label(RichText::new(merged).color(palette.warning).small());
        }
    });
}

fn render_stats_section(
    ui: &mut egui::Ui,
    node: &ViewNode,
    retained: usize,
    evicted: u64,
    lang: Language,
) {
    let palette = Palette::of(ui);
    let stats = &node.subtree;

    ui.label(RichText::new(lang.pick("Summary", "요약")).strong());
    egui::Grid::new("url-map-stats-grid")
        .num_columns(2)
        .spacing([Space::MD, Space::XS])
        .show(ui, |ui| {
            ui.weak(lang.pick("Requests", "요청"));
            let req_text = if node.direct.request_count > 0
                && node.direct.request_count != stats.request_count
            {
                match lang {
                    Language::English => format!(
                        "{} total ({} direct, {retained} retained, {evicted} evicted)",
                        stats.request_count, node.direct.request_count
                    ),
                    Language::Korean => format!(
                        "총 {}건 (직접 {}건, 보관 {retained}건, 제거 {evicted}건)",
                        stats.request_count, node.direct.request_count
                    ),
                }
            } else if evicted > 0 {
                match lang {
                    Language::English => format!(
                        "{} total ({retained} retained, {evicted} evicted)",
                        stats.request_count
                    ),
                    Language::Korean => format!(
                        "총 {}건 (보관 {retained}건, 제거 {evicted}건)",
                        stats.request_count
                    ),
                }
            } else {
                format!("{}", stats.request_count)
            };
            ui.add(egui::Label::new(req_text).wrap());
            ui.end_row();

            ui.weak(lang.pick("Methods", "메서드"));
            let methods = stats.methods.names().join(", ");
            ui.add(egui::Label::new(if methods.is_empty() { "—" } else { &methods }).wrap());
            ui.end_row();

            ui.weak(lang.pick("Status", "상태"));
            ui.horizontal_wrapped(|ui| {
                render_status_counts(ui, stats, &palette, lang);
            });
            ui.end_row();

            ui.weak("Content-Type");
            ui.add(egui::Label::new(format_content_types(stats)).wrap());
            ui.end_row();

            ui.weak(lang.pick("Transferred", "전송량"));
            ui.label(ByteSize::new(stats.total_bytes).to_string());
            ui.end_row();

            ui.weak(lang.pick("Response time", "응답 시간"));
            let timing_text = match (stats.avg_duration_ms(), stats.max_duration_ms()) {
                (Some(avg), Some(max)) => match lang {
                    Language::English => format!("avg {avg:.1} ms · max {max:.1} ms"),
                    Language::Korean => format!("평균 {avg:.1} ms · 최대 {max:.1} ms"),
                },
                _ => "—".to_owned(),
            };
            ui.label(timing_text);
            ui.end_row();

            ui.weak(lang.pick("Observed", "관측 시각"));
            ui.label(format!(
                "{} – {}",
                format_wall_time(stats.first_seen),
                format_wall_time(stats.last_seen),
            ));
            ui.end_row();
        });
}

fn render_status_counts(ui: &mut egui::Ui, stats: &NodeStats, palette: &Palette, lang: Language) {
    let buckets = [
        ("2xx", stats.status.s2xx, palette.text),
        ("3xx", stats.status.s3xx, palette.text),
        ("4xx", stats.status.s4xx, palette.warning),
        ("5xx", stats.status.s5xx, palette.error),
        (
            lang.pick("Failed", "실패"),
            stats.status.failed,
            palette.error,
        ),
        (
            lang.pick("Tunnel", "터널"),
            stats.status.tunnel,
            palette.text,
        ),
        (
            lang.pick("Pending", "대기 중"),
            stats.status.pending,
            palette.secondary,
        ),
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

fn render_query_params_section(ui: &mut egui::Ui, node: &ViewNode, lang: Language) {
    ui.label(
        RichText::new(lang.pick(
            "Query Parameters (names only)",
            "쿼리 매개변수 (이름만 표시)",
        ))
        .strong(),
    );
    let params = if node.direct.query_params.is_empty() {
        &node.subtree.query_params
    } else {
        &node.direct.query_params
    };
    if params.is_empty() {
        ui.weak(lang.pick(
            "No query parameters observed.",
            "관측된 쿼리 매개변수가 없습니다.",
        ));
        return;
    }
    let mut pairs: Vec<(&String, &u64)> = params.iter().collect();
    pairs.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    egui::Grid::new("url-map-query-params")
        .num_columns(2)
        .spacing([Space::LG, Space::XXS])
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
    lang: Language,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    let total_retained = cache.matched_count();
    let heading = match lang {
        Language::English => format!("Requests under this node ({total_retained})"),
        Language::Korean => format!("이 노드 아래의 요청 ({total_retained})"),
    };
    ui.label(RichText::new(heading).strong());
    if evicted > 0 {
        let note = match lang {
            Language::English => format!(
                "{evicted} older request(s) were evicted from the screen buffer; aggregates above still include them."
            ),
            Language::Korean => format!(
                "오래된 요청 {evicted}건이 화면 버퍼에서 제거되었지만 위 집계에는 계속 포함됩니다."
            ),
        };
        ui.label(RichText::new(note).color(palette.warning).small());
    }
    if total_retained == 0 {
        ui.weak(lang.pick(
            "No retained requests remain for this node.",
            "이 노드에 남아 있는 보관 요청이 없습니다.",
        ));
        return None;
    }

    let mut action = None;
    for (idx, preview) in &cache.newest_previews {
        let Some(record) = records.get(*idx) else {
            continue;
        };
        let (status_label, status_tone) = outcome_text(record.outcome(), lang);
        let status_color = palette.status(status_tone);
        let safe_path = preview.display(lang);
        ui.horizontal(|ui| {
            ui.monospace(record.request().method().as_str());
            ui.label(RichText::new(status_label).color(status_color).small());
            let link = ui.add(
                egui::Label::new(RichText::new(safe_path.as_str()).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .sense(egui::Sense::click()),
            );
            if link
                .on_hover_text(lang.pick(
                    "Open this request in Requests view.",
                    "이 요청을 요청 목록에서 엽니다.",
                ))
                .clicked()
            {
                action = Some(MapAction::SelectRequest(record.id()));
            }
        });
    }
    if total_retained > MAX_INLINE_REQUESTS {
        let more = match lang {
            Language::English => format!(
                "Showing newest {MAX_INLINE_REQUESTS} of {total_retained} retained requests. Use Show in Requests to inspect all."
            ),
            Language::Korean => format!(
                "보관된 요청 {total_retained}건 중 최신 {MAX_INLINE_REQUESTS}건만 표시합니다. 전체를 보려면 '요청 목록에서 보기'를 사용하세요."
            ),
        };
        ui.weak(more);
    }
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
