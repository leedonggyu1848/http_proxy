use std::sync::Arc;

use eframe::egui;
use http_inspector_analysis::filter::{RecordFilter, RecordList};
use http_inspector_analysis::pipeline::AnalysisPipeline;
use http_inspector_analysis::url_map::RequestScope;
use http_inspector_content::format::ViewMode;
use http_inspector_core::interception::ProxyHandle;
use http_inspector_core::recording::PreviewBodies;
use http_inspector_core::replay::ReplayError;
use http_inspector_exchange::{BodySource, Exchange, RecordId};

use crate::ui::detail::detail_tabs::DetailTab;
use crate::ui::detail::{detail_view::DetailWidgets, search::RequestSearch};
use crate::ui::dialogs::{
    body_saving::BodySaving,
    browser_panel::BrowserPanel,
    ca_panel::CaPanel,
    copy_curl::CurlCopy,
    har_dialog::HarDialog,
    replay_panel::{ReplayAction, ReplayPanel},
    save::SaveControls,
};
use crate::ui::map::{MapPanel, RecordMatchCache};
use crate::ui::requests::filter;
use crate::ui::requests::toolbar::AuthorizationNotice;
use crate::ui::style::theme::ThemeSettings;

use crate::ui::style::tokens::Layout;

use super::MAX_RECORDS;
use super::capture::CaptureSession;

pub(crate) struct SessionState {
    pub(crate) capture: CaptureSession,
    pub(crate) body_saving: BodySaving,
    pub(crate) error: Option<String>,
}

impl SessionState {
    pub(super) fn new() -> Self {
        Self {
            capture: CaptureSession::new(),
            body_saving: BodySaving::open(),
            error: None,
        }
    }

    pub(super) fn body_source(&self) -> Arc<dyn BodySource> {
        match self.body_saving.store() {
            Some(store) => Arc::new(store.clone()),
            None => Arc::new(PreviewBodies),
        }
    }
}

pub(crate) struct RequestsState {
    pub(crate) records: RecordList<Exchange>,
    pub(crate) record_filter: RecordFilter,
    pub(crate) search: RequestSearch,
    pub(crate) selected: Option<RecordId>,
    pub(crate) map_path_filter: Option<RequestScope>,
    pub(crate) map_filter_cache: RecordMatchCache,
}

impl RequestsState {
    pub(super) fn new(context: &egui::Context) -> Self {
        Self {
            records: RecordList::new(MAX_RECORDS),
            record_filter: RecordFilter::All,
            search: RequestSearch::new(context),
            selected: None,
            map_path_filter: None,
            map_filter_cache: RecordMatchCache::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn record_added(&mut self, record: Exchange) -> Option<Exchange> {
        super::capture::add_record(&mut self.records, &mut self.selected, record)
    }

    pub(super) fn clear(&mut self, engine_dropped: u64) {
        self.records.clear(engine_dropped);
        self.selected = None;
        self.clear_map_filter();
    }

    pub(crate) fn update_filters(&mut self, context: &egui::Context) {
        self.search.update(&self.records, context);
        if let Some(scope) = self.map_path_filter.as_ref() {
            self.map_filter_cache.sync(scope, &self.records, false);
        }
    }

    pub(super) fn has_live_records(&self) -> bool {
        self.records.iter().any(Exchange::is_live)
    }

    pub(super) fn show_map_scope(&mut self, scope: RequestScope) {
        self.map_filter_cache.sync(&scope, &self.records, false);
        self.map_path_filter = Some(scope);
        self.selected = None;
    }

    pub(crate) fn clear_map_filter(&mut self) {
        self.map_path_filter = None;
        self.map_filter_cache.clear();
    }

    pub(crate) fn filtered_records(&self) -> Vec<&Exchange> {
        filter::visible_records(
            &self.records,
            self.record_filter,
            &self.search,
            self.map_path_filter.as_ref(),
            Some(&self.map_filter_cache),
        )
    }
}

pub(crate) struct DetailState {
    pub(crate) tab: DetailTab,
    pub(crate) body_mode: ViewMode,
    pub(crate) widgets: DetailWidgets,
    pub(crate) curl: CurlCopy,
    replay: Option<ReplayPanel>,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            tab: DetailTab::Headers,
            body_mode: ViewMode::default(),
            widgets: DetailWidgets::default(),
            curl: CurlCopy::default(),
            replay: None,
        }
    }
}

impl DetailState {
    pub(crate) fn open_replay(&mut self, original: Exchange) -> Result<(), ReplayError> {
        self.replay = Some(ReplayPanel::new(original)?);
        Ok(())
    }

    pub(super) const fn replay_is_open(&self) -> bool {
        self.replay.is_some()
    }

    pub(super) fn show_replay(
        &mut self,
        context: &egui::Context,
        proxy_running: bool,
        records: &RecordList<Exchange>,
    ) -> Vec<ReplayAction> {
        let Some(panel) = self.replay.as_mut() else {
            return Vec::new();
        };
        let latest = records
            .iter()
            .rev()
            .find(|record| record.replay_of() == Some(panel.original_id()));
        panel.show(context, proxy_running, latest)
    }

    pub(super) fn apply_replay(&mut self, action: ReplayAction, proxy: Option<&ProxyHandle>) {
        if matches!(action, ReplayAction::Close) {
            self.replay = None;
        } else if let Some(panel) = self.replay.as_mut() {
            panel.apply(action, proxy);
        }
    }
}

pub(crate) struct MapState {
    pub(crate) analysis: AnalysisPipeline,
    pub(super) panel: MapPanel,
}

impl MapState {
    pub(super) fn new(analysis: AnalysisPipeline) -> Self {
        Self {
            analysis,
            panel: MapPanel::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.analysis.session_cleared();
        self.panel.clear();
    }
}

pub(crate) struct Dialogs {
    pub(crate) ca_panel: CaPanel,
    pub(crate) browsers: BrowserPanel,
    pub(crate) save: SaveControls,
    pub(crate) har: HarDialog,
    pub(crate) authorization_notice: AuthorizationNotice,
}

impl Dialogs {
    pub(super) fn new() -> Self {
        Self {
            ca_panel: CaPanel::default(),
            browsers: BrowserPanel::default(),
            save: SaveControls::new(),
            har: HarDialog::default(),
            authorization_notice: AuthorizationNotice::default(),
        }
    }

    pub(super) fn close_proxy_controls(&mut self) {
        self.browsers.close();
        self.ca_panel.refresh();
    }
}

/// Top-level inspector view switcher (`Requests` list vs `Map` site/relationship map).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MainView {
    #[default]
    Requests,
    Map,
}

pub(crate) struct LayoutState {
    pub(crate) theme: ThemeSettings,
    pub(crate) main_view: MainView,
    /// Fraction of the main area given to the details pane before clamping.
    /// Updated only when the user drags the divider, so shrinking the window
    /// and widening it again restores the split instead of staying narrow.
    pub(super) detail_share: f32,
}

impl LayoutState {
    pub(super) fn new(context: &egui::Context) -> Self {
        Self {
            theme: ThemeSettings::new(context),
            main_view: MainView::default(),
            detail_share: Layout::DEFAULT_DETAIL_SHARE,
        }
    }
}
