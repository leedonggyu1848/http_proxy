//! Toolbar controls, launch port choice, and clipboard actions.

use crate::ui::app::capture::CaptureNotice;
#[cfg(test)]
use crate::ui::app::capture::PortChoice;
use crate::ui::app::state::{HlsSessionError, MainView, RangeMergeError, SessionError};
use crate::ui::dialogs::save::localize_directory_error;
use crate::ui::i18n::{Language, domain, toolbar};
use crate::ui::style::theme::Palette;
use crate::ui::style::{
    fonts, indicator,
    tokens::{Layout, Space, StrokeWidth, Tone, TypeScale},
};
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::{chrome, controls};
use crate::ui::{AppAction, InspectorApp};
use eframe::egui;
use http_inspector_core::interception::{ProxyHandle, StartError};
use http_inspector_core::settings;
use std::borrow::Cow;

/// Explains a failed start in terms of what the user can change.
pub(crate) fn start_error_text(
    error: &StartError,
    port: settings::port::ProxyPort,
    lang: Language,
) -> String {
    domain::start_error(lang, error, port)
}

pub(crate) fn notice_text(notice: &CaptureNotice, lang: Language) -> Cow<'_, str> {
    match notice {
        CaptureNotice::InvalidPort(error) => Cow::Owned(domain::port_error(lang, error)),
        CaptureNotice::MissingCaDirectory => Cow::Borrowed(lang.toolbar().missing_ca_directory),
        CaptureNotice::Start { port, error } => Cow::Owned(start_error_text(error, *port, lang)),
        CaptureNotice::RememberPort(error) => Cow::Owned(domain::port_save_error(lang, error)),
        CaptureNotice::Stop(error) => Cow::Owned(domain::stop_error(lang, error)),
        CaptureNotice::Failed {
            message,
            stop_error,
        } => {
            let base = toolbar::proxy_stopped(lang, message);
            match stop_error {
                Some(error) => {
                    let stop_text = domain::stop_error(lang, error);
                    Cow::Owned(format!("{base} ({stop_text})"))
                }
                None => Cow::Owned(base),
            }
        }
    }
}

fn session_error_text(error: &SessionError, lang: Language) -> String {
    let text = lang.toolbar();
    match error {
        SessionError::Replay(err) => domain::replay_error(lang, err),
        SessionError::RangeMerge(err) => {
            let detail = match err {
                RangeMergeError::Directory(io_err) => localize_directory_error(io_err, lang),
                RangeMergeError::SavingUnavailable => {
                    text.range_merge_saving_unavailable.to_owned()
                }
                RangeMergeError::WorkerStopped => text.range_merge_worker_stopped.to_owned(),
                RangeMergeError::Io(io_err) => io_err.to_string(),
            };
            toolbar::range_merge_failed(lang, &detail)
        }
        SessionError::HlsDownload(err) => {
            let detail = match err {
                HlsSessionError::Directory(io_err) => localize_directory_error(io_err, lang),
            };
            toolbar::hls_download_failed(lang, &detail)
        }
    }
}

const fn authorization_warning(lang: Language) -> &'static str {
    lang.toolbar().authorization_warning
}

/// Whether the one-time authorization warning for HTTPS inspection shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AuthorizationNotice {
    #[default]
    NotShown,
    Showing,
    Dismissed,
}

impl AuthorizationNotice {
    /// Shows the warning the first time inspection is turned on.
    pub(crate) const fn inspection_enabled(self) -> Self {
        match self {
            Self::NotShown => Self::Showing,
            Self::Showing | Self::Dismissed => self,
        }
    }
}

impl InspectorApp {
    /// The toolbar: app name, proxy and HTTPS state, the view switch, the
    /// request filter, the Save and Settings menus, and the one primary
    /// action. Windows narrower than [`Layout::TOOLBAR_ONE_ROW_MIN_WIDTH`]
    /// move the view switch and the filter to a second row.
    pub(crate) fn toolbar(
        &self,
        ui: &mut egui::Ui,
        dialog_open: bool,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.language();
        let one_row = ui.available_width() >= Layout::TOOLBAR_ONE_ROW_MIN_WIDTH;
        let rows = if one_row { 1.0 } else { 2.0 };
        let rect = chrome::allocate(ui, Layout::TOOLBAR_HEIGHT * rows);
        chrome::paint(ui, rect, chrome::Edge::Bottom);
        let (top, bottom) = if one_row {
            (rect, None)
        } else {
            let split = rect.top() + Layout::TOOLBAR_HEIGHT;
            let (top, bottom) = rect.split_top_bottom_at_y(split);
            (top, Some(bottom))
        };
        let mut row = chrome::row(ui, top, chrome::Inset::BAR, "toolbar");
        row.spacing_mut().item_spacing.x = Space::MD;
        if one_row {
            title(&mut row);
            chrome::separator(&mut row);
        }
        self.proxy_indicator(&mut row, actions);
        self.https_indicator(&mut row, actions);
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            self.primary_action(ui, actions);
            self.settings_menu(ui, actions);
            actions.extend(
                self.dialogs
                    .save
                    .menu(ui, self.session.capture.full_log.is_some(), lang)
                    .into_iter()
                    .map(AppAction::SaveMenu),
            );
            if one_row {
                let switch_width = view_switch_width(ui, lang);
                let room = 2.0f32.mul_add(-Space::LG, ui.available_width() - switch_width);
                self.filter_input(
                    ui,
                    Layout::TOOLBAR_FILTER_WIDTH.fit(room),
                    dialog_open,
                    actions,
                );
                let free = ui.available_rect_before_wrap();
                let centered = egui::Rect::from_center_size(
                    egui::pos2(
                        free.center().x.max(free.left() + switch_width / 2.0),
                        free.center().y,
                    ),
                    egui::vec2(switch_width, free.height()),
                );
                let mut switch = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(centered)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                self.view_switch(&mut switch, actions);
            }
        });
        if let Some(bottom) = bottom {
            let mut row = chrome::row(ui, bottom, chrome::Inset::BAR, "toolbar-second-row");
            row.spacing_mut().item_spacing.x = Space::MD;
            self.view_switch(&mut row, actions);
            let width = row.available_width();
            self.filter_input(&mut row, width, dialog_open, actions);
        }
    }

    /// Requests and Map. The counts live in the status bar (S16).
    fn view_switch(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        actions.extend(
            controls::segmented(
                ui,
                lang.toolbar().view_switch,
                self.layout.main_view,
                &VIEWS.map(|view| (view, view_label(view, lang))),
            )
            .map(AppAction::SwitchView),
        );
    }

    /// The request filter. It only filters the request list, so the Map
    /// view leaves its place empty instead of offering a control that does
    /// nothing there.
    fn filter_input(
        &self,
        ui: &mut egui::Ui,
        width: f32,
        dialog_open: bool,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.language();
        if self.layout.main_view == MainView::Requests {
            actions.extend(
                self.requests
                    .search
                    .controls(ui, width, &self.requests.records, dialog_open, lang)
                    .map(AppAction::Search),
            );
        } else {
            ui.add_space(width);
        }
    }

    /// The proxy dot and address. While running, the address opens a menu
    /// with the copy actions and Stop; while stopped, the port to start on
    /// is edited in place.
    fn proxy_indicator(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        let text = lang.toolbar();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = Space::XS;
            if let Some(proxy) = self.session.capture.proxy.as_ref() {
                let address = proxy.local_addr();
                indicator::dot(ui, Tone::Success);
                let label = egui::RichText::new(format!("{address} ▼")).font(fonts::data());
                let menu = controls::ghost_menu(ui, label, Layout::TOOLBAR_BUTTON_HEIGHT, |ui| {
                    proxy_menu(ui, address, lang)
                });
                if let Some(action) = menu.inner.flatten() {
                    actions.push(action);
                }
            } else {
                indicator::dot(ui, Tone::Danger);
                ui.label(text.stopped);
                ui.add_space(Space::SM);
                ui.label(text.port);
                let mut port = self.session.capture.port.text.clone();
                let edited = field::single_line(
                    ui,
                    &mut port,
                    Field::new(Layout::PORT_INPUT_WIDTH).monospace(),
                    |_| {},
                )
                .on_hover_text(text.tooltips.port_hint)
                .changed();
                if edited {
                    actions.push(AppAction::SetPort(port));
                }
            }
        });
    }

    /// HTTPS decryption state; the menu holds the switch and the CA.
    fn https_indicator(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        let text = lang.toolbar();
        let capture = &self.session.capture;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = Space::XS;
            let label = if capture.https_inspection {
                if let Some(tone) = self.dialogs.ca_panel.tone(capture.ca_directory.as_deref()) {
                    indicator::dot(ui, tone);
                }
                text.https_on
            } else {
                text.https_off
            };
            let menu = controls::ghost_menu(
                ui,
                format!("{label} ▼"),
                Layout::TOOLBAR_BUTTON_HEIGHT,
                |ui| self.https_menu(ui),
            );
            actions.extend(menu.inner.into_iter().flatten());
        });
    }

    fn https_menu(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let lang = self.language();
        let text = lang.toolbar();
        let capture = &self.session.capture;
        let mut actions = Vec::new();
        let hover = if capture.proxy.is_some() {
            text.tooltips.https_stop_to_change
        } else {
            text.tooltips.https_decrypt_new
        };
        let mut enabled = capture.https_inspection;
        let toggled = ui
            .add_enabled(
                capture.proxy.is_none(),
                egui::Checkbox::new(&mut enabled, text.decrypt_https),
            )
            .on_hover_text(hover)
            .on_disabled_hover_text(hover)
            .changed();
        if toggled {
            actions.push(AppAction::SetHttpsInspection(enabled));
        }
        if capture.https_inspection {
            ui.separator();
            actions.extend(self.dialogs.ca_panel.summary(
                ui,
                capture.ca_directory.as_deref(),
                lang,
            ));
        }
        actions
    }

    /// Language, theme, body storage, cURL redaction, and the capture scope help.
    fn settings_menu(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        let menu = controls::ghost_menu(
            ui,
            lang.toolbar().settings,
            Layout::TOOLBAR_BUTTON_HEIGHT,
            |ui| {
                let mut chosen = Vec::new();
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    chosen.extend(self.layout.theme.menu(ui, lang).map(AppAction::SetTheme));
                    ui.separator();
                    chosen.extend(self.layout.language.menu(ui).map(AppAction::SetLanguage));
                    ui.separator();
                    chosen.extend(
                        self.session
                            .body_saving
                            .settings_menu(ui, lang)
                            .map(AppAction::SetBodyStorageLimit),
                    );
                    ui.separator();
                    chosen.extend(self.detail.curl.settings(ui, lang));
                    ui.separator();
                    crate::ui::dialogs::scope_help::submenu(
                        ui,
                        self.session
                            .capture
                            .proxy
                            .as_ref()
                            .map(ProxyHandle::local_addr),
                        lang,
                    );
                });
                chosen
            },
        );
        actions.extend(menu.inner.into_iter().flatten());
    }

    /// The one Primary button (S9): Start while the proxy is stopped, since
    /// nothing is captured until it runs, and Open Browser while it runs,
    /// since an inspection browser is the shortest way to send traffic
    /// through it.
    fn primary_action(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        if self.session.capture.proxy.is_none() {
            if ui
                .add(controls::primary(ui, lang.toolbar().start))
                .clicked()
            {
                actions.push(AppAction::StartProxy);
            }
            return;
        }
        actions.extend(
            self.dialogs
                .browsers
                .toolbar(
                    ui,
                    self.session.capture.ca_directory.is_some()
                        && self.session.capture.https_inspection,
                    lang,
                )
                .map(AppAction::Browser),
        );
    }

    /// Errors and warnings that need attention, one line each, in a chrome
    /// strip right below the toolbar. Draws nothing when all is well.
    pub(crate) fn notices(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let background = ui.painter().add(egui::Shape::Noop);
        let top = ui.cursor().top();
        let response = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.notice_lines(ui, actions)
        });
        if !response.inner {
            return;
        }
        let rect = egui::Rect::from_x_y_ranges(
            ui.max_rect().x_range(),
            top..=response.response.rect.bottom(),
        );
        let palette = Palette::of(ui);
        ui.painter().set(
            background,
            egui::Shape::rect_filled(rect, 0.0, palette.surfaces.chrome),
        );
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - StrokeWidth::STRUCTURAL / 2.0,
            egui::Stroke::new(StrokeWidth::STRUCTURAL, palette.borders.structural),
        );
    }

    /// Shows each notice on its own line; reports whether any was shown.
    fn notice_lines(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) -> bool {
        let lang = self.language();
        let mut shown = false;
        let mut line = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            shown = true;
            notice_row(ui, add);
        };
        if let Some(warning) = self.session.capture.port.launch_warning.as_ref() {
            let text = domain::localize_port_launch_warning(lang, warning);
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Warning, &text);
            });
        }
        if let Some(error) = self.session.capture.error.as_ref() {
            let text = notice_text(error, lang);
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, text.as_ref());
            });
        } else if let Some(error) = self.session.error.as_ref() {
            let text = session_error_text(error, lang);
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, &text);
            });
        }
        if let Some(error) = self.dialogs.browsers.error(lang) {
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, &error);
            });
        }
        if self.detail.curl.has_notice() {
            line(ui, &mut |ui| {
                if self.detail.curl.notice(ui, lang) {
                    actions.push(AppAction::DismissCurlNotice);
                }
            });
        }
        if self.session.capture.https_inspection {
            if self.dialogs.ca_panel.has_messages() {
                line(ui, &mut |ui| {
                    if self.dialogs.ca_panel.messages(ui, lang) {
                        actions.push(AppAction::DismissCaNotice);
                    }
                });
            }
            if self.dialogs.authorization_notice == AuthorizationNotice::Showing {
                line(ui, &mut |ui| {
                    indicator::dot_label(ui, Tone::Warning, authorization_warning(lang));
                    if ui.small_button(lang.shared().dismiss).clicked() {
                        actions.push(AppAction::DismissAuthorization);
                    }
                });
            }
        }
        shown
    }
}

const VIEWS: [MainView; 2] = [MainView::Requests, MainView::Map];

const fn view_label(view: MainView, lang: Language) -> &'static str {
    match view {
        MainView::Requests => lang.shared().requests,
        MainView::Map => lang.toolbar().map,
    }
}

/// Width the view switch takes, so the toolbar can center it.
fn view_switch_width(ui: &egui::Ui, lang: Language) -> f32 {
    controls::segmented_width(ui, VIEWS.iter().map(|&view| view_label(view, lang)))
}

/// The app name at the left end of the toolbar.
fn title(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(http_inspector_exchange::APP_NAME)
            .font(fonts::panel_title())
            .extra_letter_spacing(TypeScale::PANEL_TITLE_TRACKING)
            .strong(),
    );
}

/// One notice: [`Layout::STATUS_BAR_HEIGHT`] or taller when it wraps.
fn notice_row(ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(Layout::NOTICE_MARGIN)
        .show(ui, |ui| {
            ui.set_min_height(2.0f32.mul_add(-Space::XXS, Layout::STATUS_BAR_HEIGHT));
            ui.horizontal_wrapped(|ui| add(ui));
        });
}

/// Copy actions and Stop for the running proxy at `address`.
fn proxy_menu(
    ui: &mut egui::Ui,
    address: std::net::SocketAddr,
    lang: Language,
) -> Option<AppAction> {
    let mut action = None;
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.label(egui::RichText::new(lang.shared().copy).color(Palette::of(ui).secondary));
        for target in CopyTarget::ALL {
            let text = target.text(address);
            if ui.button(target.label(lang)).on_hover_text(&text).clicked() {
                action = Some(AppAction::CopyText(text));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(lang.toolbar().stop).clicked() {
            action = Some(AppAction::StopProxy);
            ui.close();
        }
    });
    action
}

/// Text placed on the clipboard for one of the toolbar copy actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CopyTarget {
    Address,
    Curl,
    Environment,
}

impl CopyTarget {
    pub(super) const ALL: [Self; 3] = [Self::Address, Self::Curl, Self::Environment];

    pub(super) const fn label(self, lang: Language) -> &'static str {
        let text = lang.toolbar();
        match self {
            Self::Address => text.copy_proxy_url,
            Self::Curl => text.copy_curl_example,
            Self::Environment => text.copy_http_proxy_env,
        }
    }

    pub(super) fn text(self, address: std::net::SocketAddr) -> String {
        let url = format!("http://{address}");
        match self {
            Self::Address => url,
            Self::Curl => format!("curl --proxy {url} http://example.com/"),
            Self::Environment => format!("HTTP_PROXY={url} HTTPS_PROXY={url}"),
        }
    }
}
#[cfg(test)]
mod port_choice_tests {
    use super::{PortChoice, start_error_text};
    use crate::ui::i18n::Language;
    use http_inspector_core::settings::port::{PortOrigin, PortSaveError, ProxyPort};
    use std::io;

    #[test]
    fn remember_reports_typed_errors_and_skips_non_user_ports()
    -> Result<(), Box<dyn std::error::Error>> {
        let port = ProxyPort::parse("8080").map_err(|error| error.to_string())?;
        let mut choice = PortChoice {
            text: port.to_string(),
            origin: PortOrigin::User,
            settings_directory: None,
            launch_warning: None,
        };
        assert!(matches!(
            choice.remember(port),
            Err(PortSaveError::SettingsDirectoryUnavailable)
        ));
        let file = tempfile::NamedTempFile::new()?;
        choice.settings_directory = Some(file.path().to_path_buf());
        assert!(matches!(choice.remember(port), Err(PortSaveError::Save(_))));
        choice.origin = PortOrigin::Environment;
        assert!(choice.remember(port).is_ok());
        Ok(())
    }

    #[test]
    fn explains_an_occupied_port() -> io::Result<()> {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = ProxyPort::from(occupied.local_addr()?.port());
        let Err(error) = http_inspector_core::interception::ProxyHandle::start(
            http_inspector_core::interception::ProxyConfig::loopback(port.value()),
        ) else {
            return Err(io::Error::other("binding an occupied port succeeded"));
        };
        let text = start_error_text(&error, port, Language::English);
        assert!(text.contains("already in use"), "{text}");
        assert!(text.contains(&port.to_string()), "{text}");
        let ko = start_error_text(&error, port, Language::Korean);
        assert!(ko.contains("이미 사용 중"), "{ko}");
        assert!(ko.contains(&port.to_string()), "{ko}");
        Ok(())
    }
}

#[cfg(test)]
mod copy_tests {
    use super::CopyTarget;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    #[test]
    fn formats_copy_texts_for_the_running_address() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18080);
        assert_eq!(CopyTarget::Address.text(address), "http://127.0.0.1:18080");
        assert_eq!(
            CopyTarget::Curl.text(address),
            "curl --proxy http://127.0.0.1:18080 http://example.com/"
        );
        assert_eq!(
            CopyTarget::Environment.text(address),
            "HTTP_PROXY=http://127.0.0.1:18080 HTTPS_PROXY=http://127.0.0.1:18080"
        );
    }
}

#[cfg(test)]
mod authorization_tests {
    use std::io;

    use super::{AuthorizationNotice, session_error_text};
    use crate::ui::app::state::{HlsSessionError, RangeMergeError, SessionError};
    use crate::ui::i18n::Language;

    #[test]
    fn warning_shows_only_the_first_time() {
        let shown = AuthorizationNotice::NotShown.inspection_enabled();
        assert_eq!(shown, AuthorizationNotice::Showing);
        assert_eq!(
            AuthorizationNotice::Dismissed.inspection_enabled(),
            AuthorizationNotice::Dismissed
        );
    }

    #[test]
    fn session_errors_retranslate_when_language_switches() {
        let missing_dir = || {
            io::Error::new(
                io::ErrorKind::NotADirectory,
                "Choose an existing save directory in the Save menu",
            )
        };

        let hls = SessionError::HlsDownload(HlsSessionError::Directory(missing_dir()));
        assert_eq!(
            session_error_text(&hls, Language::English),
            "HLS download failed: Choose an existing save directory in the Save menu"
        );
        assert_eq!(
            session_error_text(&hls, Language::Korean),
            "HLS 다운로드 실패: 저장 메뉴에서 존재하는 저장 폴더를 선택하세요"
        );

        let merge_dir = SessionError::RangeMerge(RangeMergeError::Directory(missing_dir()));
        assert_eq!(
            session_error_text(&merge_dir, Language::English),
            "Merging ranges failed: Choose an existing save directory in the Save menu"
        );
        assert_eq!(
            session_error_text(&merge_dir, Language::Korean),
            "범위 응답 병합 실패: 저장 메뉴에서 존재하는 저장 폴더를 선택하세요"
        );

        let unavailable = SessionError::RangeMerge(RangeMergeError::SavingUnavailable);
        assert_eq!(
            session_error_text(&unavailable, Language::English),
            "Merging ranges failed: Saving bodies is unavailable"
        );
        assert_eq!(
            session_error_text(&unavailable, Language::Korean),
            "범위 응답 병합 실패: 본문 저장을 사용할 수 없습니다"
        );

        let stopped = SessionError::RangeMerge(RangeMergeError::WorkerStopped);
        assert_eq!(
            session_error_text(&stopped, Language::English),
            "Merging ranges failed: The range planning worker stopped unexpectedly"
        );
        assert_eq!(
            session_error_text(&stopped, Language::Korean),
            "범위 응답 병합 실패: 범위 병합 계획 작업자가 예기치 않게 중지되었습니다"
        );
    }
}
