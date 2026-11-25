use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::launcher::{Browser, BrowserEvent, BrowserSession};

pub(crate) enum BrowserAction {
    Open(Browser),
    Close,
    Refresh,
}

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
    /// Shows the available browsers or controls for the current session.
    pub(crate) fn toolbar(
        &self,
        ui: &mut egui::Ui,
        proxy_running: bool,
        decrypt: bool,
    ) -> Option<BrowserAction> {
        if self.session.is_some() {
            if self.opened {
                ui.label("Browser open");
            } else {
                ui.spinner();
                ui.label("Opening browser…");
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            return ui
                .button("Close browser")
                .on_hover_text(PROFILE_NOTE)
                .clicked()
                .then_some(BrowserAction::Close);
        }
        if !proxy_running {
            ui.add_enabled(false, egui::Button::new("Open Browser ▼"))
                .on_disabled_hover_text("Start the proxy to open an inspection browser.");
            return None;
        }
        let menu = ui.menu_button("Open Browser ▼", |ui| self.menu(ui, decrypt));
        menu.response.on_hover_text(PROFILE_NOTE);
        menu.inner.flatten()
    }

    fn menu(&self, ui: &mut egui::Ui, decrypt: bool) -> Option<BrowserAction> {
        if self.installed.is_empty() {
            ui.weak("No supported browser found on PATH. Install Firefox or Google Chrome, then refresh.");
        }
        let mut action = None;
        for browser in &self.installed {
            if ui
                .button(browser.name())
                .on_hover_text(ca_note(browser, decrypt))
                .clicked()
            {
                action = Some(BrowserAction::Open(browser.clone()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Refresh browsers").clicked() {
            action = Some(BrowserAction::Refresh);
        }
        action
    }

    pub(crate) fn apply(
        &mut self,
        action: BrowserAction,
        proxy: Option<SocketAddr>,
        ca: Option<&Path>,
    ) {
        match action {
            BrowserAction::Open(browser) => {
                if let Some(address) = proxy {
                    self.error = None;
                    self.opened = false;
                    match browser.open(address, ca.map(Path::to_owned)) {
                        Ok(session) => self.session = Some(session),
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            }
            BrowserAction::Close => self.close(),
            BrowserAction::Refresh => self.installed = Browser::installed(),
        }
    }

    /// The last browser error, shown below the toolbar.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn receive(&mut self) -> bool {
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
