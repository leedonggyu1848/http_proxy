use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

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

const fn profile_note(lang: Language) -> &'static str {
    lang.pick(
        "Opens a separate private profile. Inspector logs are still recorded. \
         Stopping the proxy closes this browser.",
        "별도의 임시 비공개 프로필로 브라우저를 엽니다. Inspector 로그는 계속 기록됩니다. \
         프록시를 중지하면 이 브라우저도 닫힙니다.",
    )
}

impl BrowserPanel {
    /// Shows the available browsers or controls for the current session.
    pub(crate) fn toolbar(
        &self,
        ui: &mut egui::Ui,
        proxy_running: bool,
        decrypt: bool,
        lang: Language,
    ) -> Option<BrowserAction> {
        if self.session.is_some() {
            if self.opened {
                ui.label(lang.pick("Browser open", "브라우저 열림"));
            } else {
                ui.spinner();
                ui.label(lang.pick("Opening browser…", "브라우저 여는 중…"));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            return ui
                .button(lang.pick("Close browser", "브라우저 닫기"))
                .on_hover_text(profile_note(lang))
                .clicked()
                .then_some(BrowserAction::Close);
        }
        let open_label = lang.pick("Open Browser ▼", "브라우저 열기 ▼");
        if !proxy_running {
            ui.add_enabled(false, egui::Button::new(open_label))
                .on_disabled_hover_text(lang.pick(
                    "Start the proxy to open an inspection browser.",
                    "검사용 브라우저를 열려면 먼저 프록시를 시작하세요.",
                ));
            return None;
        }
        let menu = ui.menu_button(open_label, |ui| self.menu(ui, decrypt, lang));
        menu.response.on_hover_text(profile_note(lang));
        menu.inner.flatten()
    }

    fn menu(&self, ui: &mut egui::Ui, decrypt: bool, lang: Language) -> Option<BrowserAction> {
        if self.installed.is_empty() {
            ui.weak(lang.pick(
                "No supported browser found on PATH. Install Firefox or Google Chrome, then refresh.",
                "PATH에서 지원하는 브라우저를 찾지 못했습니다. Firefox 또는 Google Chrome을 설치한 뒤 새로고침하세요.",
            ));
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
        if ui
            .button(lang.pick("Refresh browsers", "브라우저 목록 새로고침"))
            .clicked()
        {
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
    if !decrypt {
        profile_note(lang)
    } else if browser.uses_shared_ca_store() {
        lang.pick(
            "Registers the CA in your user's shared Chrome/Chromium store. Remove it with Remove CA.",
            "현재 사용자의 공용 Chrome/Chromium 저장소에 CA를 등록합니다. 'CA 제거'로 해제할 수 있습니다.",
        )
    } else {
        lang.pick(
            "Firefox trusts the CA only in the temporary inspection profile.",
            "Firefox는 임시 검사 프로필 안에서만 이 CA를 신뢰합니다.",
        )
    }
}
