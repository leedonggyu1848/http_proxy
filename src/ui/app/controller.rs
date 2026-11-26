//! Applies actions to the application and advances capture between frames.

use std::time::Duration;

use crate::ui::detail::detail_tabs::DetailTab;
use crate::ui::detail::search::SearchSettings;
use crate::ui::dialogs::range_merge::RangeAction;
use crate::ui::dialogs::save::SaveMenuAction;
use crate::ui::requests::toolbar::AuthorizationNotice;
use eframe::egui;
use http_inspector_analysis::filter::RecordFilter;
use http_inspector_content::format::ViewMode;
use http_inspector_core::recording::SaveFormat;
use http_inspector_core::settings::port::PortOrigin;
use http_inspector_exchange::Exchange;
use http_inspector_exchange::{BodySide, RecordId};
use http_inspector_interop::curl::RedactedHeaders;
use http_inspector_interop::map_export::{
    export_graph_dot, export_graph_mermaid, export_tree_json, export_tree_text,
};

use super::{InspectorApp, MainView};
use crate::ui::dialogs::{
    browser_panel::BrowserAction, bulk_save::BulkAction, ca_panel, har_dialog,
    replay_panel::ReplayAction,
};
use crate::ui::map::{MapAction, MapExportFormat};
use http_inspector_core::interception::ProxyHandle;

const LIVE_REPAINT: Duration = Duration::from_millis(250);
const IDLE_REPAINT: Duration = Duration::from_secs(1);

pub(crate) enum AppAction {
    StartProxy,
    StopProxy,
    SetPort(String),
    SetHttpsInspection(bool),
    DismissAuthorization,
    DismissCaNotice,
    SaveMenu(SaveMenuAction),
    SetTheme(egui::ThemePreference),
    SetBodyStorageLimit(u64),
    Browser(BrowserAction),
    Ca(ca_panel::Action),
    Har(har_dialog::HarAction),
    Replay(ReplayAction),
    ConfirmBodySave(bool),
    BulkSave(BulkAction),
    CancelBulkSave,
    SwitchView(MainView),
    ClearSession,
    ClearMapFilter,
    Filter(RecordFilter),
    SelectRecord(RecordId),
    DetailTab(DetailTab),
    BodyMode(ViewMode),
    OpenReplay(RecordId),
    CopyCurl(RecordId),
    CopyText(String),
    SetCurlRedactions(RedactedHeaders),
    DismissCurlNotice,
    SaveBody(RecordId, BodySide, SaveFormat),
    PlanRanges(RecordId),
    Range(RangeAction),
    Search(SearchSettings),
    Map(MapAction),
}

impl InspectorApp {
    pub(super) fn update(&mut self, context: &egui::Context) {
        self.update_capture();
        if self.layout.main_view == MainView::Requests {
            self.requests.update_filters(context);
        }
        self.detail.curl.expire_notice();
        self.dialogs.save.poll();
        self.session.body_saving.poll(context);
        self.poll_range_plan(context);
        if self.dialogs.browsers.receive() {
            self.dialogs.ca_panel.refresh();
        }
        if self.session.capture.https_inspection {
            self.dialogs
                .ca_panel
                .poll(context, self.session.capture.ca_directory.as_deref());
        }
        // The proxy wakes the UI on capture events. Check periodically for
        // in-flight values and engine failures with no event of their own.
        if self.requests.has_live_records() {
            context.request_repaint_after(LIVE_REPAINT);
        } else if self.session.capture.proxy.is_some() {
            context.request_repaint_after(IDLE_REPAINT);
        }
    }

    pub(super) fn dispatch(&mut self, context: &egui::Context, actions: Vec<AppAction>) {
        if !actions.is_empty() {
            context.request_repaint();
        }
        for action in actions {
            self.apply_action(context, action);
        }
    }

    fn apply_action(&mut self, context: &egui::Context, action: AppAction) {
        match action {
            AppAction::StartProxy => self.start_proxy(),
            AppAction::StopProxy => self.stop_proxy(),
            AppAction::SetPort(text) => {
                self.session.capture.port.text = text;
                self.session.capture.port.origin = PortOrigin::User;
            }
            AppAction::SetHttpsInspection(enabled) => {
                self.session.capture.https_inspection = enabled;
                if enabled {
                    self.dialogs.authorization_notice =
                        self.dialogs.authorization_notice.inspection_enabled();
                }
            }
            AppAction::DismissAuthorization => {
                self.dialogs.authorization_notice = AuthorizationNotice::Dismissed;
            }
            AppAction::DismissCaNotice => self.dialogs.ca_panel.dismiss_notice(),
            AppAction::SaveMenu(action) => self.apply_save_menu(context, action),
            AppAction::SetTheme(preference) => {
                self.layout.theme.set_preference(context, preference);
            }
            AppAction::SetBodyStorageLimit(limit) => self.session.body_saving.set_limit(limit),
            AppAction::Browser(action) => self.apply_browser(action),
            AppAction::Ca(action) => {
                if let Some(directory) = self.session.capture.ca_directory.as_deref() {
                    self.dialogs.ca_panel.start(action, directory, context);
                }
            }
            AppAction::Har(action) => self.apply_har_action(context, action),
            AppAction::Replay(action) => self
                .detail
                .apply_replay(action, self.session.capture.proxy.as_ref()),
            AppAction::ConfirmBodySave(choice) => {
                self.session.body_saving.apply_confirmation(choice);
            }
            AppAction::BulkSave(action) => self.apply_bulk_action(action),
            AppAction::CancelBulkSave => self.session.body_saving.cancel_bulk(),
            AppAction::SwitchView(view) => self.layout.main_view = view,
            AppAction::ClearSession => self.clear_session(),
            AppAction::ClearMapFilter => self.requests.clear_map_filter(),
            AppAction::Filter(filter) => self.requests.record_filter = filter,
            AppAction::Search(settings) => self.requests.search.set_settings(settings),
            AppAction::SelectRecord(id) => self.requests.selected = Some(id),
            AppAction::DetailTab(tab) => self.detail.tab = tab,
            AppAction::BodyMode(mode) => self.detail.body_mode = mode,
            AppAction::OpenReplay(id) => {
                if let Some(record) = self.record(id).cloned()
                    && let Err(error) = self.detail.open_replay(record)
                {
                    self.report_error(error.to_string());
                }
            }
            AppAction::CopyText(text) => context.copy_text(text),
            AppAction::CopyCurl(id) => {
                if let Some(record) = self.record(id).cloned() {
                    self.detail.curl.copy(context, &record);
                }
            }
            AppAction::SetCurlRedactions(redactions) => self.detail.curl.set_redactions(redactions),
            AppAction::DismissCurlNotice => self.detail.curl.dismiss_notice(),
            AppAction::SaveBody(id, side, format) => {
                if let Some(record) = self.record(id).cloned() {
                    self.session.body_saving.request(
                        &record,
                        side,
                        format,
                        self.dialogs.save.directory(),
                    );
                }
            }
            AppAction::PlanRanges(id) => {
                if let Some(record) = self.record(id).cloned() {
                    self.start_range_plan(&record);
                }
            }
            AppAction::Range(action) => self.apply_range_action(action),
            AppAction::Map(action) => self.handle_map_action(context, action),
        }
    }

    fn apply_browser(&mut self, action: BrowserAction) {
        if matches!(action, BrowserAction::Close) {
            self.dialogs.ca_panel.refresh();
        }
        let capture = &self.session.capture;
        self.dialogs.browsers.apply(
            action,
            capture.proxy.as_ref().map(ProxyHandle::local_addr),
            capture
                .ca_directory
                .as_deref()
                .filter(|_| capture.https_inspection),
        );
    }

    fn apply_save_menu(&mut self, context: &egui::Context, action: SaveMenuAction) {
        match action {
            SaveMenuAction::Directory(directory) => self.dialogs.save.set_directory(directory),
            SaveMenuAction::SavePng => self.dialogs.save.start_screenshot(context),
            SaveMenuAction::SaveLogs => {
                if let Some(log) = self.session.capture.full_log.as_ref() {
                    self.dialogs.save.save_logs(log);
                }
            }
            SaveMenuAction::ExportHar => self
                .dialogs
                .har
                .open(self.dialogs.save.suggested_path("har")),
            SaveMenuAction::SaveBodies => self.session.body_saving.open_bulk(),
        }
    }

    pub(crate) fn report_error(&mut self, error: String) {
        self.session.error = Some(error);
        self.session.capture.error = None;
    }

    fn record(&self, id: RecordId) -> Option<&Exchange> {
        self.requests
            .records
            .iter()
            .find(|record| record.id() == id)
    }

    pub(super) fn start_proxy(&mut self) {
        let context = self.context.clone();
        self.session.error = None;
        self.session
            .capture
            .start(self.session.body_saving.store(), move || {
                context.request_repaint();
            });
    }

    pub(crate) fn stop_proxy(&mut self) {
        self.dialogs.close_proxy_controls();
        self.session.capture.stop(&mut self.requests.records);
    }

    fn update_capture(&mut self) {
        let Self {
            session,
            requests,
            map,
            ..
        } = self;
        session.capture.receive(
            &mut requests.records,
            &mut requests.selected,
            &mut map.analysis,
        );
        let _changed = map.analysis.update();
        session.capture.release_failed(
            &mut requests.records,
            &mut requests.selected,
            &mut map.analysis,
        );
    }

    pub(crate) fn clear_session(&mut self) {
        self.requests.clear(self.session.capture.engine_dropped());
        self.map.clear();
    }

    pub(super) fn handle_map_action(&mut self, ctx: &egui::Context, action: MapAction) {
        match action {
            MapAction::ShowInRequests(key) => {
                self.requests.show_map_scope(key);
                self.layout.main_view = MainView::Requests;
                ctx.request_repaint();
            }
            MapAction::SelectRequest(id) => {
                self.requests.selected = Some(id);
                self.layout.main_view = MainView::Requests;
                ctx.request_repaint();
            }
            MapAction::Export(format) => {
                let content = self.map_export_text(format);
                self.dialogs.save.save_text_file(
                    ctx,
                    format.file_prefix(),
                    format.extension(),
                    content,
                );
            }
            MapAction::Copy(format) => ctx.copy_text(self.map_export_text(format)),
            MapAction::ClearAll => {
                self.clear_session();
                ctx.request_repaint();
            }
        }
    }

    fn map_export_text(&self, format: MapExportFormat) -> String {
        match format {
            MapExportFormat::TreeText => {
                export_tree_text(self.map.analysis.url_map(), self.map.panel.fold_ids())
            }
            MapExportFormat::TreeJson => serde_json::to_string_pretty(&export_tree_json(
                self.map.analysis.url_map(),
                self.map.panel.fold_ids(),
            ))
            .unwrap_or_else(|_| "{}".to_owned()),
            MapExportFormat::GraphMermaid => export_graph_mermaid(
                self.map.analysis.url_map(),
                self.map.panel.max_graph_domains(),
            ),
            MapExportFormat::GraphDot => export_graph_dot(
                self.map.analysis.url_map(),
                self.map.panel.max_graph_domains(),
            ),
        }
    }

    fn apply_har_action(&mut self, context: &egui::Context, action: har_dialog::HarAction) {
        self.dialogs.har.close();
        if let har_dialog::HarAction::Export(export) = action {
            self.export_har(context, export);
        }
    }

    fn export_har(&mut self, context: &egui::Context, export: har_dialog::HarExport) {
        let records: Vec<Exchange> = match export.scope {
            har_dialog::HarScope::All => self.requests.records.iter().cloned().collect(),
            har_dialog::HarScope::Filtered => self
                .requests
                .filtered_records()
                .into_iter()
                .cloned()
                .collect(),
        };
        self.dialogs
            .save
            .export_har(context, records, export.options, export.path);
    }
}
