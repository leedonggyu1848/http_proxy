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

use super::request_table::{RequestTable, TableMenus};
use crate::ui::AppAction;
use crate::ui::app::state::RequestsState;
use crate::ui::detail::search::RequestSearch;
use crate::ui::map::RecordMatchCache;
use crate::ui::style::indicator;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth, Tone};
use crate::ui::workbench::text::ShellText;
use crate::ui::workbench::{chrome, controls};

pub(crate) struct ListView<'a> {
    pub(crate) requests: &'a RequestsState,
    pub(crate) analysis: &'a AnalysisPipeline,
    pub(crate) proxy: Option<SocketAddr>,
    pub(crate) saving_body: bool,
}

impl ListView<'_> {
    /// The request pane header: outcome tabs on the left and the list's own
    /// actions on the right, then the Map filter row while one is applied.
    pub(crate) fn header(
        &self,
        ui: &mut egui::Ui,
        table: &mut RequestTable,
        actions: &mut Vec<AppAction>,
    ) {
        let fits = chrome::pane_header(ui, "requests-header", |ui| {
            actions.extend(self.outcome_tabs(ui).map(AppAction::Filter));
            let fits = ui.available_width() >= list_actions_width(ui, table) + Space::LG;
            if fits {
                list_actions(ui, table, actions);
            }
            fits
        });
        if !fits {
            // Too narrow for both: the actions take a second header row.
            chrome::pane_header(ui, "requests-actions", |ui| {
                list_actions(ui, table, actions);
            });
        }
        self.map_filter_banner(ui, actions);
    }

    /// All, Responses, Failures, and Tunnels as text tabs, without counts:
    /// the status bar has them (S16).
    fn outcome_tabs(&self, ui: &mut egui::Ui) -> Option<RecordFilter> {
        controls::tab_list(ui, ShellText::RequestOutcome.text(), |ui| {
            let mut chosen = None;
            for filter in RecordFilter::ALL {
                let selected = filter == self.requests.record_filter;
                if controls::text_tab(ui, selected, filter.label()).clicked() && !selected {
                    chosen = Some(filter);
                }
            }
            chosen
        })
    }

    fn map_filter_banner(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let Some(key) = self.requests.map_path_filter.as_ref() else {
            return;
        };
        let retained = self.requests.map_filter_cache.matched_count();
        let counted = self.map_scope_total(key);
        let evicted = counted.saturating_sub(u64::try_from(retained).unwrap_or(u64::MAX));
        let approximate = key.method.is_some() && self.requests.records.evicted() > 0;
        let frame = egui::Frame::new().inner_margin(Layout::PANE_ROW_MARGIN);
        let row = frame.show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let scope = format!("Map filter: {}", key.display());
                ui.add(egui::Label::new(egui::RichText::new(&scope).monospace()).truncate())
                    .on_hover_text(scope);
                let retention = map_retention_label(retained, evicted, approximate);
                // The filter row is chrome, where warning text is not readable,
                // so a dot marks counts that are partial.
                let count = if evicted > 0 || approximate {
                    indicator::dot_label(ui, Tone::Warning, retention)
                } else {
                    ui.label(egui::RichText::new(retention).weak())
                };
                if approximate {
                    count
                        .on_hover_text("Estimate based on requests included in endpoint analysis.");
                }
                if ui.small_button("Clear map filter").clicked() {
                    actions.push(AppAction::ClearMapFilter);
                }
            })
        });
        ui.painter().hline(
            ui.max_rect().x_range(),
            row.response.rect.bottom(),
            egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle),
        );
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
        rows: Vec<&Exchange>,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) -> Option<RecordId> {
        let requests = self.requests;
        if rows.is_empty() {
            let message = empty_list_text(requests.records.len(), self.proxy);
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(message).weak()).wrap());
            });
            return None;
        }
        let result = table.show(
            ui,
            rows,
            requests.selected,
            shortcuts_allowed,
            TableMenus {
                saving_body: self.saving_body,
                filter_enabled: self.requests.search.field_menu_enabled(),
            },
        );
        actions.extend(result.selected.map(AppAction::SelectRecord));
        actions.extend(result.action);
        result.selected
    }
}

/// Follow newest, Arrival order, and Clear, right-aligned in the header.
fn list_actions(ui: &mut egui::Ui, table: &mut RequestTable, actions: &mut Vec<AppAction>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        controls::ghost_style(ui);
        if ui
            .add(controls::ghost(CLEAR_LABEL, Layout::PANE_BUTTON_HEIGHT))
            .on_hover_text(
                "Clear the listed requests and the URL map; the full-session log is kept.",
            )
            .clicked()
        {
            actions.push(AppAction::ClearSession);
        }
        table.controls(ui);
    });
}

const CLEAR_LABEL: &str = "Clear";

/// Width [`list_actions`] takes.
fn list_actions_width(ui: &egui::Ui, table: &RequestTable) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    let mut width = controls::button_width(ui, CLEAR_LABEL)
        + controls::checkbox_width(ui, super::request_table::FOLLOW_NEWEST)
        + spacing;
    if table.is_sorted() {
        width += controls::button_width(ui, super::request_table::ARRIVAL_ORDER) + spacing;
    }
    width
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
    analysis_filter::visible_records(records, filter, scope, |record| search.matches(record))
}

/// One line for a list without rows: why it is empty and, while capturing,
/// the proxy address to point a client at.
fn empty_list_text(captured: usize, proxy: Option<std::net::SocketAddr>) -> String {
    match proxy {
        _ if captured > 0 => "No requests match the filter".to_owned(),
        Some(address) => format!("No requests yet · proxy http://{address}"),
        None => "No requests · proxy stopped".to_owned(),
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
        assert!(empty_list_text(0, None).contains("stopped"));
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
