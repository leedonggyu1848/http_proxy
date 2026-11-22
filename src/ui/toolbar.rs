//! Toolbar controls, launch port choice, and clipboard actions.

use crate::ui::theme::Palette;
use crate::ui::{InspectorApp, MAX_RECORDS};
use crate::ui::{app_paths, port_settings};
use eframe::egui::{self, RichText};
use http_proxy::{FullLog, ProxyHandle};
use std::path::PathBuf;

/// Port typed in the toolbar and where its current value came from.
pub(in crate::ui) struct PortChoice {
    pub(in crate::ui) text: String,
    pub(in crate::ui) origin: port_settings::PortOrigin,
    pub(in crate::ui) settings_directory: Option<PathBuf>,
    pub(in crate::ui) launch_warning: Option<String>,
}

impl PortChoice {
    pub(in crate::ui) fn from_launch() -> Self {
        let settings_directory = app_paths::settings_directory(|name| std::env::var_os(name));
        let saved = settings_directory
            .as_deref()
            .and_then(port_settings::load_port);
        let launch = port_settings::launch_port(
            std::env::args_os(),
            std::env::var_os(port_settings::PORT_ENV),
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
    pub(in crate::ui) fn remember(
        &self,
        port: port_settings::ProxyPort,
    ) -> Result<(), port_settings::PortSaveError> {
        if self.origin != port_settings::PortOrigin::User {
            return Ok(());
        }
        let Some(directory) = self.settings_directory.as_deref() else {
            return Err(port_settings::PortSaveError::SettingsDirectoryUnavailable);
        };
        port_settings::save_port(directory, port).map_err(port_settings::PortSaveError::Save)
    }
}

const AUTHORIZATION_WARNING: &str =
    "Only enable inspection for traffic that you are authorized to inspect.";

/// Whether the one-time authorization warning for HTTPS inspection shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) enum AuthorizationNotice {
    #[default]
    NotShown,
    Showing,
    Dismissed,
}

impl AuthorizationNotice {
    /// Shows the warning the first time inspection is turned on.
    const fn inspection_enabled(self) -> Self {
        match self {
            Self::NotShown => Self::Showing,
            Self::Showing | Self::Dismissed => self,
        }
    }
}

impl InspectorApp {
    /// One toolbar row with the controls used while watching requests.
    pub(in crate::ui) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            self.proxy_controls(ui);
            ui.separator();
            self.https_controls(ui);
            ui.separator();
            if self.browsers.toolbar(
                ui,
                self.proxy.as_ref().map(ProxyHandle::local_addr),
                self.ca_directory
                    .as_deref()
                    .filter(|_| self.https_inspection),
            ) {
                self.ca_panel.refresh();
            }
            if self.save.menu(ui, self.full_log.as_ref()) {
                self.har_dialog.open(self.save.suggested_path("har"));
            }
            ui.menu_button("Settings", |ui| self.theme.menu(ui));
            crate::ui::scope_help::button(ui, self.proxy.as_ref().map(ProxyHandle::local_addr));
        });
    }

    fn proxy_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(proxy) = self.proxy.as_ref() {
            let address = proxy.local_addr();
            ui.label(RichText::new("● Running").color(Palette::of(ui).success));
            ui.monospace(address.to_string());
            copy_menu(ui, address);
            if ui.button("Stop").clicked() {
                self.stop_proxy();
            }
        } else {
            ui.label(RichText::new("● Stopped").color(Palette::of(ui).error));
            ui.label("Port");
            let edited = ui
                .add(egui::TextEdit::singleline(&mut self.port.text).desired_width(56.0))
                .on_hover_text(
                    "Loopback port for the proxy. 0 picks a free port. A port you type is remembered.",
                )
                .changed();
            if edited {
                self.port.origin = port_settings::PortOrigin::User;
            }
            if ui.button("Start").clicked() {
                self.start_proxy();
            }
        }
    }

    fn https_controls(&mut self, ui: &mut egui::Ui) {
        let hover = if self.proxy.is_some() {
            format!("Stop the proxy to change this setting. {AUTHORIZATION_WARNING}")
        } else {
            format!("Decrypt HTTPS in new connections with the app CA. {AUTHORIZATION_WARNING}")
        };
        let toggled = ui
            .add_enabled(
                self.proxy.is_none(),
                egui::Checkbox::new(&mut self.https_inspection, "Decrypt HTTPS"),
            )
            .on_hover_text(&hover)
            .on_disabled_hover_text(&hover)
            .changed();
        if toggled && self.https_inspection {
            self.authorization_notice = self.authorization_notice.inspection_enabled();
        }
        if self.https_inspection {
            self.ca_panel.summary(ui, self.ca_directory.as_deref());
        }
    }

    /// Errors and warnings that need attention, shown right below the toolbar.
    pub(in crate::ui) fn notices(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        if let Some(warning) = self.port.launch_warning.as_ref() {
            ui.colored_label(palette.warning, warning);
        }
        if let Some(error) = self.error.as_ref() {
            ui.colored_label(palette.error, error);
        }
        if let Some(error) = self.browsers.error() {
            ui.colored_label(palette.error, error);
        }
        self.curl.notice(ui);
        if self.https_inspection {
            self.ca_panel.messages(ui);
            if self.authorization_notice == AuthorizationNotice::Showing {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(palette.warning, AUTHORIZATION_WARNING);
                    if ui.small_button("Dismiss").clicked() {
                        self.authorization_notice = AuthorizationNotice::Dismissed;
                    }
                });
            }
        }
    }

    /// Counters and full-session log state below the request list.
    pub(in crate::ui) fn status_bar(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} requests", self.records.len()));
            let dropped = self.records.dropped(self.engine_dropped());
            if dropped > 0 {
                ui.separator();
                ui.colored_label(palette.error, format!("{dropped} records dropped"))
                    .on_hover_text(
                        "The capture queue was full, so these exchanges were forwarded but not shown.",
                    );
            }
            let evicted = self.records.evicted();
            if evicted > 0 {
                ui.separator();
                ui.colored_label(palette.warning, format!("{evicted} older records removed"))
                    .on_hover_text(format!(
                        "Only the newest {MAX_RECORDS} requests are kept on screen. The full-session log keeps every record."
                    ));
            }
            if let Some(error) = self
                .log_init_error
                .clone()
                .or_else(|| self.full_log.as_ref().and_then(FullLog::error))
            {
                ui.separator();
                ui.colored_label(palette.error, error);
            }
            if let Some(log) = self
                .full_log
                .as_ref()
                .filter(|log| log.body_limit_reached())
            {
                ui.separator();
                ui.colored_label(
                    palette.warning,
                    format!(
                        "The full-session log reached its {} body limit. Later body bytes are omitted; headers and results are still logged.",
                        log.formatted_body_limit()
                    ),
                );
            }
            self.save.status(ui);
            self.theme.status(ui);
        });
    }
}

/// Text placed on the clipboard for one of the toolbar copy actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum CopyTarget {
    Address,
    Curl,
    Environment,
}

impl CopyTarget {
    pub(in crate::ui) const ALL: [Self; 3] = [Self::Address, Self::Curl, Self::Environment];

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Address => "Proxy URL",
            Self::Curl => "curl example",
            Self::Environment => "HTTP_PROXY variables",
        }
    }

    pub(in crate::ui) fn text(self, address: std::net::SocketAddr) -> String {
        let url = format!("http://{address}");
        match self {
            Self::Address => url,
            Self::Curl => format!("curl --proxy {url} http://example.com/"),
            Self::Environment => format!("HTTP_PROXY={url} HTTPS_PROXY={url}"),
        }
    }
}

fn copy_menu(ui: &mut egui::Ui, address: std::net::SocketAddr) {
    ui.menu_button("Copy ▼", |ui| {
        for target in CopyTarget::ALL {
            let text = target.text(address);
            if ui.button(target.label()).on_hover_text(&text).clicked() {
                ui.ctx().copy_text(text);
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod port_choice_tests {
    use super::{
        PortChoice,
        port_settings::{PortOrigin, PortSaveError, ProxyPort},
    };

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
