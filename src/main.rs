#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use http_proxy::{CaptureOutcome, CaptureRecord, ProxyConfig, ProxyHandle};
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
        Box::new(|_| Ok(Box::new(InspectorApp::new()))),
    )
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
    error: Option<String>,
}

impl InspectorApp {
    fn new() -> Self {
        let mut app = Self {
            proxy: None,
            records: VecDeque::with_capacity(MAX_RECORDS),
            error: None,
        };
        app.start_proxy();
        app
    }

    fn start_proxy(&mut self) {
        match ProxyHandle::start(ProxyConfig::loopback(PROXY_PORT)) {
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
            if self.records.len() == MAX_RECORDS {
                self.records.pop_front();
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
            if ui.button("Clear").clicked() {
                self.records.clear();
            }
        });

        if let Some(error) = self.error.as_ref() {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn request_table(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::both()
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

                        for record in self.records.iter().rev() {
                            ui.monospace(record.method().as_str());
                            ui.monospace(record.target());
                            let (result, color) = outcome_text(record.outcome());
                            ui.colored_label(color, result);
                            ui.monospace(format!("{:.1} ms", elapsed_millis(record.elapsed())));
                            ui.end_row();
                        }
                    });
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
        self.request_table(ui);

        if self.proxy.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
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
        CaptureOutcome::Failed { stage, message } => {
            (format!("{stage:?}: {message}"), Color32::LIGHT_RED)
        }
    }
}

fn elapsed_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
