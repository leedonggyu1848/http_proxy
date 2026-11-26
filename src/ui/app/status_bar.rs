//! The status bar: counts, background work, and capture limits in one
//! [`Layout::STATUS_BAR_HEIGHT`] row of monospace secondary text (D22).
//!
//! Every value here appears nowhere else on screen (S16): the view switch
//! and the outcome tabs carry no counts.

use eframe::egui;
use http_inspector_analysis::filter::{self as analysis_filter, RecordFilter};
use http_inspector_core::recording::FullLog;
use http_inspector_exchange::ByteSize;

use super::state::MainView;
use super::{AppAction, InspectorApp, MAX_RECORDS};
use crate::ui::style::indicator;
use crate::ui::style::tokens::{Layout, Tone};
use crate::ui::workbench::chrome;
use crate::ui::workbench::text;

impl InspectorApp {
    pub(super) fn status_bar(
        &self,
        ui: &mut egui::Ui,
        shown: Option<usize>,
        actions: &mut Vec<AppAction>,
    ) {
        chrome::band(
            ui,
            "status-bar",
            Layout::STATUS_BAR_HEIGHT,
            chrome::Edge::Top,
            chrome::Inset::BAR,
            |ui| {
                chrome::status_text(ui);
                self.counts(ui, shown);
                self.limits(ui);
                self.dialogs.save.status(ui);
                if self.session.body_saving.status(ui) {
                    actions.push(AppAction::CancelBulkSave);
                }
                self.layout.theme.status(ui);
            },
        );
    }

    /// Requests and failures, or hosts and nodes in the Map view.
    fn counts(&self, ui: &mut egui::Ui, shown: Option<usize>) {
        let records = &self.requests.records;
        match self.layout.main_view {
            MainView::Requests => {
                let shown = shown.unwrap_or(records.len());
                ui.label(text::request_count(shown, records.len()));
                let failed = records
                    .iter()
                    .filter(|record| {
                        analysis_filter::record_matches(record, RecordFilter::Failures)
                    })
                    .count();
                if failed > 0 {
                    ui.label(text::failure_count(failed));
                }
                self.requests.search.status(ui);
            }
            MainView::Map => {
                let map = self.map.analysis.url_map();
                ui.label(text::request_count(records.len(), records.len()));
                ui.label(text::map_count(map.origins().len(), map.node_count()));
            }
        }
    }

    /// Records the capture could not keep, and logging limits.
    fn limits(&self, ui: &mut egui::Ui) {
        let dropped = self
            .requests
            .records
            .dropped(self.session.capture.engine_dropped());
        if dropped > 0 {
            indicator::dot_label(ui, Tone::Danger, format!("{dropped} dropped")).on_hover_text(
                "The capture queue was full, so these exchanges were forwarded but not shown.",
            );
        }
        let evicted = self.requests.records.evicted();
        if evicted > 0 {
            indicator::dot_label(ui, Tone::Warning, format!("{evicted} older removed"))
                .on_hover_text(format!(
                    "Only the newest {MAX_RECORDS} requests stay on screen; the full-session log keeps every record."
                ));
        }
        let map = self.map.analysis.url_map();
        if map.node_limit_reached() {
            indicator::dot_label(ui, Tone::Warning, "URL map limit").on_hover_text(format!(
                "The URL map reached {} nodes; deeper paths are folded into parent nodes.",
                map.max_nodes()
            ));
        }
        let log_error = self
            .session
            .capture
            .log_init_error
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| {
                self.session
                    .capture
                    .full_log
                    .as_ref()
                    .and_then(FullLog::error)
            });
        if let Some(error) = log_error {
            indicator::dot_label(ui, Tone::Danger, "Full log unavailable").on_hover_text(error);
        }
        if let Some(log) = self
            .session
            .capture
            .full_log
            .as_ref()
            .filter(|log| log.body_limit_reached())
        {
            indicator::dot_label(ui, Tone::Warning, "Log body limit").on_hover_text(format!(
                "Body bytes after {} are left out of the full-session log; headers and results are still logged.",
                ByteSize::new(log.body_limit()).exact()
            ));
        }
    }
}
