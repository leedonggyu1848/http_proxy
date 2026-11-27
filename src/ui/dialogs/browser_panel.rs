use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::ui::style::tokens::Layout;
use crate::ui::workbench::controls;
use eframe::egui;
use http_inspector_core::launcher::{Browser, BrowserEvent, BrowserLaunchError, BrowserSession};

use crate::ui::i18n::{Language, domain};

pub(crate) enum BrowserAction {
    Open(Browser),
    Close,
    Refresh,
}

pub(crate) struct BrowserPanel {
    installed: Vec<Browser>,
    session: Option<BrowserSession>,
    opened: bool,
    error: Option<BrowserLaunchError>,
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

impl BrowserPanel {
    /// The toolbar's primary action while the proxy runs: the Open Browser
    /// menu, or the open session and its Close button. Laid out right to
    /// left, so the button comes first.
    pub(crate) fn toolbar(
        &self,
        ui: &mut egui::Ui,
        decrypt: bool,
        lang: Language,
    ) -> Option<BrowserAction> {
        let text = lang.browser();
        if self.session.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            let close = controls::ghost_scope(ui, |ui| {
                ui.add(controls::ghost(
                    text.close_browser,
                    Layout::TOOLBAR_BUTTON_HEIGHT,
                ))
                .on_hover_text(text.tooltips.profile)
                .clicked()
            });
            if self.opened {
                ui.label(text.browser_open);
            } else {
                ui.label(text.opening_browser);
                ui.spinner();
            }
            return close.then_some(BrowserAction::Close);
        }
        let menu = controls::primary_menu(ui, text.open_browser_menu, |ui| {
            self.menu(ui, decrypt, lang)
        });
        menu.response.on_hover_text(text.tooltips.profile);
        menu.inner.flatten()
    }

    fn menu(&self, ui: &mut egui::Ui, decrypt: bool, lang: Language) -> Option<BrowserAction> {
        let text = lang.browser();
        if self.installed.is_empty() {
            ui.weak(text.no_browsers_found);
        }
        let mut action = None;
        for browser in &self.installed {
            if ui
                .button(browser.name())
                .on_hover_text(ca_note(browser, decrypt, lang))
                .clicked()
            {
                action = Some(BrowserAction::Open(browser.clone()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(text.refresh_browsers).clicked() {
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
                        Err(error) => self.error = Some(BrowserLaunchError::Io(error)),
                    }
                }
            }
            BrowserAction::Close => self.close(),
            BrowserAction::Refresh => self.installed = Browser::installed(),
        }
    }

    /// The last browser error, shown below the toolbar.
    pub(crate) fn error(&self, lang: Language) -> Option<String> {
        self.error
            .as_ref()
            .map(|error| domain::browser_launch_error(lang, error))
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
                    self.error = result.err();
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
const fn ca_note(browser: &Browser, decrypt: bool, lang: Language) -> &'static str {
    let tips = &lang.browser().tooltips;
    if !decrypt {
        tips.profile
    } else if browser.uses_shared_ca_store() {
        tips.chrome_shared_ca
    } else {
        tips.firefox_profile_ca
    }
}
