//! The HTTPS status chip of the top bar, its setup panel, and the changes
//! the panel's buttons make.

use eframe::egui;
use http_inspector_core::interception::ProxyHandle;

use crate::ui::InspectorApp;
use crate::ui::dialogs::https_steps::{self, HttpsFacts, StepAction};
use crate::ui::style::controls::chip_button;

impl InspectorApp {
    /// What the HTTPS steps and chip are derived from.
    pub(crate) fn https_facts(&self) -> HttpsFacts {
        HttpsFacts {
            ca: self
                .dialogs
                .ca_panel
                .state(self.session.ca_directory.as_deref()),
            decrypting: self.session.https_inspection,
            proxy_running: self.session.proxy.is_some(),
            trust_store: self.dialogs.ca_panel.trust_store(),
        }
    }

    /// The HTTPS chip; clicking it opens the setup steps.
    pub(crate) fn https_status(&mut self, ui: &mut egui::Ui) {
        let panel_open = egui::Popup::is_id_open(ui.ctx(), https_steps::panel_id());
        self.dialogs.ca_panel.update(
            self.session.ca_directory.as_deref(),
            panel_open || self.session.https_inspection,
            self.session.https_inspection,
            ui.ctx(),
        );
        let facts = self.https_facts();
        let (text, tone) = https_steps::chip(&facts);
        let response = ui
            .add(chip_button(ui, format!("● {text} ▼"), tone))
            .on_hover_text("HTTPS decryption setup");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{text}. HTTPS setup"))
        });
        let directory = self.session.ca_directory.clone();
        // A panel, not a menu: keep framed buttons instead of menu styling.
        let action = egui::Popup::menu(&response)
            .style(egui::style::StyleModifier::default())
            .id(https_steps::panel_id())
            .layout(egui::Layout::top_down(egui::Align::Min))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .accessible_name("HTTPS setup")
            .show(|ui| https_steps::show(ui, &facts, &self.dialogs.ca_panel, directory.as_deref()))
            .and_then(|inner| inner.inner);
        if let Some(action) = action {
            self.apply_https_action(ui.ctx(), action);
        }
    }

    fn apply_https_action(&mut self, context: &egui::Context, action: StepAction) {
        if let Some(ca_action) = action.ca_action() {
            if let Some(directory) = self.session.ca_directory.clone() {
                self.dialogs.ca_panel.start(ca_action, &directory, context);
            }
            return;
        }
        self.set_decryption(context, action == StepAction::TurnOn);
    }

    /// Turns decryption on or off. A running proxy is restarted on the same
    /// port, because the setting applies only when the proxy starts.
    pub(crate) fn set_decryption(&mut self, context: &egui::Context, on: bool) {
        let running_port = self
            .session
            .proxy
            .as_ref()
            .map(|proxy| ProxyHandle::local_addr(proxy).port());
        if running_port.is_some() {
            self.stop_proxy();
        }
        self.session.https_inspection = on;
        if on {
            self.dialogs.authorization_notice =
                self.dialogs.authorization_notice.inspection_enabled();
        }
        self.dialogs.ca_panel.refresh();
        if let Some(port) = running_port {
            self.session.restart_proxy(context, port);
        }
    }
}
