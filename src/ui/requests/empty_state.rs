//! The start checklist shown while nothing has been captured: run the
//! proxy, send traffic through it, and optionally decrypt HTTPS.

use eframe::egui::{self, RichText};
use http_inspector_core::interception::ProxyHandle;

use super::toolbar::CopyTarget;
use crate::ui::InspectorApp;
use crate::ui::dialogs::https_steps::{self, StepStatus};
use crate::ui::style::controls::{StepHeader, primary_button, step_card};
use crate::ui::style::tokens::{Radius, Space, ThemeTokens, Tone, TypeScale};

/// Widest the checklist card grows in a wide pane.
const CARD_WIDTH: f32 = 440.0;

impl InspectorApp {
    /// Three steps to the first captured request, centered in the list pane.
    pub(crate) fn start_checklist(&mut self, ui: &mut egui::Ui) {
        let tokens = ThemeTokens::for_theme(ui.ctx().theme());
        egui::ScrollArea::vertical()
            .id_salt("start-checklist")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(Space::LG);
                ui.vertical_centered(|ui| {
                    let width = 2.0f32
                        .mul_add(-Space::MD, ui.available_width())
                        .clamp(0.0, CARD_WIDTH);
                    egui::Frame::new()
                        .fill(tokens.surfaces.raised)
                        .stroke(egui::Stroke::new(1.0, tokens.surfaces.border))
                        .corner_radius(egui::CornerRadius::same(Radius::CARD))
                        .inner_margin(Space::LG)
                        .show(ui, |ui| {
                            ui.set_width(2.0f32.mul_add(-Space::LG, width));
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                self.checklist_steps(ui);
                            });
                        });
                });
            });
    }

    fn checklist_steps(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Start capturing")
                .strong()
                .size(TypeScale::HEADING),
        );
        ui.add(
            egui::Label::new(
                RichText::new("Requests appear here once a client sends them through the proxy.")
                    .weak(),
            )
            .wrap(),
        );
        ui.add_space(Space::MD);
        let address = self.session.proxy.as_ref().map(ProxyHandle::local_addr);
        self.proxy_step(ui, address);
        ui.add_space(Space::SM);
        self.traffic_step(ui, address);
        ui.add_space(Space::SM);
        self.https_step(ui);
    }

    fn proxy_step(&mut self, ui: &mut egui::Ui, address: Option<std::net::SocketAddr>) {
        let (header, detail) = address.map_or_else(
            || {
                (
                    header(1, "Run the proxy", StepStatus::Needed),
                    "The proxy is stopped. Start it to capture requests.".to_owned(),
                )
            },
            |address| {
                (
                    header(1, "Run the proxy", StepStatus::Done),
                    format!("Listening on {}", CopyTarget::Address.text(address)),
                )
            },
        );
        let start = address.is_none().then_some(|ui: &mut egui::Ui| {
            if ui.add(primary_button(ui, "Start")).clicked() {
                self.session.start_proxy(ui.ctx());
            }
        });
        step_card(ui, &header, &detail, start);
    }

    fn traffic_step(&mut self, ui: &mut egui::Ui, address: Option<std::net::SocketAddr>) {
        let Some(address) = address else {
            step_card(
                ui,
                &header(2, "Send requests through it", StepStatus::Blocked),
                "Start the proxy first.",
                None::<fn(&mut egui::Ui)>,
            );
            return;
        };
        step_card(
            ui,
            &header(2, "Send requests through it", StepStatus::Needed),
            "Open a browser that already uses the proxy, or set your app's HTTP proxy to the address above.",
            Some(|ui: &mut egui::Ui| {
                if self.dialogs.browsers.toolbar(
                    ui,
                    Some(address),
                    self.session
                        .ca_directory
                        .as_deref()
                        .filter(|_| self.session.https_inspection),
                ) {
                    self.dialogs.ca_panel.refresh();
                }
                for (target, label) in [
                    (CopyTarget::Address, "Copy proxy URL"),
                    (CopyTarget::Environment, "Copy HTTP_PROXY variables"),
                ] {
                    let text = target.text(address);
                    if ui.button(label).on_hover_text(&text).clicked() {
                        ui.ctx().copy_text(text);
                    }
                }
            }),
        );
    }

    fn https_step(&self, ui: &mut egui::Ui) {
        let facts = self.https_facts();
        let done = https_steps::steps(&facts)
            .iter()
            .all(|step| step.status == StepStatus::Done);
        let detail = if done {
            "HTTPS requests show their paths, headers, and bodies."
        } else {
            "Without it, HTTPS shows as tunnels with only the host and byte counts."
        };
        let header = StepHeader {
            number: (!done).then_some(3),
            title: "Decrypt HTTPS",
            status: if done { "Done" } else { "Optional" },
            tone: if done { Tone::Success } else { Tone::Neutral },
            muted: false,
        };
        step_card(
            ui,
            &header,
            detail,
            Some(|ui: &mut egui::Ui| {
                if ui.button("Set up HTTPS…").clicked() {
                    egui::Popup::open_id(ui.ctx(), https_steps::panel_id());
                    ui.ctx().request_repaint();
                }
            }),
        );
    }
}

fn header(number: usize, title: &'static str, status: StepStatus) -> StepHeader<'static> {
    StepHeader {
        number: (status != StepStatus::Done).then_some(number),
        title,
        status: status.text(),
        tone: status.tone(),
        muted: status == StepStatus::Blocked,
    }
}
