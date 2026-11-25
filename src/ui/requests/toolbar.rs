//! Toolbar controls, launch port choice, and clipboard actions.

use crate::ui::app::capture::CaptureNotice;
#[cfg(test)]
use crate::ui::app::capture::PortChoice;
use crate::ui::style::{
    indicator,
    tokens::{Layout, Tone},
};
use crate::ui::{AppAction, InspectorApp, MAX_RECORDS};
use eframe::egui;
use http_inspector_core::interception::{ProxyHandle, StartError};
use http_inspector_core::recording::FullLog;
use http_inspector_core::settings;
use http_inspector_exchange::ByteSize;
use std::borrow::Cow;
use std::io;

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
            let message = format!("The proxy stopped: {message}. Press Start to restart it.");
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
    /// One toolbar row with the controls used while watching requests.
    pub(crate) fn toolbar(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        ui.horizontal_wrapped(|ui| {
            self.proxy_controls(ui, actions);
            ui.separator();
            self.https_controls(ui, actions);
            ui.separator();
            actions.extend(
                self.dialogs
                    .browsers
                    .toolbar(
                        ui,
                        self.session.capture.proxy.is_some(),
                        self.session.capture.ca_directory.is_some()
                            && self.session.capture.https_inspection,
                    )
                    .map(AppAction::Browser),
            );
            actions.extend(
                self.dialogs
                    .save
                    .menu(ui, self.session.capture.full_log.is_some())
                    .into_iter()
                    .map(AppAction::SaveMenu),
            );
            ui.menu_button("Settings", |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    actions.extend(self.layout.theme.menu(ui).map(AppAction::SetTheme));
                    ui.separator();
                    actions.extend(
                        self.session
                            .body_saving
                            .settings_menu(ui)
                            .map(AppAction::SetBodyStorageLimit),
                    );
                });
            });
            crate::ui::dialogs::scope_help::button(
                ui,
                self.session
                    .capture
                    .proxy
                    .as_ref()
                    .map(ProxyHandle::local_addr),
            );
        });
    }

    fn proxy_controls(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        if let Some(proxy) = self.session.capture.proxy.as_ref() {
            let address = proxy.local_addr();
            indicator::dot_label(ui, Tone::Success, "Running");
            ui.monospace(address.to_string());
            copy_menu(ui, address);
            if ui.button("Stop").clicked() {
                actions.push(AppAction::StopProxy);
            }
        } else {
            indicator::dot_label(ui, Tone::Danger, "Stopped");
            ui.label("Port");
            let mut port = self.session.capture.port.text.clone();
            let edited = ui
                .add(egui::TextEdit::singleline(&mut port).desired_width(Layout::PORT_INPUT_WIDTH))
                .on_hover_text(
                    "Loopback port for the proxy. 0 picks a free port. A port you type is remembered.",
                )
                .changed();
            if edited {
                actions.push(AppAction::SetPort(port));
            }
            if ui.button("Start").clicked() {
                actions.push(AppAction::StartProxy);
            }
        }
    }

    fn https_controls(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let hover = if self.session.capture.proxy.is_some() {
            format!("Stop the proxy to change this setting. {AUTHORIZATION_WARNING}")
        } else {
            format!("Decrypt HTTPS in new connections with the app CA. {AUTHORIZATION_WARNING}")
        };
        let mut enabled = self.session.capture.https_inspection;
        let toggled = ui
            .add_enabled(
                self.session.capture.proxy.is_none(),
                egui::Checkbox::new(&mut enabled, "Decrypt HTTPS"),
            )
            .on_hover_text(&hover)
            .on_disabled_hover_text(&hover)
            .changed();
        if toggled {
            actions.push(AppAction::SetHttpsInspection(enabled));
        }
        if self.session.capture.https_inspection {
            actions.extend(
                self.dialogs
                    .ca_panel
                    .summary(ui, self.session.capture.ca_directory.as_deref())
                    .map(AppAction::Ca),
            );
        }
    }

    /// Errors and warnings that need attention, shown right below the toolbar.
    pub(crate) fn notices(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        if let Some(warning) = self.session.capture.port.launch_warning.as_ref() {
            indicator::dot_label(ui, Tone::Warning, warning);
        }
        if let Some(error) = self.session.capture.error.as_ref() {
            indicator::dot_label(ui, Tone::Danger, notice_text(error));
        } else if let Some(error) = self.session.error.as_ref() {
            indicator::dot_label(ui, Tone::Danger, error);
        }
        if let Some(error) = self.dialogs.browsers.error() {
            indicator::dot_label(ui, Tone::Danger, error);
        }
        if self.detail.curl.notice(ui) {
            actions.push(AppAction::DismissCurlNotice);
        }
        if self.session.capture.https_inspection {
            self.dialogs.ca_panel.messages(ui);
            if self.dialogs.authorization_notice == AuthorizationNotice::Showing {
                ui.horizontal_wrapped(|ui| {
                    indicator::dot_label(ui, Tone::Warning, AUTHORIZATION_WARNING);
                    if ui.small_button("Dismiss").clicked() {
                        actions.push(AppAction::DismissAuthorization);
                    }
                });
            }
        }
    }

    /// Counters and full-session log state below the request list.
    pub(crate) fn status_bar(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} requests", self.requests.records.len()));
            let dropped = self.requests.records.dropped(self.session.capture.engine_dropped());
            if dropped > 0 {
                ui.separator();
                indicator::dot_label(ui, Tone::Danger, format!("{dropped} records dropped"))
                    .on_hover_text(
                        "The capture queue was full, so these exchanges were forwarded but not shown.",
                    );
            }
            let evicted = self.requests.records.evicted();
            if evicted > 0 {
                ui.separator();
                indicator::dot_label(ui, Tone::Warning, format!("{evicted} older records removed"))
                    .on_hover_text(format!(
                        "Only the newest {MAX_RECORDS} requests are kept on screen. The full-session log keeps every record."
                    ));
            }
            if self.map.analysis.url_map().node_limit_reached() {
                ui.separator();
                indicator::dot_label(
                    ui,
                    Tone::Warning,
                    format!(
                        "URL map reached its {} node limit; deeper paths are folded into parent nodes.",
                        self.map.analysis.url_map().max_nodes()
                    ),
                );
            }
            if let Some(error) = self
                .session.capture.log_init_error
                .as_ref()
                .map(|error| format!("Full-session logging is unavailable: {error}"))
                .or_else(|| self.session.capture.full_log.as_ref().and_then(FullLog::error))
            {
                ui.separator();
                indicator::dot_label(ui, Tone::Danger, error);
            }
            if let Some(log) = self
                .session.capture.full_log
                .as_ref()
                .filter(|log| log.body_limit_reached())
            {
                ui.separator();
                indicator::dot_label(
                    ui,
                    Tone::Warning,
                    format!(
                        "The full-session log reached its {} body limit. Later body bytes are omitted; headers and results are still logged.",
                        ByteSize::new(log.body_limit()).exact()
                    ),
                );
            }
            self.dialogs.save.status(ui);
            if self.session.body_saving.status(ui) {
                actions.push(AppAction::CancelBulkSave);
            }
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
