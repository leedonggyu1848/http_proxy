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

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use http_proxy::{
    BodyLifecycle, BodyPreview, BodyRetention, CaptureOutcome, CaptureRecord, FullLog,
    HeaderPreview, HttpsInspection, ProxyConfig, ProxyHandle, RecordId,
};
use tracing_subscriber::EnvFilter;

const PROXY_PORT: u16 = 8080;
const MAX_RECORDS: usize = 2_000;

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
            Ok(Box::new(InspectorApp::new()))
        }),
    )
}

fn configure_fonts(context: &egui::Context) {
    let candidates = [
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
    records: VecDeque<CaptureRecord>,
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
}

impl InspectorApp {
    fn new() -> Self {
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (
                None,
                Some(format!("Full-session logging is unavailable: {error}")),
            ),
        };
        let mut app = Self {
            proxy: None,
            records: VecDeque::with_capacity(MAX_RECORDS),
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
        };
        app.start_proxy();
        app
    }

    fn start_proxy(&mut self) {
        let mut config = ProxyConfig::loopback(PROXY_PORT);
        if self.https_inspection {
            let Some(directory) = self.ca_directory.as_ref() else {
                self.error = Some(
                    "HTTPS inspection requires XDG_DATA_HOME or HOME to locate the CA directory"
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
                self.proxy = Some(proxy);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
            }
        }
    }

    fn stop_proxy(&mut self) {
        self.browsers.close();
        self.ca_panel.refresh();
        if let Some(proxy) = self.proxy.take()
            && let Err(error) = proxy.stop()
        {
            self.error = Some(error.to_string());
        }
    }

    fn receive_records(&mut self) {
        let Some(proxy) = self.proxy.as_mut() else {
            return;
        };
        while let Some(record) = proxy.try_receive() {
            if self.records.len() == MAX_RECORDS
                && let Some(removed) = self.records.pop_front()
                && self.selected == Some(removed.id())
            {
                self.selected = None;
            }
            self.records.push_back(record);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(proxy) = self.proxy.as_ref() {
                ui.label(RichText::new("Running").color(Color32::LIGHT_GREEN));
                ui.monospace(proxy.local_addr().to_string());
                if ui.button("Stop").clicked() {
                    self.stop_proxy();
                }
            } else {
                ui.label(RichText::new("Stopped").color(Color32::LIGHT_RED));
                if ui.button("Start").clicked() {
                    self.start_proxy();
                }
            }

            ui.separator();
            ui.label(format!("{} requests", self.records.len()));
            let dropped = self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records);
            if dropped > 0 {
                ui.colored_label(Color32::LIGHT_RED, format!("{dropped} records dropped"));
            }
            if ui
                .button("Clear")
                .on_hover_text("Clear the displayed requests. The full-session log is retained.")
                .clicked()
            {
                self.records.clear();
                self.selected = None;
            }
        });

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

    fn request_table(&mut self, ui: &mut egui::Ui) {
        let query = self.filter_text.to_lowercase();
        let selected = self.selected;
        let mut new_selection = None;
        egui::ScrollArea::both()
            .id_salt("requests-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("request-table")
                    .striped(true)
                    .min_col_width(80.0)
                    .show(ui, |ui| {
                        ui.strong("Method");
                        ui.strong("Target");
                        ui.strong("Result");
                        ui.strong("Time");
                        ui.end_row();

                        for record in self
                            .records
                            .iter()
                            .rev()
                            .filter(|record| record_matches(record, self.record_filter, &query))
                        {
                            if ui
                                .selectable_label(
                                    selected == Some(record.id()),
                                    RichText::new(record.method().as_str()).monospace(),
                                )
                                .clicked()
                            {
                                new_selection = Some(record.id());
                            }
                            ui.monospace(record.target());
                            let (result, color) = outcome_text(record.outcome());
                            ui.colored_label(color, result);
                            ui.monospace(format!("{:.1} ms", elapsed_millis(record.elapsed())));
                            ui.end_row();
                        }
                    });
            });
        if new_selection.is_some() {
            self.selected = new_selection;
        }
    }

    fn detail_panel(&self, ui: &mut egui::Ui) {
        let Some(selected) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a request to inspect headers and body previews.");
            });
            return;
        };
        let Some(record) = self.records.iter().find(|record| record.id() == selected) else {
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt("details-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading(format!("{} {}", record.method(), record.target()));
                ui.label(format!(
                    "Response headers in {:.1} ms",
                    elapsed_millis(record.elapsed())
                ));
                ui.separator();
                ui.strong("Request headers");
                render_headers(ui, "request-headers", record.request_headers());
                ui.separator();
                ui.strong("Request body");
                render_optional_body(ui, record.request_body(), record.request_headers());

                match record.outcome() {
                    CaptureOutcome::Response {
                        status,
                        headers,
                        body,
                    } => {
                        ui.separator();
                        ui.strong(format!("Response headers · {status}"));
                        render_headers(ui, "response-headers", headers);
                        ui.separator();
                        ui.strong("Response body");
                        render_body(ui, body, headers);
                    }
                    CaptureOutcome::TunnelEstablished => {
                        ui.separator();
                        ui.label(
                            "CONNECT tunnel established. Encrypted tunnel contents are not inspected.",
                        );
                    }
                    CaptureOutcome::InspectionEstablished => {
                        ui.separator();
                        ui.label("HTTPS inspection session established.");
                    }
                    CaptureOutcome::Failed { stage, message } => {
                        ui.separator();
                        ui.colored_label(Color32::LIGHT_RED, format!("{stage:?}: {message}"));
                    }
                }
            });
    }
}

impl eframe::App for InspectorApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.receive_records();
        ui.heading(http_proxy::APP_NAME);
        ui.label("Configure an HTTP client to use the proxy shown below.");
        ui.separator();
        self.toolbar(ui);
        ui.separator();
        self.filters(ui);
        ui.separator();
        ui.columns(2, |columns| {
            let [requests, details] = columns else {
                return;
            };
            requests.heading("Requests");
            self.request_table(requests);
            details.heading("Details");
            self.detail_panel(details);
        });

        if self.proxy.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
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

fn record_matches(record: &CaptureRecord, filter: RecordFilter, query: &str) -> bool {
    let outcome_matches = matches!(filter, RecordFilter::All)
        || matches!(
            (filter, record.outcome()),
            (RecordFilter::Responses, CaptureOutcome::Response { .. })
                | (RecordFilter::Failures, CaptureOutcome::Failed { .. })
                | (
                    RecordFilter::Tunnels,
                    CaptureOutcome::TunnelEstablished | CaptureOutcome::InspectionEstablished
                )
        );
    outcome_matches
        && (query.is_empty()
            || text_matches(record.method().as_str(), query)
            || text_matches(record.target(), query)
            || headers_match(record.request_headers(), query)
            || outcome_matches_query(record.outcome(), query))
}

fn outcome_matches_query(outcome: &CaptureOutcome, query: &str) -> bool {
    match outcome {
        CaptureOutcome::Response {
            status, headers, ..
        } => status.to_string().contains(query) || headers_match(headers, query),
        CaptureOutcome::TunnelEstablished => "tunnel".contains(query),
        CaptureOutcome::InspectionEstablished => "https inspection".contains(query),
        CaptureOutcome::Failed { stage, message } => {
            text_matches(&format!("{stage:?}"), query) || text_matches(message, query)
        }
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
        Some(body) => render_body(ui, body, headers),
        None => {
            ui.weak("Body was not observed.");
        }
    }
}

fn render_body(ui: &mut egui::Ui, body: &BodyPreview, headers: &HeaderPreview) {
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
    let encodings: Vec<_> = headers
        .entries()
        .iter()
        .filter(|header| header.name() == "content-encoding")
        .map(http_proxy::CapturedHeader::value)
        .collect();
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

const fn lifecycle_text(lifecycle: BodyLifecycle) -> &'static str {
    match lifecycle {
        BodyLifecycle::Streaming => "streaming",
        BodyLifecycle::Complete => "complete",
        BodyLifecycle::Interrupted => "interrupted",
    }
}

const fn retention_text(retention: BodyRetention) -> &'static str {
    match retention {
        BodyRetention::Prefix => "full preview",
        BodyRetention::Truncated => "truncated preview",
        BodyRetention::Omitted => "preview omitted",
    }
}

fn outcome_text(outcome: &CaptureOutcome) -> (String, Color32) {
    match outcome {
        CaptureOutcome::Response { status, .. } => {
            let color = if *status < 400 {
                Color32::LIGHT_GREEN
            } else {
                Color32::LIGHT_RED
            };
            (status.to_string(), color)
        }
        CaptureOutcome::TunnelEstablished => ("Tunnel".to_owned(), Color32::LIGHT_BLUE),
        CaptureOutcome::InspectionEstablished => ("HTTPS".to_owned(), Color32::LIGHT_BLUE),
        CaptureOutcome::Failed { stage, message } => {
            (format!("{stage:?}: {message}"), Color32::LIGHT_RED)
        }
    }
}

fn elapsed_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn default_ca_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|directory| directory.join("http-inspector/ca"))
}
