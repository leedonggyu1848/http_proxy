mod app_paths;
mod body_display;
mod body_format;
mod body_saving;
mod body_view;
mod browser_panel;
mod ca_panel;
mod copy_curl;
mod detail_tabs;
mod detail_view;
mod filter;
mod fonts;
mod har;
mod har_dialog;
mod image_decode;
mod image_view;
mod outcome_view;
mod port_settings;
mod records;
mod replay_panel;
mod request_table;
mod save;
mod scope_help;
mod theme;
mod toolbar;
mod tree_view;

use self::detail_tabs::DetailTab;
use self::filter::RecordFilter;
use self::toolbar::PortChoice;

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_proxy::{
    EngineState, Exchange, FullLog, HttpsInspection, ProxyConfig, ProxyHandle, RecordId,
};

const MAX_RECORDS: usize = 2_000;
/// Repaint interval while pending requests or streaming bodies change on screen.
const LIVE_REPAINT: Duration = Duration::from_millis(250);
/// Repaint interval otherwise, to notice an engine that stopped on its own.
const IDLE_REPAINT: Duration = Duration::from_secs(1);
/// Default window size, large enough for the list and details side by side.
const DEFAULT_WINDOW: [f32; 2] = [1_280.0, 820.0];
const MIN_WINDOW: [f32; 2] = [640.0, 420.0];
/// Height kept for the request list and details even in a short window,
/// about ten table rows; the window scrolls instead of shrinking it further.
const MIN_MAIN_HEIGHT: f32 = 280.0;
const MIN_TABLE_WIDTH: f32 = 320.0;
const MIN_DETAIL_WIDTH: f32 = 280.0;
/// Share of the main area the details panel takes before the user drags it.
const DEFAULT_DETAIL_SHARE: f32 = 0.4;

pub fn main() -> eframe::Result {
    init_tracing();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW)
            .with_min_inner_size(MIN_WINDOW),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        http_proxy::APP_NAME,
        options,
        Box::new(|context| {
            fonts::configure_fonts(&context.egui_ctx);
            Ok(Box::new(InspectorApp::new(context.egui_ctx.clone())))
        }),
    )
}

use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _subscriber_already_set = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .is_err();
}

struct InspectorApp {
    theme: theme::ThemeSettings,
    replay: Option<replay_panel::ReplayPanel>,
    curl: copy_curl::CurlCopy,
    proxy: Option<ProxyHandle>,
    records: records::RecordList<Exchange>,
    selected: Option<RecordId>,
    filter_text: String,
    record_filter: RecordFilter,
    https_inspection: bool,
    authorization_notice: toolbar::AuthorizationNotice,
    ca_directory: Option<PathBuf>,
    ca_panel: ca_panel::CaPanel,
    browsers: browser_panel::BrowserPanel,
    error: Option<String>,
    full_log: Option<FullLog>,
    log_init_error: Option<String>,
    save: save::SaveControls,
    har_dialog: har_dialog::HarDialog,
    body_saving: body_saving::BodySaving,
    port: PortChoice,
    table: request_table::RequestTable,
    detail_tab: DetailTab,
    body_mode: body_format::ViewMode,
    body_cache: body_display::BodyViewCache,
    image: image_view::ImagePanel,
    /// Height of the status bar in the previous frame, reserved below the
    /// main area so that it fits in the window without scrolling.
    status_bar_height: f32,
    context: egui::Context,
}

impl InspectorApp {
    fn new(context: egui::Context) -> Self {
        let theme = theme::ThemeSettings::new(&context);
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (
                None,
                Some(format!("Full-session logging is unavailable: {error}")),
            ),
        };
        let mut app = Self {
            replay: None,
            curl: copy_curl::CurlCopy::default(),
            proxy: None,
            records: records::RecordList::new(MAX_RECORDS),
            selected: None,
            filter_text: String::new(),
            record_filter: RecordFilter::All,
            https_inspection: false,
            authorization_notice: toolbar::AuthorizationNotice::default(),
            ca_directory: app_paths::ca_directory(|name| std::env::var_os(name)),
            ca_panel: ca_panel::CaPanel::default(),
            browsers: browser_panel::BrowserPanel::default(),
            error: None,
            full_log,
            log_init_error,
            save: save::SaveControls::new(),
            har_dialog: har_dialog::HarDialog::default(),
            body_saving: body_saving::BodySaving::open(),
            port: PortChoice::from_launch(),
            table: request_table::RequestTable::new(),
            detail_tab: DetailTab::Headers,
            body_mode: body_format::ViewMode::default(),
            body_cache: body_display::BodyViewCache::default(),
            image: image_view::ImagePanel::default(),
            status_bar_height: 0.0,
            theme,
            context,
        };
        app.start_proxy();
        app
    }

    fn start_proxy(&mut self) {
        let port = match port_settings::ProxyPort::parse(&self.port.text) {
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
                let context = self.context.clone();
                let _first_notifier = proxy.notify_on_event(move || context.request_repaint());
                self.proxy = Some(proxy);
                self.error = self
                    .port
                    .remember(port)
                    .err()
                    .map(|error| error.to_string());
            }
            Err(error) => {
                self.error = Some(port_settings::start_error_text(&error, port));
            }
        }
    }

    fn stop_proxy(&mut self) {
        self.browsers.close();
        self.ca_panel.refresh();
        if let Some(proxy) = self.take_proxy()
            && let Err(error) = proxy.stop()
        {
            self.error = Some(error.to_string());
        }
    }

    /// Releases a proxy whose engine stopped on its own so the toolbar shows
    /// `Stopped` with the cause and `Start` can be pressed again.
    fn release_failed_proxy(&mut self) {
        let Some(EngineState::Failed { message }) = self.proxy.as_ref().map(ProxyHandle::state)
        else {
            return;
        };
        self.receive_records();
        let mut error = format!("The proxy stopped: {message}. Press Start to restart it.");
        if let Some(proxy) = self.take_proxy()
            && let Err(stop_error) = proxy.stop()
        {
            error = format!("{error} ({stop_error})");
        }
        self.error = Some(error);
    }

    /// Detaches the running proxy, keeping its drop count for the toolbar.
    fn take_proxy(&mut self) -> Option<ProxyHandle> {
        let proxy = self.proxy.take()?;
        self.records.proxy_stopped(proxy.dropped_records());
        Some(proxy)
    }

    fn engine_dropped(&self) -> u64 {
        self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records)
    }

    fn receive_records(&mut self) {
        let Some(proxy) = self.proxy.as_mut() else {
            return;
        };
        while let Some(record) = proxy.try_receive() {
            if let Some(removed) = self.records.push(record)
                && self.selected == Some(removed.id())
            {
                self.selected = None;
            }
        }
    }

    /// Reports whether any displayed value still changes over time.
    fn has_live_records(&self) -> bool {
        self.records.iter().any(Exchange::is_live)
    }
}

impl eframe::App for InspectorApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.receive_records();
        self.release_failed_proxy();
        self.body_saving.update(ui.ctx());
        if let Some(panel) = self.replay.as_mut() {
            let latest = self
                .records
                .iter()
                .rev()
                .find(|record| record.replay_of() == Some(panel.original_id()));
            if !panel.show(ui.ctx(), self.proxy.as_ref(), latest) {
                self.replay = None;
            }
        }
        self.har_export(ui.ctx());
        let shortcuts_allowed = request_shortcuts_allowed(ui.ctx());
        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("window-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let top = ui.cursor().top();
                self.toolbar(ui);
                self.notices(ui);
                ui.separator();
                self.filters(ui);
                let used = ui.cursor().top() - top + ui.spacing().item_spacing.y;
                let height = main_height(viewport_height, used, self.status_bar_height);
                self.main_area(ui, height, shortcuts_allowed);
                let status_top = ui.cursor().top();
                ui.separator();
                self.status_bar(ui);
                self.status_bar_height = ui.min_rect().bottom() - status_top;
            });

        // New exchanges wake the UI through the proxy's notifier. Periodic
        // repaints are needed only for values that change without an event
        // and for noticing an engine that stopped on its own.
        if self.has_live_records() {
            ui.ctx().request_repaint_after(LIVE_REPAINT);
        } else if self.proxy.is_some() {
            ui.ctx().request_repaint_after(IDLE_REPAINT);
        }
    }
}

impl InspectorApp {
    /// Shows the `Export HAR` window and starts the export it confirms.
    fn har_export(&mut self, context: &egui::Context) {
        if !self.har_dialog.is_open() {
            return;
        }
        let counts = har_dialog::ScopeCounts {
            all: self.records.len(),
            filtered: self.filtered_records().len(),
        };
        let Some(export) = self.har_dialog.show(context, counts, self.save.is_busy()) else {
            return;
        };
        let records: Vec<Exchange> = match export.scope {
            har_dialog::HarScope::All => self.records.iter().cloned().collect(),
            har_dialog::HarScope::Filtered => {
                self.filtered_records().into_iter().cloned().collect()
            }
        };
        self.save
            .export_har(context, records, export.options, export.path);
    }

    fn filtered_records(&self) -> Vec<&Exchange> {
        filter::visible_records(&self.records, self.record_filter, &self.filter_text)
    }

    /// Requests on the left and details on the right, split by a draggable
    /// divider. Both are clipped to `height` so neither draws over the other.
    fn main_area(&mut self, ui: &mut egui::Ui, height: f32, shortcuts_allowed: bool) {
        let size = egui::vec2(ui.available_width(), height);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let mut main = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("main-area")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        main.shrink_clip_rect(rect);
        let width = rect.width();
        let max_detail = (width - MIN_TABLE_WIDTH).max(MIN_DETAIL_WIDTH);
        egui::Panel::right("details-panel")
            .resizable(true)
            .default_size((width * DEFAULT_DETAIL_SHARE).clamp(MIN_DETAIL_WIDTH, max_detail))
            .size_range(MIN_DETAIL_WIDTH..=max_detail)
            .show(&mut main, |ui| self.detail_panel(ui, shortcuts_allowed));
        egui::CentralPanel::no_frame().show(&mut main, |ui| {
            self.request_table(ui, shortcuts_allowed);
        });
    }
}

/// Height left for the request list and details once the controls above and
/// the status bar below are placed, but never less than [`MIN_MAIN_HEIGHT`].
fn main_height(viewport: f32, above: f32, below: f32) -> f32 {
    (viewport - above - below).floor().max(MIN_MAIN_HEIGHT)
}

fn request_shortcuts_allowed(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(egui::Memory::focused);
    !egui::Popup::is_any_open(ctx)
        && focused.is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_none())
}

#[cfg(test)]
mod layout_tests {
    use super::{MIN_MAIN_HEIGHT, main_height};

    #[test]
    fn main_area_fills_the_rest_but_keeps_a_minimum() {
        assert!((main_height(700.0, 90.0, 30.0) - 580.0).abs() < f32::EPSILON);
        assert!((main_height(300.0, 90.0, 30.0) - MIN_MAIN_HEIGHT).abs() < f32::EPSILON);
    }
}
