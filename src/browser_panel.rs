use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use eframe::egui;
use http_proxy::{Browser, BrowserEvent, BrowserSession};

pub(super) struct BrowserPanel {
    installed: Vec<Browser>,
    selected: Option<Browser>,
    session: Option<BrowserSession>,
    opened: bool,
    error: Option<String>,
}

impl Default for BrowserPanel {
    fn default() -> Self {
        Self {
            installed: Browser::installed(),
            selected: None,
            session: None,
            opened: false,
            error: None,
        }
    }
}

impl BrowserPanel {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        proxy: Option<SocketAddr>,
        ca: Option<&Path>,
    ) -> bool {
        let mut changed = self.receive();
        ui.horizontal(|ui| {
            ui.label("Browser");
            ui.add_enabled_ui(self.session.is_none(), |ui| {
                self.chooser(ui);
                if ui.button("Refresh browsers").clicked() {
                    self.installed = Browser::installed();
                    if !self
                        .selected
                        .as_ref()
                        .is_some_and(|browser| self.installed.contains(browser))
                    {
                        self.selected = None;
                    }
                }
                if ui
                    .add_enabled(
                        proxy.is_some() && self.selected.is_some(),
                        egui::Button::new("Open Browser"),
                    )
                    .clicked()
                    && let (Some(browser), Some(address)) = (&self.selected, proxy)
                {
                    self.error = None;
                    self.opened = false;
                    match browser.open(address, ca.map(Path::to_owned)) {
                        Ok(session) => self.session = Some(session),
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            });
            if self.session.is_some() && ui.button("Close browser").clicked() {
                self.close();
                changed = true;
            }
        });
        self.status(ui, ca.is_some());
        changed
    }

    fn chooser(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_id_salt("inspection-browser")
            .selected_text(
                self.selected
                    .as_ref()
                    .map_or("Choose a browser", Browser::name),
            )
            .show_ui(ui, |ui| {
                for browser in &self.installed {
                    ui.selectable_value(&mut self.selected, Some(browser.clone()), browser.name());
                }
            });
    }

    fn status(&self, ui: &mut egui::Ui, decrypt: bool) {
        if self.installed.is_empty() {
            ui.weak("No supported browser found on PATH. Install Firefox or Google Chrome, then refresh.");
        }
        if self.session.is_some() {
            ui.label(if self.opened {
                "Private inspection browser is open."
            } else {
                "Preparing private browser..."
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::theme::Palette::of(ui).error, error);
        }
        if decrypt
            && self
                .selected
                .as_ref()
                .is_some_and(Browser::uses_shared_ca_store)
        {
            ui.weak("Open Browser registers the CA in your user's shared Chrome/Chromium store. Remove it with Remove CA.");
        } else if decrypt && self.selected.is_some() {
            ui.weak("Firefox trusts the CA only in the temporary inspection profile.");
        }
        ui.weak("Uses a separate private profile. Inspector logs are still recorded. Stopping the proxy closes this browser.");
    }

    fn receive(&mut self) -> bool {
        let mut changed = false;
        while let Some(event) = self.session.as_mut().and_then(BrowserSession::try_receive) {
            changed = true;
            match event {
                BrowserEvent::Opened => self.opened = true,
                BrowserEvent::Finished(result) => {
                    self.session = None;
                    self.opened = false;
                    self.error = result.err();
                }
            }
        }
        changed
    }

    pub(super) fn close(&mut self) {
        self.session = None;
        self.opened = false;
    }
}
