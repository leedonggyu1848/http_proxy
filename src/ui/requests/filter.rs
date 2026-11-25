//! Request filtering controls above the list, backed by
//! [`http_inspector_analysis::filter`].

use eframe::egui;
use http_inspector_analysis::filter::{
    self as analysis_filter, RecordFilter, RecordList, ScopeFilter,
};
use http_inspector_analysis::url_map::RequestScope;
use http_inspector_exchange::Exchange;

use crate::ui::InspectorApp;
use crate::ui::detail::search::RequestSearch;
use crate::ui::map::RecordMatchCache;
use crate::ui::style::theme::Palette;

impl InspectorApp {
    /// Filter, list options, and Clear, shown right above the request list.
    pub(crate) fn filters(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            self.requests.search.controls(ui);
            egui::ComboBox::from_id_salt("record-filter")
                .selected_text(self.requests.record_filter.label())
                .show_ui(ui, |ui| {
                    for filter in RecordFilter::ALL {
                        ui.selectable_value(&mut self.requests.record_filter, filter, filter.label());
                    }
                });
            self.requests.table.controls(ui);
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
        self.requests.search.status(ui);
    }

    fn map_filter_banner(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.requests.map_path_filter.clone() else {
            return;
        };
        self.requests
            .map_filter_cache
            .sync(&key, &self.requests.records, false);
        let palette = Palette::of(ui);
        let retained = self.requests.map_filter_cache.matched_count();
        let counted = self.map_scope_total(&key);
        let evicted = counted.saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));
        let approximate = key.method.is_some() && self.requests.records.evicted() > 0;
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("Map filter: {}", key.display()))
                    .monospace()
                    .color(palette.info),
            );
            let color = if evicted > 0 || approximate {
                palette.warning
            } else {
                palette.muted
            };
            let count = ui.label(
                egui::RichText::new(map_retention_label(retained, evicted, approximate))
                    .color(color)
                    .small(),
            );
            if approximate {
                count.on_hover_text("Estimate based on requests included in endpoint analysis.");
            }
            if ui.small_button("Clear map filter").clicked() {
                clear = true;
            }
        });
        if clear {
            self.requests.clear_map_filter();
        }
    }

    /// Requests the Map counted for `scope` since the last `Clear`.
    fn map_scope_total(&self, scope: &RequestScope) -> u64 {
        scope.method.as_ref().map_or_else(
            || {
                self.map
                    .analysis
                    .url_map()
                    .matching_request_count(&scope.key)
            },
            |method| {
                self.map.analysis.endpoint_schemas().request_count(
                    &self.map.analysis.url_map().matching_node_ids(&scope.key),
                    method,
                )
            },
        )
    }

    pub(crate) fn request_table(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let requests = &mut self.requests;
        requests.update_filters(ui.ctx());
        let rows = visible_records(
            &requests.records,
            requests.record_filter,
            &requests.search,
            requests.map_path_filter.as_ref(),
            Some(&requests.map_filter_cache),
        );
        if rows.is_empty() {
            let message = empty_list_text(
                requests.records.len(),
                self.session
                    .proxy
                    .as_ref()
                    .map(http_inspector_core::interception::ProxyHandle::local_addr),
            );
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(message).weak()).wrap());
            });
            return;
        }
        requests.table.saving_body = self.session.body_saving.is_busy();
        if let Some(selected) = requests
            .table
            .show(ui, rows, requests.selected, shortcuts_allowed)
        {
            requests.selected = Some(selected);
        }
        if let Some(record) = requests.table.copy_curl.take() {
            self.detail.curl.copy(ui.ctx(), &record);
        }
        if let Some((record, side)) = requests.table.save_body.take() {
            let directory = self.dialogs.save.directory();
            self.session.body_saving.request(
                &record,
                side,
                http_inspector_core::recording::SaveFormat::Decoded,
                directory,
            );
        }
        if let Some(original) = requests.table.replay.take()
            && let Err(error) = self.detail.open_replay(original)
        {
            self.session.error = Some(error.to_string());
        }
    }
}

fn map_retention_label(retained: usize, evicted: u64, approximate: bool) -> String {
    if approximate {
        format!("({retained} retained · ≈{evicted} older removed)")
    } else if evicted > 0 {
        format!("({retained} retained · {evicted} older removed)")
    } else {
        format!("({retained} matching)")
    }
}

/// Records that match the outcome filter, optional Map node prefix, and search, in arrival order.
pub(crate) fn visible_records<'a>(
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
    use http_inspector_analysis::search::{MatchOptions, SearchScope};
    use http_inspector_exchange::BodyLifecycle;
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    use super::*;

    #[test]
    fn endpoint_evictions_are_marked_as_estimates_even_at_zero() {
        assert_eq!(
            map_retention_label(5, 0, true),
            "(5 retained · ≈0 older removed)"
        );
        assert_eq!(
            map_retention_label(5, 2, true),
            "(5 retained · ≈2 older removed)"
        );
        assert_eq!(
            map_retention_label(5, 2, false),
            "(5 retained · 2 older removed)"
        );
        assert_eq!(map_retention_label(5, 0, false), "(5 matching)");
    }

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
            "a".repeat(http_inspector_exchange::capture::BODY_PREVIEW_BYTES + 100)
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
