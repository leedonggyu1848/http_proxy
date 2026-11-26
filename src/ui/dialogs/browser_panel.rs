use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::ui::style::tokens::Layout;
use crate::ui::workbench::controls;
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

const PROFILE_NOTE: &str = "Opens a separate private profile that closes when the proxy stops.";

impl BrowserPanel {
    /// The toolbar's primary action while the proxy runs: the Open Browser
    /// menu, or the open session and its Close button. Laid out right to
    /// left, so the button comes first.
    pub(crate) fn toolbar(&self, ui: &mut egui::Ui, decrypt: bool) -> Option<BrowserAction> {
        if self.session.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            let close = controls::ghost_scope(ui, |ui| {
                ui.add(controls::ghost(
                    "Close browser",
                    Layout::TOOLBAR_BUTTON_HEIGHT,
                ))
                .on_hover_text(PROFILE_NOTE)
                .clicked()
            });
            if self.opened {
                ui.label("Browser open");
            } else {
                ui.label("Opening browser…");
                ui.spinner();
            }
            return close.then_some(BrowserAction::Close);
        }
        let menu = controls::primary_menu(ui, "Open Browser ▼", |ui| self.menu(ui, decrypt));
        menu.response.on_hover_text(PROFILE_NOTE);
        menu.inner.flatten()
    }

    fn menu(&self, ui: &mut egui::Ui, decrypt: bool) -> Option<BrowserAction> {
        if self.installed.is_empty() {
            ui.weak("No Firefox or Chrome found on PATH.");
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
