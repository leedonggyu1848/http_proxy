#![forbid(unsafe_code)]

#[allow(
    clippy::redundant_pub_crate,
    reason = "Save controls are visible only to the parent application module."
)]
mod save;

#[allow(
    clippy::redundant_pub_crate,
    reason = "The CA panel is private to the binary; unreachable_pub forbids public exports."
)]
mod ca_panel;

#[allow(
    clippy::redundant_pub_crate,
    reason = "Browser controls are private to the application module."
)]
mod browser_panel;

#[allow(
    clippy::redundant_pub_crate,
    reason = "Body preview formatting is private to the application module."
)]
mod body_view;

#[allow(
    clippy::redundant_pub_crate,
    reason = "Image previews are private to the application module."
)]
mod image_view;

#[allow(
    clippy::redundant_pub_crate,
    reason = "The displayed record list is private to the application module."
)]
mod records;

#[allow(
    clippy::redundant_pub_crate,
    reason = "Capture scope help is private to the application module."
)]
mod scope_help;

#[allow(
    clippy::redundant_pub_crate,
    reason = "Port settings are private to the application module."
)]
mod port_settings;

#[allow(
    clippy::redundant_pub_crate,
    reason = "The request list is private to the application module."
)]
mod request_table;

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, EngineState, Exchange, FullLog, HeaderPreview,
    HttpsInspection, Outcome, ProxyConfig, ProxyHandle, RecordId, TunnelPreview, TunnelState,
};
use tracing_subscriber::EnvFilter;

const MAX_RECORDS: usize = 2_000;
/// Repaint interval while pending requests or streaming bodies change on screen.
const LIVE_REPAINT: Duration = Duration::from_millis(250);
/// Repaint interval otherwise, to notice an engine that stopped on its own.
const IDLE_REPAINT: Duration = Duration::from_secs(1);

fn main() -> eframe::Result {
    init_tracing();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1_100.0, 700.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        http_proxy::APP_NAME,
        options,
        Box::new(|context| {
            configure_fonts(&context.egui_ctx);
            Ok(Box::new(InspectorApp::new(context.egui_ctx.clone())))
        }),
    )
}

fn configure_fonts(context: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/usr/share/fonts/truetype/nanum/NanumGothicCoding.ttf",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    ];
    let Some(data) = candidates
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let name = "cjk-fallback".to_owned();
    fonts
        .font_data
        .insert(name.clone(), egui::FontData::from_owned(data).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    context.set_fonts(fonts);
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _subscriber_already_set = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .is_err();
}

struct InspectorApp {
    proxy: Option<ProxyHandle>,
    records: records::RecordList<Exchange>,
    selected: Option<RecordId>,
    filter_text: String,
    record_filter: RecordFilter,
    https_inspection: bool,
    ca_directory: Option<PathBuf>,
    ca_panel: ca_panel::CaPanel,
    browsers: browser_panel::BrowserPanel,
    error: Option<String>,
    full_log: Option<FullLog>,
    log_init_error: Option<String>,
    save: save::SaveControls,
    port: PortChoice,
    table: request_table::RequestTable,
    detail_tab: DetailTab,
    image: image_view::ImagePanel,
    context: egui::Context,
}

/// Port typed in the toolbar and where its current value came from.
struct PortChoice {
    text: String,
    origin: port_settings::PortOrigin,
    settings_directory: Option<PathBuf>,
    launch_warning: Option<String>,
}

impl PortChoice {
    fn from_launch() -> Self {
        let settings_directory = port_settings::default_settings_directory();
        let saved = settings_directory
            .as_deref()
            .and_then(port_settings::load_port);
        let launch = port_settings::launch_port(
            std::env::args_os(),
            std::env::var_os(port_settings::PORT_ENV),
            saved,
        );
        Self {
            text: launch.port.to_string(),
            origin: launch.origin,
            settings_directory,
            launch_warning: launch.warning,
        }
    }

    /// Remembers a port the user typed once the proxy started on it.
    fn remember(&self, port: port_settings::ProxyPort) -> Result<(), String> {
        if self.origin != port_settings::PortOrigin::User {
            return Ok(());
        }
        let Some(directory) = self.settings_directory.as_deref() else {
            return Err(
                "The port could not be saved because the application settings directory is unavailable."
                    .to_owned(),
            );
        };
        port_settings::save_port(directory, port)
            .map_err(|error| format!("The proxy started, but the port could not be saved: {error}"))
    }
}

impl InspectorApp {
    fn new(context: egui::Context) -> Self {
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (
                None,
                Some(format!("Full-session logging is unavailable: {error}")),
            ),
        };
        let mut app = Self {
            proxy: None,
            records: records::RecordList::new(MAX_RECORDS),
            selected: None,
            filter_text: String::new(),
            record_filter: RecordFilter::All,
            https_inspection: false,
            ca_directory: default_ca_directory(),
            ca_panel: ca_panel::CaPanel::default(),
            browsers: browser_panel::BrowserPanel::default(),
            error: None,
            full_log,
            log_init_error,
            save: save::SaveControls::new(),
            port: PortChoice::from_launch(),
            table: request_table::RequestTable::new(),
            detail_tab: DetailTab::Headers,
            image: image_view::ImagePanel::default(),
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
        let result = self.full_log.as_ref().map_or_else(
            || ProxyHandle::start(config.clone()),
            |log| ProxyHandle::start_recording(config.clone(), log.clone()),
        );
        match result {
            Ok(proxy) => {
                let context = self.context.clone();
                let _first_notifier = proxy.notify_on_event(move || context.request_repaint());
                self.proxy = Some(proxy);
                self.error = self.port.remember(port).err();
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

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(proxy) = self.proxy.as_ref() {
                let address = proxy.local_addr();
                ui.label(RichText::new("Running").color(Color32::LIGHT_GREEN));
                ui.monospace(address.to_string());
                copy_menu(ui, address);
                if ui.button("Stop").clicked() {
                    self.stop_proxy();
                }
            } else {
                ui.label(RichText::new("Stopped").color(Color32::LIGHT_RED));
                ui.label("Port");
                let edited = ui
                    .add(egui::TextEdit::singleline(&mut self.port.text).desired_width(56.0))
                    .on_hover_text(
                        "Loopback port for the proxy. 0 picks a free port. A port you type is remembered.",
                    )
                    .changed();
                if edited {
                    self.port.origin = port_settings::PortOrigin::User;
                }
                if ui.button("Start").clicked() {
                    self.start_proxy();
                }
            }

            ui.separator();
            ui.label(format!("{} requests", self.records.len()));
            let dropped = self.records.dropped(self.engine_dropped());
            if dropped > 0 {
                ui.colored_label(Color32::LIGHT_RED, format!("{dropped} records dropped"))
                    .on_hover_text(
                        "The capture queue was full, so these exchanges were forwarded but not shown.",
                    );
            }
            let evicted = self.records.evicted();
            if evicted > 0 {
                ui.colored_label(
                    Color32::YELLOW,
                    format!("{evicted} older records removed"),
                )
                .on_hover_text(format!(
                    "Only the newest {MAX_RECORDS} requests are kept on screen. The full-session log keeps every record."
                ));
            }
            if ui
                .button("Clear")
                .on_hover_text("Clear the displayed requests. The full-session log is retained.")
                .clicked()
            {
                self.records.clear(self.engine_dropped());
                self.selected = None;
            }
        });

        self.status_messages(ui);

        self.save.ui(ui, self.full_log.as_ref());

        self.https_controls(ui);
        ui.separator();
        if self.browsers.show(
            ui,
            self.proxy.as_ref().map(ProxyHandle::local_addr),
            self.ca_directory
                .as_deref()
                .filter(|_| self.https_inspection),
        ) {
            self.ca_panel.refresh();
        }
    }

    /// Shows launch warnings, errors, and full-session log notices.
    fn status_messages(&self, ui: &mut egui::Ui) {
        if let Some(warning) = self.port.launch_warning.as_ref() {
            ui.colored_label(Color32::YELLOW, warning);
        }
        if let Some(error) = self.error.as_ref() {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(error) = self
            .log_init_error
            .clone()
            .or_else(|| self.full_log.as_ref().and_then(FullLog::error))
        {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(log) = self
            .full_log
            .as_ref()
            .filter(|log| log.body_limit_reached())
        {
            ui.colored_label(
                Color32::YELLOW,
                format!(
                    "The full-session log reached its {} body limit. Later body bytes are omitted; headers and results are still logged.",
                    log.formatted_body_limit()
                ),
            );
        }
    }

    fn https_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("HTTPS inspection");
            ui.add_enabled(
                self.proxy.is_none(),
                egui::Checkbox::new(&mut self.https_inspection, "Decrypt HTTPS"),
            );
            if self.proxy.is_some() {
                ui.weak("Stop the proxy to change this setting.");
            }
        });
        if self.https_inspection {
            self.ca_panel.show(ui, self.ca_directory.as_deref());
            ui.colored_label(
                Color32::YELLOW,
                "Only enable inspection for traffic that you are authorized to inspect.",
            );
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_text)
                    .hint_text("method, URL, status, or header"),
            );
            egui::ComboBox::from_id_salt("record-filter")
                .selected_text(self.record_filter.label())
                .show_ui(ui, |ui| {
                    for filter in RecordFilter::ALL {
                        ui.selectable_value(&mut self.record_filter, filter, filter.label());
                    }
                });
        });
    }

    fn request_table(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let query = self.filter_text.to_lowercase();
        let rows: Vec<&Exchange> = self
            .records
            .iter()
            .filter(|record| record_matches(record, self.record_filter, &query))
            .collect();
        self.table.controls(ui);
        if let Some(selected) = self.table.show(ui, rows, self.selected, shortcuts_allowed) {
            self.selected = Some(selected);
        }
    }

    /// Reports whether any displayed value still changes over time.
    fn has_live_records(&self) -> bool {
        self.records.iter().any(|record| match record.outcome() {
            Outcome::InFlight => true,
            Outcome::Response(response) => response.body().lifecycle() == BodyLifecycle::Streaming,
            Outcome::Tunnel(tunnel) => tunnel.snapshot().state() == &TunnelState::Open,
            Outcome::InspectionEstablished
            | Outcome::Failed { .. }
            | Outcome::Cancelled
            | Outcome::Incomplete => false,
        })
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let Some(selected) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a request to inspect headers and body previews.");
            });
            return;
        };
        let Some(record) = self.records.iter().find(|record| record.id() == selected) else {
            return;
        };

        if shortcuts_allowed
            && ui.input_mut(|input| {
                !input.modifiers.any() && input.consume_key(egui::Modifiers::NONE, egui::Key::F6)
            })
        {
            self.detail_tab = match self.detail_tab {
                DetailTab::Headers => DetailTab::Body,
                DetailTab::Body => DetailTab::Headers,
            };
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.detail_tab, DetailTab::Headers, "Headers");
            ui.selectable_value(&mut self.detail_tab, DetailTab::Body, "Body");
        });

        egui::ScrollArea::vertical()
            .id_salt(("details-scroll", self.detail_tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let request = record.request();
                ui.heading(format!("{} {}", request.method(), request.target()));
                render_timing(ui, record);
                ui.separator();
                match self.detail_tab {
                    DetailTab::Headers => {
                        ui.strong("Request headers");
                        render_headers(ui, "request-headers", request.headers());
                    }
                    DetailTab::Body => {
                        ui.strong("Request body");
                        render_optional_body(ui, request.body(), request.headers());
                    }
                }
                ui.separator();
                render_outcome(ui, record, self.detail_tab, &mut self.image);
            });
    }
}

impl eframe::App for InspectorApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.receive_records();
        self.release_failed_proxy();
        ui.heading(http_proxy::APP_NAME);
        scope_help::show(ui);
        ui.separator();
        self.toolbar(ui);
        ui.separator();
        self.filters(ui);
        ui.separator();
        let shortcuts_allowed = request_shortcuts_allowed(ui.ctx());
        ui.columns(2, |columns| {
            let [requests, details] = columns else {
                return;
            };
            requests.heading("Requests");
            self.request_table(requests, shortcuts_allowed);
            details.heading("Details");
            self.detail_panel(details, shortcuts_allowed);
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

fn request_shortcuts_allowed(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(egui::Memory::focused);
    !egui::Popup::is_any_open(ctx)
        && focused.is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_none())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum DetailTab {
    Headers,
    Body,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordFilter {
    All,
    Responses,
    Failures,
    Tunnels,
}

impl RecordFilter {
    const ALL: [Self; 4] = [Self::All, Self::Responses, Self::Failures, Self::Tunnels];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Responses => "Responses",
            Self::Failures => "Failures",
            Self::Tunnels => "Tunnels",
        }
    }
}

fn record_matches(record: &Exchange, filter: RecordFilter, query: &str) -> bool {
    let outcome = record.outcome();
    let outcome_matches = match filter {
        RecordFilter::All => true,
        RecordFilter::Responses => matches!(outcome, Outcome::Response(_)),
        RecordFilter::Failures => match outcome {
            Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => true,
            Outcome::Tunnel(tunnel) => matches!(
                tunnel.snapshot().state(),
                TunnelState::Failed { .. } | TunnelState::Incomplete
            ),
            Outcome::InFlight | Outcome::Response(_) | Outcome::InspectionEstablished => false,
        },
        RecordFilter::Tunnels => {
            matches!(outcome, Outcome::Tunnel(_) | Outcome::InspectionEstablished)
        }
    };
    let request = record.request();
    outcome_matches
        && (query.is_empty()
            || text_matches(request.method().as_str(), query)
            || text_matches(request.target(), query)
            || headers_match(request.headers(), query)
            || outcome_matches_query(outcome, query))
}

fn outcome_matches_query(outcome: &Outcome, query: &str) -> bool {
    match outcome {
        Outcome::Response(response) => {
            response.status().as_str().contains(query) || headers_match(response.headers(), query)
        }
        Outcome::Failed { stage, message } => {
            text_matches(&format!("{stage:?}"), query) || text_matches(message, query)
        }
        Outcome::InFlight
        | Outcome::Tunnel(_)
        | Outcome::InspectionEstablished
        | Outcome::Cancelled
        | Outcome::Incomplete => outcome_label(outcome).contains(query),
    }
}

/// Lowercase search keyword for outcomes that carry no searchable text.
const fn outcome_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::InFlight => "pending",
        Outcome::Response(_) => "response",
        Outcome::Tunnel(_) => "tunnel",
        Outcome::InspectionEstablished => "https inspection",
        Outcome::Failed { .. } => "failed",
        Outcome::Cancelled => "cancelled",
        Outcome::Incomplete => "incomplete",
    }
}

fn headers_match(headers: &HeaderPreview, query: &str) -> bool {
    headers
        .entries()
        .iter()
        .any(|header| text_matches(header.name(), query) || text_matches(header.value(), query))
}

fn text_matches(value: &str, lowercase_query: &str) -> bool {
    value.to_lowercase().contains(lowercase_query)
}

fn render_headers(ui: &mut egui::Ui, id: &str, headers: &HeaderPreview) {
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for header in headers.entries() {
            ui.monospace(header.name());
            ui.monospace(header.value());
            ui.end_row();
        }
    });
    if headers.entries().is_empty() {
        ui.weak("No headers captured.");
    }
    if headers.omitted() {
        ui.colored_label(Color32::YELLOW, "Additional headers were omitted.");
    }
}

fn render_optional_body(ui: &mut egui::Ui, body: Option<&BodyPreview>, headers: &HeaderPreview) {
    match body {
        Some(body) => render_body(ui, body, headers, None),
        None => {
            ui.weak("Body was not observed.");
        }
    }
}

/// Draws a body preview; `image` draws PNG bodies of that record as images.
fn render_body(
    ui: &mut egui::Ui,
    body: &BodyPreview,
    headers: &HeaderPreview,
    image: Option<(&mut image_view::ImagePanel, RecordId)>,
) {
    let Some(snapshot) = body.snapshot() else {
        ui.weak("Preview is updating.");
        return;
    };
    ui.label(format!(
        "{} bytes observed · {} · {}",
        snapshot.observed_bytes(),
        lifecycle_text(snapshot.lifecycle()),
        retention_text(snapshot.retention())
    ));
    if snapshot.retention() == BodyRetention::Omitted {
        ui.colored_label(
            Color32::YELLOW,
            "Preview was omitted to keep forwarding nonblocking.",
        );
        return;
    }
    if snapshot.bytes().is_empty() {
        ui.weak("Empty body.");
        return;
    }
    let encodings = header_values(headers, "content-encoding");
    if let Some((panel, record)) = image
        && header_values(headers, "content-type")
            .first()
            .is_some_and(|value| http_proxy::is_png_media_type(value))
    {
        panel.show(ui, record, &snapshot, &encodings);
        return;
    }
    let complete = snapshot.lifecycle() == BodyLifecycle::Complete
        && snapshot.retention() == BodyRetention::Prefix;
    let mut preview = match body_view::decode(snapshot.bytes(), &encodings, complete) {
        body_view::BodyText::Ready { text, notice } => {
            if let Some(notice) = notice {
                ui.weak(notice);
            }
            text
        }
        body_view::BodyText::Unavailable(message) => {
            ui.colored_label(Color32::YELLOW, message);
            return;
        }
    };
    ui.add(
        egui::TextEdit::multiline(&mut preview)
            .font(egui::TextStyle::Monospace)
            .desired_rows(8)
            .interactive(false),
    );
}

fn header_values<'a>(headers: &'a HeaderPreview, name: &str) -> Vec<&'a str> {
    headers
        .entries()
        .iter()
        .filter(|header| header.name() == name)
        .map(http_proxy::CapturedHeader::value)
        .collect()
}

const fn lifecycle_text(lifecycle: BodyLifecycle) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => "streaming",
        BodyLifecycle::Complete => "complete",
        BodyLifecycle::Interrupted => "interrupted by the sender",
        BodyLifecycle::Cancelled => "cancelled by the receiver",
    }
}

const fn retention_text(retention: BodyRetention) -> &'static str {
    match retention {
        BodyRetention::Prefix => "full preview",
        BodyRetention::Truncated => "truncated preview",
        BodyRetention::Omitted => "preview omitted",
    }
}

fn render_outcome(
    ui: &mut egui::Ui,
    record: &Exchange,
    tab: DetailTab,
    image: &mut image_view::ImagePanel,
) {
    match record.outcome() {
        Outcome::InFlight => {
            ui.weak("Waiting for the upstream server.");
        }
        Outcome::Response(response) => match tab {
            DetailTab::Headers => {
                ui.strong(format!(
                    "Response headers · {} {} · {:?}",
                    response.status().as_str(),
                    response.reason(),
                    response.version()
                ));
                render_headers(ui, "response-headers", response.headers());
            }
            DetailTab::Body => {
                ui.strong("Response body");
                render_body(
                    ui,
                    response.body(),
                    response.headers(),
                    Some((image, record.id())),
                );
            }
        },
        Outcome::Tunnel(tunnel) => {
            ui.label(scope_help::TUNNEL_NOTE);
            render_tunnel(ui, tunnel);
        }
        Outcome::InspectionEstablished => {
            ui.label("HTTPS inspection session established.");
        }
        Outcome::Failed { stage, message } => {
            ui.colored_label(Color32::LIGHT_RED, format!("{stage:?}: {message}"));
        }
        Outcome::Cancelled => {
            ui.colored_label(
                Color32::YELLOW,
                "The client closed the connection before a response arrived.",
            );
        }
        Outcome::Incomplete => {
            ui.colored_label(
                Color32::YELLOW,
                "The proxy stopped before a response arrived.",
            );
        }
    }
}

fn render_timing(ui: &mut egui::Ui, record: &Exchange) {
    let timing = record.timing();
    let (settled_label, settled_value) = match record.outcome() {
        Outcome::InFlight => ("Waiting", elapsed_text(record)),
        Outcome::Response(_) => ("Response headers", optional_millis(timing.settled())),
        Outcome::Tunnel(_) | Outcome::InspectionEstablished => {
            ("Established", optional_millis(timing.settled()))
        }
        Outcome::Failed { .. } | Outcome::Cancelled | Outcome::Incomplete => {
            ("Ended", optional_millis(timing.settled()))
        }
    };
    let total = timing.complete().map_or_else(
        || "in progress".to_owned(),
        |duration| format!("{:.1} ms", elapsed_millis(duration)),
    );
    let rows = [
        ("Connect", optional_millis(timing.connect())),
        ("Send", optional_millis(timing.send())),
        ("Wait (TTFB)", optional_millis(timing.wait())),
        ("Receive", optional_millis(timing.receive())),
        (settled_label, settled_value),
        ("Total", total),
    ];
    egui::Grid::new("timing").num_columns(2).show(ui, |ui| {
        for (label, value) in rows {
            ui.label(label);
            ui.monospace(value);
            ui.end_row();
        }
    });
}

fn optional_millis(duration: Option<Duration>) -> String {
    duration.map_or_else(
        || "not observed".to_owned(),
        |duration| format!("{:.1} ms", elapsed_millis(duration)),
    )
}

fn elapsed_text(record: &Exchange) -> String {
    let millis = elapsed_millis(record.elapsed());
    if record.outcome().is_in_flight() {
        format!("{millis:.0} ms…")
    } else {
        format!("{millis:.1} ms")
    }
}

fn render_tunnel(ui: &mut egui::Ui, tunnel: &TunnelPreview) {
    let snapshot = tunnel.snapshot();
    let state = match snapshot.state() {
        TunnelState::Open => "Open".to_owned(),
        TunnelState::Closed => "Closed".to_owned(),
        TunnelState::Failed { stage, message } => format!("Failed at {stage:?}: {message}"),
        TunnelState::Incomplete => "Incomplete: the proxy stopped while it was open".to_owned(),
    };
    egui::Grid::new("tunnel-details")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("State");
            ui.label(state);
            ui.end_row();
            ui.label("Sent to upstream");
            ui.monospace(format!("{} bytes", snapshot.client_to_upstream()));
            ui.end_row();
            ui.label("Received from upstream");
            ui.monospace(format!("{} bytes", snapshot.upstream_to_client()));
            ui.end_row();
            ui.label("Duration");
            ui.monospace(format!("{:.1} ms", elapsed_millis(snapshot.duration())));
            ui.end_row();
        });
}

fn elapsed_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn copy_menu(ui: &mut egui::Ui, address: std::net::SocketAddr) {
    ui.menu_button("Copy", |ui| {
        for target in port_settings::CopyTarget::ALL {
            let text = target.text(address);
            if ui.button(target.label()).on_hover_text(&text).clicked() {
                ui.ctx().copy_text(text);
                ui.close();
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn default_ca_directory() -> Option<PathBuf> {
    port_settings::default_settings_directory().map(|directory| directory.join("ca"))
}

#[cfg(not(target_os = "macos"))]
fn default_ca_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|directory| directory.join("http-inspector/ca"))
}
