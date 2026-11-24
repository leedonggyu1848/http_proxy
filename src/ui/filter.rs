//! Request filtering controls above the list, backed by
//! [`http_proxy::analysis::filter`].

use eframe::egui;
use http_proxy::analysis::filter::{
    self as analysis_filter, RecordFilter, RecordList, ScopeFilter,
};
use http_proxy::analysis::url_map::RequestScope;
use http_proxy::exchange::Exchange;

use crate::ui::InspectorApp;
use crate::ui::map_panel::RecordMatchCache;
use crate::ui::replay_panel;
use crate::ui::search::RequestSearch;
use crate::ui::theme::Palette;

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
            || self.analysis.url_map().matching_request_count(&scope.key),
            |method| {
                self.analysis.endpoint_schemas().request_count(
                    &self.analysis.url_map().matching_node_ids(&scope.key),
                    method,
                )
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
                self.proxy
                    .as_ref()
                    .map(http_proxy::interception::ProxyHandle::local_addr),
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
            self.body_saving.request(
                &record,
                side,
                http_proxy::recording::SaveFormat::Decoded,
                directory,
            );
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
    let scope = map_filter.map(|key| {
        map_cache
            .and_then(|cache| cache.fresh_ids(key, records))
            .map_or(ScopeFilter::Live(key), ScopeFilter::Cached)
    });
    analysis_filter::visible_records(records, filter, scope, |id| search.shows(id))
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
    use std::io::Write as _;

    use http::header::{CONTENT_ENCODING, CONTENT_TYPE};
    use http::{HeaderValue, Method, StatusCode};
    use http_proxy::analysis::search::{MatchOptions, SearchScope};
    use http_proxy::exchange::BodyLifecycle;
    use http_proxy::interception::{ExchangeBuilder, ResponseFixture};

    use super::*;

    #[test]
    fn empty_list_explains_the_next_step() {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 18080));
        assert!(empty_list_text(0, Some(address)).contains("http://127.0.0.1:18080"));
        assert!(empty_list_text(0, None).contains("Start"));
        assert!(empty_list_text(3, Some(address)).contains("match the filter"));
    }

    #[test]
    fn request_search_filters_decoded_bodies() -> Result<(), Box<dyn std::error::Error>> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(br#"{"error":"Quota Exceeded"}"#)?;
        let gzip = encoder.finish()?;

        let record = ExchangeBuilder::http(Method::POST, "http://api.example.test/submit")?
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .request_body("orderId=REQ-123", BodyLifecycle::Complete)
            .respond(
                ResponseFixture::new(StatusCode::TOO_MANY_REQUESTS)
                    .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                    .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                    .body(gzip, BodyLifecycle::Complete),
            )?;

        let mut records = RecordList::new(4);
        let id = record.id();
        let _evicted = records.push(record);
        let context = egui::Context::default();
        let mut search = RequestSearch::new(&context);
        let options = MatchOptions::default();
        search.search_now(("quota", SearchScope::All, options), &records, &context);
        assert!(search.shows(id));
        search.search_now(
            ("quota exceeded!", SearchScope::All, options),
            &records,
            &context,
        );
        assert!(!search.shows(id));
        search.search_now(("", SearchScope::All, options), &records, &context);
        assert!(search.shows(id));
        Ok(())
    }

    #[test]
    fn request_search_counts_partially_searched_previews() -> Result<(), Box<dyn std::error::Error>>
    {
        let body = format!(
            "{}tail-word",
            "a".repeat(http_proxy::interception::BODY_PREVIEW_BYTES + 100)
        );
        let record = ExchangeBuilder::http(Method::GET, "http://api.example.test/large")?.respond(
            ResponseFixture::new(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
                .body(body, BodyLifecycle::Complete),
        )?;

        let mut records = RecordList::new(4);
        let id = record.id();
        let _evicted = records.push(record);
        let context = egui::Context::default();
        let mut search = RequestSearch::new(&context);
        search.search_now(
            ("tail-word", SearchScope::All, MatchOptions::default()),
            &records,
            &context,
        );
        assert!(!search.shows(id));
        assert_eq!(search.partially_searched(), 1);
        Ok(())
    }
}
