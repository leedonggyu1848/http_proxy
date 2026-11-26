//! Toolbar controls, launch port choice, and clipboard actions.

use crate::ui::app::capture::CaptureNotice;
#[cfg(test)]
use crate::ui::app::capture::PortChoice;
use crate::ui::app::state::MainView;
use crate::ui::style::theme::Palette;
use crate::ui::style::{
    fonts, indicator,
    tokens::{Layout, Space, StrokeWidth, Tone, TypeScale},
};
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::ShellText;
use crate::ui::workbench::{chrome, controls};
use crate::ui::{AppAction, InspectorApp};
use eframe::egui;
use http_inspector_core::interception::{ProxyHandle, StartError};
use http_inspector_core::settings;
use std::borrow::Cow;
use std::io;

/// Explains a failed start in terms of what the user can change.
pub(crate) fn start_error_text(error: &StartError, port: settings::port::ProxyPort) -> String {
    match error {
        StartError::Bind(bind) if bind.kind() == io::ErrorKind::AddrInUse => {
            format!("Port {port} is already in use; enter another port, or 0 for a free one.")
        }
        StartError::Bind(bind) if bind.kind() == io::ErrorKind::PermissionDenied => format!(
            "Port {port} needs administrator privileges; use 1024 to 65535, or 0 for a free one."
        ),
        other => other.to_string(),
    }
}

pub(crate) fn notice_text(notice: &CaptureNotice) -> Cow<'_, str> {
    match notice {
        CaptureNotice::InvalidPort(error) => Cow::Owned(error.to_string()),
        CaptureNotice::MissingCaDirectory => Cow::Borrowed(
            "HTTPS inspection requires an application data directory; check HOME and platform storage settings",
        ),
        CaptureNotice::Start { port, error } => Cow::Owned(start_error_text(error, *port)),
        CaptureNotice::RememberPort(error) => Cow::Owned(error.to_string()),
        CaptureNotice::Stop(error) => Cow::Owned(error.to_string()),
        CaptureNotice::Failed {
            message,
            stop_error,
        } => {
            let message = format!("The proxy stopped: {message}");
            match stop_error {
                Some(error) => Cow::Owned(format!("{message} ({error})")),
                None => Cow::Owned(message),
            }
        }
    }
}

const AUTHORIZATION_WARNING: &str =
    "Only enable inspection for traffic that you are authorized to inspect.";

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
    pub(crate) fn toolbar(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
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
                    .menu(ui, self.session.capture.full_log.is_some())
                    .into_iter()
                    .map(AppAction::SaveMenu),
            );
            if one_row {
                let switch_width = view_switch_width(ui);
                let room = 2.0f32.mul_add(-Space::LG, ui.available_width() - switch_width);
                self.filter_input(ui, Layout::TOOLBAR_FILTER_WIDTH.fit(room), actions);
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
            self.filter_input(&mut row, width, actions);
        }
    }

    /// Requests and Map. The counts live in the status bar (S16).
    fn view_switch(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        actions.extend(
            controls::segmented(
                ui,
                ShellText::ViewSwitch.text(),
                self.layout.main_view,
                &VIEWS.map(|(view, label)| (view, label.text())),
            )
            .map(AppAction::SwitchView),
        );
    }

    /// The request filter. It only filters the request list, so the Map
    /// view leaves its place empty instead of offering a control that does
    /// nothing there.
    fn filter_input(&self, ui: &mut egui::Ui, width: f32, actions: &mut Vec<AppAction>) {
        if self.layout.main_view == MainView::Requests {
            actions.extend(
                self.requests
                    .search
                    .controls(ui, width)
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
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = Space::XS;
            if let Some(proxy) = self.session.capture.proxy.as_ref() {
                let address = proxy.local_addr();
                indicator::dot(ui, Tone::Success);
                let label = egui::RichText::new(format!("{address} ▼")).font(fonts::data());
                let menu = controls::ghost_menu(ui, label, Layout::TOOLBAR_BUTTON_HEIGHT, |ui| {
                    proxy_menu(ui, address)
                });
                if let Some(action) = menu.inner.flatten() {
                    actions.push(action);
                }
            } else {
                indicator::dot(ui, Tone::Danger);
                ui.label(ShellText::Stopped.text());
                ui.add_space(Space::SM);
                ui.label(ShellText::Port.text());
                let mut port = self.session.capture.port.text.clone();
                let edited = field::single_line(
                    ui,
                    &mut port,
                    Field::new(Layout::PORT_INPUT_WIDTH).monospace(),
                    |_| {},
                )
                .on_hover_text("Loopback port to listen on; 0 picks a free port.")
                .changed();
                if edited {
                    actions.push(AppAction::SetPort(port));
                }
            }
        });
    }

    /// HTTPS decryption state; the menu holds the switch and the CA.
    fn https_indicator(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let capture = &self.session.capture;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = Space::XS;
            let label = if capture.https_inspection {
                if let Some(tone) = self.dialogs.ca_panel.tone(capture.ca_directory.as_deref()) {
                    indicator::dot(ui, tone);
                }
                ShellText::HttpsOn
            } else {
                ShellText::HttpsOff
            };
            let menu = controls::ghost_menu(
                ui,
                format!("{} ▼", label.text()),
                Layout::TOOLBAR_BUTTON_HEIGHT,
                |ui| self.https_menu(ui),
            );
            actions.extend(menu.inner.into_iter().flatten());
        });
    }

    fn https_menu(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let capture = &self.session.capture;
        let mut actions = Vec::new();
        let hover = if capture.proxy.is_some() {
            "Stop the proxy to change this setting."
        } else {
            "Decrypt HTTPS in new connections with the app CA."
        };
        let mut enabled = capture.https_inspection;
        let toggled = ui
            .add_enabled(
                capture.proxy.is_none(),
                egui::Checkbox::new(&mut enabled, ShellText::DecryptHttps.text()),
            )
            .on_hover_text(hover)
            .on_disabled_hover_text(hover)
            .changed();
        if toggled {
            actions.push(AppAction::SetHttpsInspection(enabled));
        }
        if capture.https_inspection {
            ui.separator();
            actions.extend(
                self.dialogs
                    .ca_panel
                    .summary(ui, capture.ca_directory.as_deref()),
            );
        }
        actions
    }

    /// Theme, body storage, cURL redaction, and the capture scope help.
    fn settings_menu(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let menu = controls::ghost_menu(
            ui,
            ShellText::Settings.text(),
            Layout::TOOLBAR_BUTTON_HEIGHT,
            |ui| {
                let mut chosen = Vec::new();
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    chosen.extend(self.layout.theme.menu(ui).map(AppAction::SetTheme));
                    ui.separator();
                    chosen.extend(
                        self.session
                            .body_saving
                            .settings_menu(ui)
                            .map(AppAction::SetBodyStorageLimit),
                    );
                    ui.separator();
                    chosen.extend(self.detail.curl.settings(ui));
                    ui.separator();
                    crate::ui::dialogs::scope_help::submenu(
                        ui,
                        self.session
                            .capture
                            .proxy
                            .as_ref()
                            .map(ProxyHandle::local_addr),
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
        if self.session.capture.proxy.is_none() {
            if ui
                .add(controls::primary(ui, ShellText::Start.text()))
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
        let mut shown = false;
        let mut line = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            shown = true;
            notice_row(ui, add);
        };
        if let Some(warning) = self.session.capture.port.launch_warning.as_ref() {
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Warning, warning);
            });
        }
        if let Some(error) = self.session.capture.error.as_ref() {
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, notice_text(error));
            });
        } else if let Some(error) = self.session.error.as_ref() {
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, error);
            });
        }
        if let Some(error) = self.dialogs.browsers.error() {
            line(ui, &mut |ui| {
                indicator::dot_label(ui, Tone::Danger, error);
            });
        }
        if self.detail.curl.has_notice() {
            line(ui, &mut |ui| {
                if self.detail.curl.notice(ui) {
                    actions.push(AppAction::DismissCurlNotice);
                }
            });
        }
        if self.session.capture.https_inspection {
            if self.dialogs.ca_panel.has_messages() {
                line(ui, &mut |ui| {
                    if self.dialogs.ca_panel.messages(ui) {
                        actions.push(AppAction::DismissCaNotice);
                    }
                });
            }
            if self.dialogs.authorization_notice == AuthorizationNotice::Showing {
                line(ui, &mut |ui| {
                    indicator::dot_label(ui, Tone::Warning, AUTHORIZATION_WARNING);
                    if ui.small_button("Dismiss").clicked() {
                        actions.push(AppAction::DismissAuthorization);
                    }
                });
            }
        }
        shown
    }
}

const VIEWS: [(MainView, ShellText); 2] = [
    (MainView::Requests, ShellText::Requests),
    (MainView::Map, ShellText::Map),
];

/// Width the view switch takes, so the toolbar can center it.
fn view_switch_width(ui: &egui::Ui) -> f32 {
    controls::segmented_width(ui, VIEWS.iter().map(|(_, label)| label.text()))
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
fn proxy_menu(ui: &mut egui::Ui, address: std::net::SocketAddr) -> Option<AppAction> {
    let mut action = None;
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.label(egui::RichText::new(ShellText::Copy.text()).color(Palette::of(ui).secondary));
        for target in CopyTarget::ALL {
            let text = target.text(address);
            if ui.button(target.label()).on_hover_text(&text).clicked() {
                action = Some(AppAction::CopyText(text));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(ShellText::Stop.text()).clicked() {
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

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Address => "Proxy URL",
            Self::Curl => "curl example",
            Self::Environment => "HTTP_PROXY variables",
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
        let text = start_error_text(&error, port);
        assert!(text.contains("already in use"), "{text}");
        assert!(text.contains(&port.to_string()), "{text}");
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
    use super::AuthorizationNotice;

    #[test]
    fn warning_shows_only_the_first_time() {
        let shown = AuthorizationNotice::NotShown.inspection_enabled();
        assert_eq!(shown, AuthorizationNotice::Showing);
        assert_eq!(
            AuthorizationNotice::Dismissed.inspection_enabled(),
            AuthorizationNotice::Dismissed
        );
    }
}
