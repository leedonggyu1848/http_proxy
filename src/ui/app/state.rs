use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use http_inspector_analysis::filter::{RecordFilter, RecordList};
use http_inspector_analysis::pipeline::AnalysisPipeline;
use http_inspector_analysis::url_map::RequestScope;
use http_inspector_content::format::ViewMode;
use http_inspector_core::https::HttpsInspection;
use http_inspector_core::interception::{ProxyConfig, ProxyHandle};
use http_inspector_core::recording::{FullLog, PreviewBodies};
use http_inspector_core::replay::ReplayError;
use http_inspector_core::settings;
use http_inspector_exchange::{BodySource, Exchange, RecordId};

use crate::ui::detail::detail_tabs::DetailTab;
use crate::ui::detail::{
    body_render::BodyViewCache, image_view::ImagePanel, search::RequestSearch,
};
use crate::ui::dialogs::{
    body_saving::BodySaving, browser_panel::BrowserPanel, ca_panel::CaPanel, copy_curl::CurlCopy,
    har_dialog::HarDialog, replay_panel::ReplayPanel, save::SaveControls,
};
use crate::ui::map::{MapPanel, RecordMatchCache};
use crate::ui::requests::filter;
use crate::ui::requests::request_table::RequestTable;
use crate::ui::requests::toolbar::{self, AuthorizationNotice, PortChoice};
use crate::ui::style::theme::ThemeSettings;

use super::{DEFAULT_DETAIL_SHARE, MAX_RECORDS};

pub(crate) struct SessionState {
    pub(crate) proxy: Option<ProxyHandle>,
    pub(crate) full_log: Option<FullLog>,
    pub(crate) log_init_error: Option<String>,
    pub(crate) body_saving: BodySaving,
    pub(crate) https_inspection: bool,
    pub(crate) ca_directory: Option<PathBuf>,
    pub(crate) port: PortChoice,
    pub(crate) error: Option<String>,
}

impl SessionState {
    pub(super) fn new() -> Self {
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (
                None,
                Some(format!("Full-session logging is unavailable: {error}")),
            ),
        };
        Self {
            proxy: None,
            full_log,
            log_init_error,
            body_saving: BodySaving::open(),
            https_inspection: false,
            ca_directory: settings::paths::ca_directory(|name| std::env::var_os(name)),
            port: PortChoice::from_launch(),
            error: None,
        }
    }

    pub(super) fn body_source(&self) -> Arc<dyn BodySource> {
        match self.body_saving.store() {
            Some(store) => Arc::new(store.clone()),
            None => Arc::new(PreviewBodies),
        }
    }

    pub(crate) fn start_proxy(&mut self, context: &egui::Context) {
        let port = match settings::port::ProxyPort::parse(&self.port.text) {
            Ok(port) => port,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let mut config = ProxyConfig::loopback(port.value());
        if self.https_inspection {
            let Some(directory) = self.ca_directory.as_ref() else {
                self.error = Some(
                    "HTTPS inspection requires an application data directory; check HOME and platform storage settings"
                        .to_owned(),
                );
                return;
            };
            config = config.with_https_inspection(HttpsInspection::new(directory.clone()));
        }
        if let Some(store) = self.body_saving.store() {
            config = config.with_body_store(store.clone());
        }
        let result = self.full_log.as_ref().map_or_else(
            || ProxyHandle::start(config.clone()),
            |log| ProxyHandle::start_recording(config.clone(), log.clone()),
        );
        match result {
            Ok(proxy) => {
                let context = context.clone();
                let _first_notifier = proxy.notify_on_event(move || context.request_repaint());
                self.proxy = Some(proxy);
                self.error = self
                    .port
                    .remember(port)
                    .err()
                    .map(|error| error.to_string());
            }
            Err(error) => {
                self.error = Some(toolbar::start_error_text(&error, port));
            }
        }
    }

    pub(crate) fn engine_dropped(&self) -> u64 {
        self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records)
    }
}

pub(crate) struct RequestsState {
    pub(crate) records: RecordList<Exchange>,
    pub(crate) record_filter: RecordFilter,
    pub(crate) search: RequestSearch,
    pub(crate) table: RequestTable,
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
            table: RequestTable::new(),
            selected: None,
            map_path_filter: None,
            map_filter_cache: RecordMatchCache::default(),
        }
    }

    pub(super) fn record_added(&mut self, record: Exchange) {
        if let Some(removed) = self.records.push(record)
            && self.selected == Some(removed.id())
        {
            self.selected = None;
        }
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
    pub(crate) body_cache: BodyViewCache,
    pub(crate) image: ImagePanel,
    pub(crate) curl: CurlCopy,
    replay: Option<ReplayPanel>,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            tab: DetailTab::Headers,
            body_mode: ViewMode::default(),
            body_cache: BodyViewCache::default(),
            image: ImagePanel::default(),
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

    pub(super) fn update_replay(
        &mut self,
        context: &egui::Context,
        proxy: Option<&ProxyHandle>,
        records: &RecordList<Exchange>,
    ) {
        if let Some(panel) = self.replay.as_mut() {
            let latest = records
                .iter()
                .rev()
                .find(|record| record.replay_of() == Some(panel.original_id()));
            if !panel.show(context, proxy, latest) {
                self.replay = None;
            }
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
pub(super) enum MainView {
    #[default]
    Requests,
    Map,
}

pub(crate) struct LayoutState {
    pub(crate) theme: ThemeSettings,
    pub(super) main_view: MainView,
    /// Height of the status bar in the previous frame, reserved below the
    /// main area so that it fits in the window without scrolling.
    pub(super) status_bar_height: f32,
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
            status_bar_height: 0.0,
            detail_share: DEFAULT_DETAIL_SHARE,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http::Method;
    use http_inspector_exchange::ExchangeBuilder;

    use super::*;

    #[test]
    fn eviction_clears_only_the_selection_that_was_removed() -> Result<(), Box<dyn Error>> {
        let mut requests = RequestsState::new(&egui::Context::default());
        requests.records = RecordList::new(2);
        let oldest = ExchangeBuilder::http(Method::GET, "http://fixture.test/a")?.in_flight()?;
        let selected = ExchangeBuilder::http(Method::GET, "http://fixture.test/b")?.in_flight()?;
        let selected_id = selected.id();
        requests.record_added(oldest);
        requests.record_added(selected);
        requests.selected = Some(selected_id);

        requests.record_added(
            ExchangeBuilder::http(Method::GET, "http://fixture.test/c")?.in_flight()?,
        );
        assert_eq!(requests.selected, Some(selected_id));

        requests.record_added(
            ExchangeBuilder::http(Method::GET, "http://fixture.test/d")?.in_flight()?,
        );
        assert_eq!(requests.selected, None);
        assert_eq!(requests.records.evicted(), 2);
        Ok(())
    }
}
