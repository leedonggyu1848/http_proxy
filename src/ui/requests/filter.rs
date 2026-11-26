//! Request filtering controls above the list, backed by
//! [`http_inspector_analysis::filter`].

use eframe::egui;
use http_inspector_analysis::filter::{
    self as analysis_filter, RecordFilter, RecordList, ScopeFilter,
};
use http_inspector_analysis::pipeline::AnalysisPipeline;
use http_inspector_analysis::url_map::RequestScope;
use http_inspector_exchange::{Exchange, RecordId};
use std::net::SocketAddr;

use super::request_table::RequestTable;
use crate::ui::AppAction;
use crate::ui::app::state::RequestsState;
use crate::ui::detail::search::RequestSearch;
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::map::RecordMatchCache;
use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

pub(crate) struct ListView<'a> {
    pub(crate) requests: &'a RequestsState,
    pub(crate) analysis: &'a AnalysisPipeline,
    pub(crate) proxy: Option<SocketAddr>,
    pub(crate) saving_body: bool,
    pub(crate) lang: Language,
}

impl ListView<'_> {
    /// Filter, list options, and Clear, shown right above the request list.
    pub(crate) fn filters(
        &self,
        ui: &mut egui::Ui,
        table: &mut RequestTable,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.lang;
        ui.horizontal_wrapped(|ui| {
            actions.extend(
                self.requests
                    .search
                    .controls(ui, lang)
                    .map(AppAction::Search),
            );
            let mut filter_choice = self.requests.record_filter;
            egui::ComboBox::from_id_salt("record-filter")
                .selected_text(domain::record_filter(lang, self.requests.record_filter))
                .show_ui(ui, |ui| {
                    for filter in RecordFilter::ALL {
                        ui.selectable_value(
                            &mut filter_choice,
                            filter,
                            domain::record_filter(lang, filter),
                        );
                    }
                });
            if filter_choice != self.requests.record_filter {
                actions.push(AppAction::Filter(filter_choice));
            }
            table.controls(ui, lang);
            if ui
                .button(lang.text(Text::Clear))
                .on_hover_text(lang.pick(
                    "Clear the displayed requests and reset the URL map while retaining the full-session log.",
                    "전체 세션 로그는 유지한 채 표시된 요청 목록과 URL 맵을 비웁니다.",
                ))
                .clicked()
            {
                actions.push(AppAction::ClearSession);
            }
        });
        self.map_filter_banner(ui, actions);
        self.requests.search.status(ui, lang);
    }

    fn map_filter_banner(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let Some(key) = self.requests.map_path_filter.as_ref() else {
            return;
        };
        let lang = self.lang;
        let retained = self.requests.map_filter_cache.matched_count();
        let counted = self.map_scope_total(key);
        let evicted = counted.saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));
        let approximate = key.method.is_some() && self.requests.records.evicted() > 0;
        ui.horizontal_wrapped(|ui| {
            let prefix = lang.pick("Map filter", "맵 필터");
            ui.label(egui::RichText::new(format!("{prefix}: {}", key.display())).monospace());
            let retention = map_retention_label(retained, evicted, approximate, lang);
            // The filter row is chrome, where warning text is not readable,
            // so a dot marks counts that are partial.
            let count = if evicted > 0 || approximate {
                indicator::dot_label(ui, Tone::Warning, retention)
            } else {
                ui.label(egui::RichText::new(retention).weak())
            };
            if approximate {
                count.on_hover_text(lang.pick(
                    "Estimate based on requests included in endpoint analysis.",
                    "엔드포인트 분석에 포함된 요청을 기준으로 추정한 값입니다.",
                ));
            }
            if ui
                .small_button(lang.pick("Clear map filter", "맵 필터 해제"))
                .clicked()
            {
                actions.push(AppAction::ClearMapFilter);
            }
        });
    }

    /// Requests the Map counted for `scope` since the last `Clear`.
    fn map_scope_total(&self, scope: &RequestScope) -> u64 {
        scope.method.as_ref().map_or_else(
            || self.analysis.url_map().matching_request_count(&scope.key),
            |method| {
                self.analysis.endpoint_schemas().request_count(
                    &self.analysis.url_map().matching_node_ids(&scope.key),
                    method,
                )
            },
        )
    }

    pub(crate) fn table(
        &self,
        ui: &mut egui::Ui,
        table: &mut RequestTable,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) -> Option<RecordId> {
        let requests = self.requests;
        let rows = visible_records(
            &requests.records,
            requests.record_filter,
            &requests.search,
            requests.map_path_filter.as_ref(),
            Some(&requests.map_filter_cache),
        );
        if rows.is_empty() {
            let message = empty_list_text(requests.records.len(), self.proxy, self.lang);
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(message).weak()).wrap());
            });
            return None;
        }
        let result = table.show(
            ui,
            rows,
            super::request_table::TableViewOptions {
                selected: requests.selected,
                shortcuts_allowed,
                saving_body: self.saving_body,
                lang: self.lang,
            },
        );
        actions.extend(result.selected.map(AppAction::SelectRecord));
        actions.extend(result.action);
        result.selected
    }
}

fn map_retention_label(retained: usize, evicted: u64, approximate: bool, lang: Language) -> String {
    match lang {
        Language::English => {
            if approximate {
                format!("({retained} retained · ≈{evicted} older removed)")
            } else if evicted > 0 {
                format!("({retained} retained · {evicted} older removed)")
            } else {
                format!("({retained} matching)")
            }
        }
        Language::Korean => {
            if approximate {
                format!("({retained}건 유지 · 오래된 기록 ≈{evicted}건 정리됨)")
            } else if evicted > 0 {
                format!("({retained}건 유지 · 오래된 기록 {evicted}건 정리됨)")
            } else {
                format!("({retained}건 일치)")
            }
        }
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

/// One line for a list without rows: why it is empty and, while capturing,
/// the proxy address to point a client at.
fn empty_list_text(captured: usize, proxy: Option<std::net::SocketAddr>, lang: Language) -> String {
    match proxy {
        _ if captured > 0 => lang
            .pick(
                "No requests match the filter",
                "필터와 일치하는 요청이 없습니다",
            )
            .to_owned(),
        Some(address) => match lang {
            Language::English => format!("No requests yet · proxy http://{address}"),
            Language::Korean => format!("요청 없음 · 프록시 http://{address}"),
        },
        None => lang
            .pick("No requests · proxy stopped", "요청 없음 · 프록시 중지됨")
            .to_owned(),
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
            map_retention_label(5, 0, true, Language::English),
            "(5 retained · ≈0 older removed)"
        );
        assert_eq!(
            map_retention_label(5, 2, true, Language::English),
            "(5 retained · ≈2 older removed)"
        );
        assert_eq!(
            map_retention_label(5, 2, false, Language::English),
            "(5 retained · 2 older removed)"
        );
        assert_eq!(
            map_retention_label(5, 0, false, Language::English),
            "(5 matching)"
        );
        assert_eq!(
            map_retention_label(5, 0, false, Language::Korean),
            "(5건 일치)"
        );
    }

    #[test]
    fn empty_list_explains_the_next_step() {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 18080));
        assert!(
            empty_list_text(0, Some(address), Language::English).contains("http://127.0.0.1:18080")
        );
        assert!(empty_list_text(0, None, Language::English).contains("stopped"));
        assert!(empty_list_text(3, Some(address), Language::English).contains("match the filter"));
        assert!(
            empty_list_text(0, Some(address), Language::Korean).contains("http://127.0.0.1:18080")
        );
        assert!(empty_list_text(0, None, Language::Korean).contains("중지됨"));
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
