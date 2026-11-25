//! Proxy status and actions of the top bar, launch port choice, notices,
//! the status bar, and clipboard actions.

use crate::ui::dialogs::https_steps::AUTHORIZATION_WARNING;
use crate::ui::style::badge::Badge;
use crate::ui::style::controls::{chip_button, primary_button};
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Space, Tone};
use crate::ui::{InspectorApp, MAX_RECORDS};
use eframe::egui::containers::menu::MenuButton;
use eframe::egui::{self, RichText};
use http_inspector_core::interception::{ProxyHandle, StartError};
use http_inspector_core::recording::FullLog;
use http_inspector_core::settings;
use http_inspector_exchange::ByteSize;
use std::io;
use std::path::PathBuf;

/// Port typed in the toolbar and where its current value came from.
pub(crate) struct PortChoice {
    pub(crate) text: String,
    pub(crate) origin: settings::port::PortOrigin,
    pub(crate) settings_directory: Option<PathBuf>,
    pub(crate) launch_warning: Option<String>,
}

impl PortChoice {
    pub(crate) fn from_launch() -> Self {
        let settings_directory = settings::paths::settings_directory(|name| std::env::var_os(name));
        let saved = settings_directory
            .as_deref()
            .and_then(settings::port::load_port);
        let launch = settings::port::launch_port(
            std::env::args_os(),
            std::env::var_os(settings::port::PORT_ENV),
            saved,
        );
        Self {
            text: launch.port.to_string(),
            origin: launch.origin,
            settings_directory,
            launch_warning: launch.warning,
        }
    }

    /// Remembers a port the user typed once the proxy started on it.
    pub(crate) fn remember(
        &self,
        port: settings::port::ProxyPort,
    ) -> Result<(), settings::port::PortSaveError> {
        if self.origin != settings::port::PortOrigin::User {
            return Ok(());
        }
        let Some(directory) = self.settings_directory.as_deref() else {
            return Err(settings::port::PortSaveError::SettingsDirectoryUnavailable);
        };
        settings::port::save_port(directory, port).map_err(settings::port::PortSaveError::Save)
    }
}

/// Explains a failed start in terms of what the user can change.
pub(crate) fn start_error_text(error: &StartError, port: settings::port::ProxyPort) -> String {
    match error {
        StartError::Bind(bind) if bind.kind() == io::ErrorKind::AddrInUse => format!(
            "Port {port} is already in use by another program. Enter another port, or 0 to pick a free one, then press Start."
        ),
        StartError::Bind(bind) if bind.kind() == io::ErrorKind::PermissionDenied => format!(
            "Port {port} needs administrator privileges. Use a port from 1024 to 65535, or 0 to pick a free one."
        ),
        other => other.to_string(),
    }
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
    /// Left zone of the top bar: a status chip whose menu copies the proxy
    /// address or stops the proxy, or the port and `Start` while stopped.
    pub(crate) fn proxy_status(&mut self, ui: &mut egui::Ui) {
        if let Some(proxy) = self.session.proxy.as_ref() {
            let address = proxy.local_addr();
            let chip = chip_button(ui, format!("● Proxy {address} ▼"), Tone::Success);
            let (response, menu) =
                MenuButton::from_button(chip).ui(ui, |ui| proxy_menu(ui, address));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    true,
                    format!("Proxy running at {address}"),
                )
            });
            if menu.is_some_and(|menu| menu.inner) {
                self.stop_proxy();
            }
        } else {
            ui.add(Badge::status("● Proxy stopped", Tone::Danger));
            ui.add_space(Space::XS);
            ui.label("Port");
            let edited = ui
                .add(egui::TextEdit::singleline(&mut self.session.port.text).desired_width(56.0))
                .on_hover_text(
                    "Loopback port for the proxy. 0 picks a free port. A port you type is remembered.",
                )
                .changed();
            if edited {
                self.session.port.origin = settings::port::PortOrigin::User;
            }
            if ui.add(primary_button(ui, "Start")).clicked() {
                self.session.start_proxy(ui.ctx());
            }
        }
    }

    /// Right zone of the top bar: HTTPS status, then the primary
    /// `Open Browser` and the secondary Save, Settings, and help menus.
    pub(crate) fn top_bar_actions(&mut self, ui: &mut egui::Ui) {
        self.https_status(ui);
        if self.dialogs.browsers.toolbar(
            ui,
            self.session.proxy.as_ref().map(ProxyHandle::local_addr),
            self.session
                .ca_directory
                .as_deref()
                .filter(|_| self.session.https_inspection),
        ) {
            self.dialogs.ca_panel.refresh();
        }
        match self.dialogs.save.menu(ui, self.session.full_log.as_ref()) {
            Some(crate::ui::dialogs::save::SaveMenuAction::ExportHar) => {
                self.dialogs
                    .har
                    .open(self.dialogs.save.suggested_path("har"));
            }
            Some(crate::ui::dialogs::save::SaveMenuAction::SaveBodies) => {
                self.session.body_saving.open_bulk();
            }
            None => {}
        }
        ui.menu_button("Settings", |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                self.layout.theme.menu(ui);
                ui.separator();
                self.session.body_saving.settings_menu(ui);
            });
        });
        crate::ui::dialogs::scope_help::button(
            ui,
            self.session.proxy.as_ref().map(ProxyHandle::local_addr),
        );
    }

    /// Errors and warnings that need attention, shown right below the toolbar.
    pub(crate) fn notices(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        if let Some(warning) = self.session.port.launch_warning.as_ref() {
            ui.colored_label(palette.warning, warning);
        }
        if let Some(error) = self.session.error.as_ref() {
            ui.colored_label(palette.error, error);
        }
        if let Some(error) = self.dialogs.browsers.error() {
            ui.colored_label(palette.error, error);
        }
        self.detail.curl.notice(ui);
        if self.session.https_inspection {
            self.dialogs.ca_panel.messages(ui);
            if self.dialogs.authorization_notice == AuthorizationNotice::Showing {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(palette.warning, AUTHORIZATION_WARNING);
                    if ui.small_button("Dismiss").clicked() {
                        self.dialogs.authorization_notice = AuthorizationNotice::Dismissed;
                    }
                });
            }
        }
    }

    /// Counters and full-session log state below the request list.
    pub(crate) fn status_bar(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} requests", self.requests.records.len()));
            let dropped = self.requests.records.dropped(self.session.engine_dropped());
            if dropped > 0 {
                ui.separator();
                ui.colored_label(palette.error, format!("{dropped} records dropped"))
                    .on_hover_text(
                        "The capture queue was full, so these exchanges were forwarded but not shown.",
                    );
            }
            let evicted = self.requests.records.evicted();
            if evicted > 0 {
                ui.separator();
                ui.colored_label(palette.warning, format!("{evicted} older records removed"))
                    .on_hover_text(format!(
                        "Only the newest {MAX_RECORDS} requests are kept on screen. The full-session log keeps every record."
                    ));
            }
            if self.map.analysis.url_map().node_limit_reached() {
                ui.separator();
                ui.colored_label(
                    palette.warning,
                    format!(
                        "URL map reached its {} node limit; deeper paths are folded into parent nodes.",
                        self.map.analysis.url_map().max_nodes()
                    ),
                );
            }
            if let Some(error) = self
                .session.log_init_error
                .clone()
                .or_else(|| self.session.full_log.as_ref().and_then(FullLog::error))
            {
                ui.separator();
                ui.colored_label(palette.error, error);
            }
            if let Some(log) = self
                .session.full_log
                .as_ref()
                .filter(|log| log.body_limit_reached())
            {
                ui.separator();
                ui.colored_label(
                    palette.warning,
                    format!(
                        "The full-session log reached its {} body limit. Later body bytes are omitted; headers and results are still logged.",
                        ByteSize::new(log.body_limit()).exact()
                    ),
                );
            }
            self.dialogs.save.status(ui);
            self.session.body_saving.status(ui);
            self.layout.theme.status(ui);
        });
    }
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

/// The running proxy's address, copy actions, and `Stop proxy`. Returns
/// whether `Stop proxy` was pressed.
fn proxy_menu(ui: &mut egui::Ui, address: std::net::SocketAddr) -> bool {
    ui.label(RichText::new("Listening on").weak());
    ui.label(RichText::new(CopyTarget::Address.text(address)).monospace());
    ui.add_space(Space::XS);
    ui.label(RichText::new("Copy").strong());
    for target in CopyTarget::ALL {
        let text = target.text(address);
        if ui.button(target.label()).on_hover_text(&text).clicked() {
            ui.ctx().copy_text(text);
            ui.close();
        }
    }
    ui.separator();
    let stop = ui
        .button(RichText::new("Stop proxy").color(Tone::Danger.colors(ui.ctx().theme()).fg))
        .on_hover_text("Stop capturing. A browser opened with Open Browser closes.")
        .clicked();
    if stop {
        ui.close();
    }
    stop
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
