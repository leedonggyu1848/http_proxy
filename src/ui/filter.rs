//! Request filtering by outcome kind, and the controls above the list.

use http_proxy::{Exchange, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum RecordFilter {
    All,
    Responses,
    Failures,
    Tunnels,
}

impl RecordFilter {
    pub(in crate::ui) const ALL: [Self; 4] =
        [Self::All, Self::Responses, Self::Failures, Self::Tunnels];

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Responses => "Responses",
            Self::Failures => "Failures",
            Self::Tunnels => "Tunnels",
        }
    }
}

/// Reports whether `record` has the outcome kind `filter` selects. Text
/// search is applied separately by [`crate::ui::search::RequestSearch`].
pub(in crate::ui) fn record_matches(record: &Exchange, filter: RecordFilter) -> bool {
    let outcome = record.outcome();
    match filter {
        RecordFilter::All => true,
        RecordFilter::Responses => matches!(outcome, Outcome::Response(_)),
        RecordFilter::Failures => outcome.is_failure(),
        RecordFilter::Tunnels => {
            matches!(outcome, Outcome::Tunnel(_) | Outcome::InspectionEstablished)
        }
    }
}

use crate::ui::InspectorApp;
use crate::ui::map_panel::RecordMatchCache;
use crate::ui::records::RecordList;
use crate::ui::replay_panel;
use crate::ui::search::RequestSearch;
use crate::ui::theme::Palette;
use eframe::egui;
use http_proxy::url_map::RequestScope;

impl InspectorApp {
    /// Filter, list options, and Clear, shown right above the request list.
    pub(in crate::ui) fn filters(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            self.search.controls(ui);
            egui::ComboBox::from_id_salt("record-filter")
                .selected_text(self.record_filter.label())
                .show_ui(ui, |ui| {
                    for filter in RecordFilter::ALL {
                        ui.selectable_value(&mut self.record_filter, filter, filter.label());
                    }
                });
            self.table.controls(ui);
            if ui
                .button("Clear")
                .on_hover_text(
                    "Clear the displayed requests and reset the URL map. The full-session log is retained.",
                )
                .clicked()
            {
                self.clear_session();
            }
        });
        self.map_filter_banner(ui);
        self.search.status(ui);
    }

    fn map_filter_banner(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.map_path_filter.clone() else {
            return;
        };
        self.map_filter_cache.sync(&key, &self.records, false);
        let palette = Palette::of(ui);
        let retained = self.map_filter_cache.matched_count();
        let total = self.map_scope_total(&key, retained);
        let evicted = total.saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("Map filter: {}", key.display()))
                    .monospace()
                    .color(palette.info),
            );
            if evicted > 0 {
                ui.label(
                    egui::RichText::new(format!("({retained} retained · {evicted} older removed)"))
                        .color(palette.warning)
                        .small(),
                );
            } else {
                ui.label(
                    egui::RichText::new(format!("({retained} matching)"))
                        .color(palette.muted)
                        .small(),
                );
            }
            if ui.small_button("Clear map filter").clicked() {
                clear = true;
            }
        });
        if clear {
            self.map_path_filter = None;
            self.map_filter_cache.clear();
        }
    }

    /// Requests the Map counted for `scope` since the last `Clear`, never
    /// fewer than the `retained` ones still listed.
    fn map_scope_total(&self, scope: &RequestScope, retained: usize) -> u64 {
        let counted = scope.method.as_ref().map_or_else(
            || self.url_map.matching_request_count(&scope.key),
            |method| {
                self.endpoints
                    .schemas()
                    .request_count(&self.url_map.matching_node_ids(&scope.key), method)
            },
        );
        counted.max(u64::try_from(retained).unwrap_or(u64::MAX))
    }

    pub(in crate::ui) fn request_table(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        self.search.update(&self.records, ui.ctx());
        if let Some(key) = self.map_path_filter.as_ref() {
            self.map_filter_cache.sync(key, &self.records, false);
        }
        let rows = visible_records(
            &self.records,
            self.record_filter,
            &self.search,
            self.map_path_filter.as_ref(),
            Some(&self.map_filter_cache),
        );
        if rows.is_empty() {
            let message = empty_list_text(
                self.records.len(),
                self.proxy.as_ref().map(http_proxy::ProxyHandle::local_addr),
            );
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(message).weak()).wrap());
            });
            return;
        }
        self.table.saving_body = self.body_saving.is_busy();
        if let Some(selected) = self.table.show(ui, rows, self.selected, shortcuts_allowed) {
            self.selected = Some(selected);
        }
        if let Some(record) = self.table.copy_curl.take() {
            self.curl.copy(ui.ctx(), &record);
        }
        if let Some((record, side)) = self.table.save_body.take() {
            let directory = self.save.directory();
            self.body_saving
                .request(&record, side, http_proxy::SaveFormat::Decoded, directory);
        }
        if let Some(original) = self.table.replay.take() {
            match replay_panel::ReplayPanel::new(original) {
                Ok(panel) => self.replay = Some(panel),
                Err(error) => self.error = Some(error),
            }
        }
    }
}

/// Records that match the outcome filter, optional Map node prefix, and search, in arrival order.
pub(in crate::ui) fn visible_records<'a>(
    records: &'a RecordList<Exchange>,
    filter: RecordFilter,
    search: &RequestSearch,
    map_filter: Option<&RequestScope>,
    map_cache: Option<&RecordMatchCache>,
) -> Vec<&'a Exchange> {
    let fresh_cache = map_filter
        .zip(map_cache)
        .filter(|(key, cache)| cache.is_fresh_for(key, records))
        .map(|(_, cache)| cache);
    records
        .iter()
        .filter(|record| record_matches(record, filter))
        .filter(|record| match (map_filter, fresh_cache) {
            (None, _) => true,
            (Some(_), Some(cache)) => cache.contains(record.id()),
            (Some(key), None) => key.matches_exchange(record),
        })
        .filter(|record| search.shows(record.id()))
        .collect()
}

/// Tells the user what to do next when the list shows no rows.
fn empty_list_text(captured: usize, proxy: Option<std::net::SocketAddr>) -> String {
    match proxy {
        _ if captured > 0 => "No requests match the filter.".to_owned(),
        Some(address) => format!(
            "No requests yet.\nSet your client's HTTP proxy to http://{address} or use Open Browser."
        ),
        None => "The proxy is stopped. Press Start to capture requests.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::search::SearchScope;
    use crate::ui::search::document::{self, BodyCache};
    use http_proxy::text_search::{MatchOptions, Matcher};
    use http_proxy::{ProxyConfig, ProxyHandle};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{Duration, timeout};

    #[test]
    fn empty_list_explains_the_next_step() {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 18080));
        assert!(empty_list_text(0, Some(address)).contains("http://127.0.0.1:18080"));
        assert!(empty_list_text(0, None).contains("Start"));
        assert!(empty_list_text(3, Some(address)).contains("match the filter"));
    }

    /// Checks `record` the way the search thread does.
    fn found(record: &Exchange, query: &str, scope: SearchScope, case_sensitive: bool) -> bool {
        let options = MatchOptions {
            case_sensitive,
            regex: false,
        };
        Matcher::new(query, options).is_ok_and(|matcher| {
            document::check(record, &matcher, scope, &mut BodyCache::default()).matched
        })
    }

    #[tokio::test]
    async fn filters_and_search_captured_exchanges() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await?;
            let address = upstream.local_addr()?;
            let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            client.write_all(format!("GET http://{address}/MiXeD HTTP/1.1\r\nHost: {address}\r\nX-Request: RequestVALUE\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let pending = proxy.receive().await.ok_or("missing pending capture")?;
            check_filters(&pending, [true, false, false, false]);
            for query in ["pending", "get", "/mixed", "x-request", "requestvalue"] {
                assert!(found(&pending, query, SearchScope::All, false), "{query}");
            }
            read_request_head(&mut server).await?;
            server.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nX-Response: ResponseVALUE\r\nConnection: close\r\n\r\n").await?;
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await?;
            wait_settled(&pending).await;
            check_filters(&pending, [true, true, false, false]);
            for query in ["201", "x-response", "responsevalue"] {
                assert!(found(&pending, query, SearchScope::All, false), "{query}");
            }
            assert!(!found(&pending, "absent", SearchScope::All, false));

            let mut tunnel_client = TcpStream::connect(proxy.local_addr()).await?;
            tunnel_client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await?;
            let (_tunnel_server, _) = upstream.accept().await?;
            let tunnel = proxy.receive().await.ok_or("missing tunnel capture")?;
            wait_settled(&tunnel).await;
            check_filters(&tunnel, [true, false, false, true]);
            assert!(found(&tunnel, "tunnel", SearchScope::All, false));

            drop(upstream);
            let mut failed_client = TcpStream::connect(proxy.local_addr()).await?;
            failed_client.write_all(format!("GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            let failed = proxy.receive().await.ok_or("missing failure capture")?;
            wait_settled(&failed).await;
            check_filters(&failed, [true, false, true, false]);
            assert!(found(&failed, "connect", SearchScope::All, false));
            proxy.stop()?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        Ok(())
    }

    #[tokio::test]
    async fn searches_bodies_by_scope_and_case() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await?;
            let address = upstream.local_addr()?;
            let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            let body = "orderId=REQ-123";
            client.write_all(format!("POST http://{address}/submit HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer SECRET-TOKEN\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let record = proxy.receive().await.ok_or("missing capture")?;
            read_request_head(&mut server).await?;
            let mut received = vec![0; body.len()];
            server.read_exact(&mut received).await?;
            let gzip = {
                use std::io::Write as _;
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(br#"{"error":"Quota Exceeded"}"#)?;
                encoder.finish()?
            };
            server.write_all(format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", gzip.len()).as_bytes()).await?;
            server.write_all(&gzip).await?;
            drop(server);
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await?;
            while !document::is_settled(&record) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }

            // Only the gzip-compressed response body has this text.
            assert!(found(&record, "quota exceeded", SearchScope::All, false));
            assert!(found(&record, "quota exceeded", SearchScope::ResponseBody, false));
            for scope in [SearchScope::RequestBody, SearchScope::Url, SearchScope::Headers] {
                assert!(!found(&record, "quota exceeded", scope, false), "{scope:?}");
            }
            assert!(!found(&record, "quota exceeded", SearchScope::All, true));
            assert!(found(&record, "Quota Exceeded", SearchScope::All, true));
            assert!(found(&record, "REQ-123", SearchScope::RequestBody, true));
            assert!(!found(&record, "REQ-123", SearchScope::ResponseBody, false));
            assert!(found(&record, "/submit", SearchScope::Url, false));
            assert!(!found(&record, "/submit", SearchScope::Headers, false));
            // Redacted header values are not searchable.
            assert!(found(&record, "authorization", SearchScope::Headers, false));
            assert!(!found(&record, "secret-token", SearchScope::All, false));

            let mut records = crate::ui::records::RecordList::new(4);
            let _evicted = records.push(record.clone());
            let context = eframe::egui::Context::default();
            let mut search = crate::ui::search::RequestSearch::new(&context);
            let options = MatchOptions::default();
            search.search_now(("quota", SearchScope::All, options), &records, &context);
            assert!(search.shows(record.id()));
            search.search_now(("quota exceeded!", SearchScope::All, options), &records, &context);
            assert!(!search.shows(record.id()));
            search.search_now(("", SearchScope::All, options), &records, &context);
            assert!(search.shows(record.id()));
            proxy.stop()?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        Ok(())
    }

    #[tokio::test]
    async fn reports_bodies_searched_only_in_their_preview()
    -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await?;
            let address = upstream.local_addr()?;
            let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            client.write_all(format!("GET http://{address}/large HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let record = proxy.receive().await.ok_or("missing capture")?;
            read_request_head(&mut server).await?;
            // The word sits after the 8 KiB the preview keeps.
            let body = format!("{}tail-word", "a".repeat(http_proxy::BODY_PREVIEW_BYTES + 100));
            server.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
            drop(server);
            client.read_to_end(&mut Vec::new()).await?;
            while !document::is_settled(&record) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }

            let matcher = Matcher::new("tail-word", MatchOptions::default())?;
            let checked = document::check(&record, &matcher, SearchScope::ResponseBody, &mut BodyCache::default());
            assert!(!checked.matched);
            assert!(checked.coverage.partial);
            assert!(found(&record, "aaaa", SearchScope::ResponseBody, false));

            let mut records = crate::ui::records::RecordList::new(4);
            let _evicted = records.push(record.clone());
            let context = eframe::egui::Context::default();
            let mut search = crate::ui::search::RequestSearch::new(&context);
            search.search_now(("tail-word", SearchScope::All, MatchOptions::default()), &records, &context);
            assert!(!search.shows(record.id()));
            assert_eq!(search.partially_searched(), 1);
            proxy.stop()?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        Ok(())
    }

    fn check_filters(record: &Exchange, expected: [bool; 4]) {
        for (filter, expected) in RecordFilter::ALL.into_iter().zip(expected) {
            assert_eq!(
                record_matches(record, filter),
                expected,
                "{filter:?}: {:?}",
                record.outcome()
            );
        }
    }

    /// Reads one request head so the reply cannot arrive before the proxy sends it.
    async fn read_request_head(stream: &mut TcpStream) -> std::io::Result<()> {
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            head.extend_from_slice(&byte);
        }
        Ok(())
    }

    async fn wait_settled(record: &Exchange) {
        while record.outcome().is_in_flight() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}
