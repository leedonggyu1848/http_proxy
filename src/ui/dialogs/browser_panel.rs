use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::ui::style::controls::primary_button;

use eframe::egui::{self, containers::menu::MenuButton};
use http_inspector_core::launcher::{Browser, BrowserEvent, BrowserSession};

pub(crate) struct BrowserPanel {
    installed: Vec<Browser>,
    session: Option<BrowserSession>,
    opened: bool,
    error: Option<String>,
}

impl Default for BrowserPanel {
    fn default() -> Self {
        Self {
            installed: Browser::installed(),
            session: None,
            opened: false,
            error: None,
        }
    }
}

const PROFILE_NOTE: &str = "Opens a separate private profile. Inspector logs are still recorded. \
     Stopping the proxy closes this browser.";

impl BrowserPanel {
    /// Shows the `Open Browser` menu, or the open session with a close
    /// button, inline in the toolbar. Returns whether a session ended.
    pub(crate) fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        proxy: Option<SocketAddr>,
        ca: Option<&Path>,
    ) -> bool {
        let mut changed = self.receive();
        if self.session.is_some() {
            if self.opened {
                ui.label("Browser open");
            } else {
                ui.spinner();
                ui.label("Opening browser…");
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            if ui
                .button("Close browser")
                .on_hover_text(PROFILE_NOTE)
                .clicked()
            {
                self.close();
                changed = true;
            }
            return changed;
        }
        let Some(address) = proxy else {
            ui.add_enabled(false, primary_button(ui, "Open Browser ▼"))
                .on_disabled_hover_text("Start the proxy to open an inspection browser.");
            return changed;
        };
        MenuButton::from_button(primary_button(ui, "Open Browser ▼"))
            .ui(ui, |ui| self.menu(ui, address, ca))
            .0
            .on_hover_text(PROFILE_NOTE);
        changed
    }

    fn menu(&mut self, ui: &mut egui::Ui, address: SocketAddr, ca: Option<&Path>) {
        if self.installed.is_empty() {
            ui.weak("No supported browser found on PATH. Install Firefox or Google Chrome, then refresh.");
        }
        let mut chosen = None;
        for browser in &self.installed {
            if ui
                .button(browser.name())
                .on_hover_text(ca_note(browser, ca.is_some()))
                .clicked()
            {
                chosen = Some(browser.clone());
            }
        }
        if let Some(browser) = chosen {
            self.error = None;
            self.opened = false;
            match browser.open(address, ca.map(Path::to_owned)) {
                Ok(session) => self.session = Some(session),
                Err(error) => self.error = Some(error.to_string()),
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Refresh browsers").clicked() {
            self.installed = Browser::installed();
        }
    }

    /// The last browser error, shown below the toolbar.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
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
                    self.error = result.err().map(|error| error.to_string());
                }
            }
        }
        changed
    }

    pub(crate) fn close(&mut self) {
        self.session = None;
        self.opened = false;
    }
}

/// How the chosen browser trusts the CA while HTTPS is decrypted.
const fn ca_note(browser: &Browser, decrypt: bool) -> &'static str {
    if !decrypt {
        PROFILE_NOTE
    } else if browser.uses_shared_ca_store() {
        "Registers the CA in your user's shared Chrome/Chromium store. Remove it with Remove CA."
    } else {
        "Firefox trusts the CA only in the temporary inspection profile."
    }
}
