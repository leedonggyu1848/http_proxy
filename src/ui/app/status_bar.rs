//! The status bar: counts, background work, and capture limits in one
//! [`Layout::STATUS_BAR_HEIGHT`] row of monospace secondary text (D22).
//!
//! Every value here appears nowhere else on screen (S16): the view switch
//! and the outcome tabs carry no counts. When the row is too narrow, the
//! least important entries give way to a `+N` entry (see
//! [`crate::ui::workbench::status`]).

use eframe::egui;
use http_inspector_analysis::filter::{self as analysis_filter, RecordFilter};
use http_inspector_core::recording::FullLog;
use http_inspector_exchange::ByteSize;

use super::state::MainView;
use super::{AppAction, InspectorApp, MAX_RECORDS};
use crate::ui::i18n::{Language, domain};
use crate::ui::map::MapSubView;
use crate::ui::style::tokens::{Layout, Tone};
use crate::ui::workbench::chrome;
use crate::ui::workbench::status::{self, StatusItem};
use crate::ui::workbench::text::{self, ShellText};

/// Share of the row kept for save results and errors after the entries.
const RESULT_SHARE: f32 = 0.4;

impl InspectorApp {
    pub(super) fn status_bar(
        &self,
        ui: &mut egui::Ui,
        shown: Option<usize>,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.language();
        chrome::band(
            ui,
            "status-bar",
            Layout::STATUS_BAR_HEIGHT,
            chrome::Edge::Top,
            chrome::Inset::BAR,
            |ui| {
                chrome::status_text(ui);
                let mut items = self.counts(shown, lang);
                items.extend(self.limits(lang));
                let results = self.dialogs.save.has_status()
                    || self.session.body_saving.has_status()
                    || self.layout.theme.has_error()
                    || self.layout.language.has_error();
                let available = if results {
                    ui.available_width() * (1.0 - RESULT_SHARE)
                } else {
                    ui.available_width()
                };
                status::show(ui, &items, available);
                self.dialogs.save.status(ui, lang);
                if self.session.body_saving.status(ui, lang) {
                    actions.push(AppAction::CancelBulkSave);
                }
                self.layout.theme.status(ui, lang);
                self.layout.language.status(ui);
            },
        );
    }

    /// Requests and failures, hosts and nodes in the Map tree, or domains
    /// and edges in the Map graph.
    fn counts(&self, shown: Option<usize>, lang: Language) -> Vec<StatusItem> {
        let records = &self.requests.records;
        let mut items = Vec::new();
        match self.layout.main_view {
            MainView::Requests => {
                // The request area counted the rows it showed this frame.
                let shown = shown.unwrap_or(records.len());
                items.push(StatusItem::count(text::request_count(
                    lang,
                    shown,
                    records.len(),
                )));
                // Outcomes change in place without a new list revision, so
                // failures are counted again each frame; the list holds at
                // most `MAX_RECORDS`.
                let failed = records
                    .iter()
                    .filter(|record| {
                        analysis_filter::record_matches(record, RecordFilter::Failures)
                    })
                    .count();
                if failed > 0 {
                    items.push(StatusItem::count(text::failure_count(lang, failed)));
                }
                items.extend(self.requests.search.status_items(lang));
            }
            MainView::Map => {
                let map = self.map.analysis.url_map();
                items.push(StatusItem::count(text::request_count(
                    lang,
                    records.len(),
                    records.len(),
                )));
                if self.map.panel.sub_view == MapSubView::Graph {
                    let (summary, capped) = self.map.panel.graph_summary(lang);
                    items.push(if capped {
                        StatusItem::marked(
                            Tone::Warning,
                            summary,
                            ShellText::DomainLimitHint.text(lang),
                        )
                    } else {
                        StatusItem::count(summary)
                    });
                } else {
                    items.push(StatusItem::count(text::map_count(
                        lang,
                        map.origins().len(),
                        map.node_count(),
                    )));
                }
            }
        }
        items
    }

    /// Records the capture could not keep, and logging limits.
    fn limits(&self, lang: Language) -> Vec<StatusItem> {
        let mut items = Vec::new();
        let dropped = self
            .requests
            .records
            .dropped(self.session.capture.engine_dropped());
        if dropped > 0 {
            items.push(StatusItem::marked(
                Tone::Danger,
                text::dropped(lang, dropped),
                text::dropped_hint(lang),
            ));
        }
        let evicted = self.requests.records.evicted();
        if evicted > 0 {
            items.push(StatusItem::marked(
                Tone::Warning,
                text::older_removed(lang, evicted),
                text::older_removed_hint(lang, MAX_RECORDS),
            ));
        }
        let map = self.map.analysis.url_map();
        if map.node_limit_reached() {
            items.push(StatusItem::marked(
                Tone::Warning,
                ShellText::UrlMapLimit.text(lang),
                text::url_map_limit_hint(lang, map.max_nodes()),
            ));
        }
        let log_error = self
            .session
            .capture
            .log_init_error
            .as_ref()
            .map(|error| domain::full_log_error(lang, error))
            .or_else(|| {
                self.session
                    .capture
                    .full_log
                    .as_ref()
                    .and_then(FullLog::error)
                    .map(|raw| domain::localize_full_log_runtime_error(lang, &raw))
            });
        if let Some(error) = log_error {
            items.push(StatusItem::marked(
                Tone::Danger,
                ShellText::FullLogUnavailable.text(lang),
                error,
            ));
        }
        if let Some(log) = self
            .session
            .capture
            .full_log
            .as_ref()
            .filter(|log| log.body_limit_reached())
        {
            items.push(StatusItem::marked(
                Tone::Warning,
                ShellText::LogBodyLimit.text(lang),
                text::log_body_limit_hint(
                    lang,
                    &ByteSize::new(log.body_limit()).exact().to_string(),
                ),
            ));
        }
        items
    }
}
