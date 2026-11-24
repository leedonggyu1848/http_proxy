use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::time::Duration;

use crate::ui::theme::Palette;

use eframe::egui::{self, Color32, RichText};
use http_proxy::https::{CaInfo, CaTrustStatus, CaTrustStore};

#[derive(Default)]
pub(in crate::ui) struct CaPanel {
    info: Option<CaInfo>,
    registration: Option<Registration>,
    pending: Option<Receiver<Event>>,
    pending_action: Option<Action>,
    error: Option<String>,
    notice: Option<&'static str>,
    checked: bool,
}

struct Registration {
    target: &'static str,
    location: PathBuf,
    status: CaTrustStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Check,
    Install,
    Remove,
}

enum Event {
    Prepared(CaInfo),
    Finished(Action, Result<Registration, String>),
}

/// Registration state shown as one colored word in the toolbar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Badge {
    Busy(Action),
    Unknown,
    Failed,
    Status(CaTrustStatus),
}

impl Badge {
    const fn text(self) -> &'static str {
        match self {
            Self::Busy(Action::Check) => "Checking…",
            Self::Busy(Action::Install) => "Registering…",
            Self::Busy(Action::Remove) => "Removing…",
            Self::Unknown => "● Not checked",
            Self::Failed => "● Error",
            Self::Status(CaTrustStatus::NotInstalled) => "● Not registered",
            Self::Status(CaTrustStatus::Trusted) => "● Trusted",
            Self::Status(CaTrustStatus::Untrusted) => "● Registered, not trusted",
        }
    }

    const fn color(self, palette: &Palette) -> Color32 {
        match self {
            Self::Busy(_) | Self::Unknown => palette.muted,
            Self::Status(CaTrustStatus::Trusted) => palette.success,
            Self::Status(CaTrustStatus::NotInstalled) => palette.warning,
            Self::Failed | Self::Status(CaTrustStatus::Untrusted) => palette.error,
        }
    }

    /// The one action the summary offers for this state, if any.
    const fn primary(self) -> Option<Action> {
        match self {
            Self::Busy(_) => None,
            Self::Status(CaTrustStatus::Trusted) => Some(Action::Remove),
            Self::Unknown | Self::Failed | Self::Status(_) => Some(Action::Install),
        }
    }
}

const REGISTER_HELP: &str = "Trust the public CA for HTTPS in Chrome/Chromium for your current user. \
     Other browsers and apps may require separate CA registration.";

#[cfg(target_os = "macos")]
const KEYCHAIN_HELP: &str = "macOS may request authorization. Approve the keychain dialog; cancellation leaves registration incomplete.";

impl CaPanel {
    pub(in crate::ui) const fn refresh(&mut self) {
        self.checked = false;
    }

    const fn badge(&self) -> Badge {
        match (self.pending_action, &self.registration, &self.error) {
            (Some(action), _, _) => Badge::Busy(action),
            (None, Some(registration), _) => Badge::Status(registration.status),
            (None, None, Some(_)) => Badge::Failed,
            (None, None, None) => Badge::Unknown,
        }
    }

    /// Shows the registration badge, the action that fits it, and a
    /// `CA details` menu, inline in the toolbar.
    pub(in crate::ui) fn summary(&mut self, ui: &mut egui::Ui, directory: Option<&Path>) {
        self.receive();
        let palette = Palette::of(ui);
        let Some(directory) = directory else {
            ui.colored_label(palette.error, "● CA storage unavailable")
                .on_hover_text("Check HOME and platform storage settings.");
            return;
        };
        if !self.checked && self.pending_action.is_none() {
            self.start(Action::Check, directory, ui.ctx());
            self.checked = true;
        }

        let badge = self.badge();
        ui.label("CA");
        if matches!(badge, Badge::Busy(_)) {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let badge_button =
            egui::Button::new(RichText::new(badge.text()).color(badge.color(&palette)))
                .frame(false);
        if ui
            .add_enabled(!matches!(badge, Badge::Busy(_)), badge_button)
            .on_hover_text("CA registration status. Click to check again.")
            .clicked()
        {
            self.start(Action::Check, directory, ui.ctx());
        }
        match badge.primary() {
            Some(Action::Install) => {
                if ui
                    .button("Register CA")
                    .on_hover_text(REGISTER_HELP)
                    .clicked()
                {
                    self.start(Action::Install, directory, ui.ctx());
                }
            }
            Some(Action::Remove) => {
                if ui
                    .small_button("Remove CA")
                    .on_hover_text("Remove the CA that HTTP Inspector registered.")
                    .clicked()
                {
                    self.start(Action::Remove, directory, ui.ctx());
                }
            }
            Some(Action::Check) | None => {}
        }
        ui.menu_button("CA details", |ui| self.details(ui, directory));
    }

    /// Paths, fingerprint, and less frequent actions, shown only on request.
    fn details(&mut self, ui: &mut egui::Ui, directory: &Path) {
        let viewport = ui.ctx().content_rect();
        let max_width = (viewport.width() - 32.0).clamp(240.0, 560.0);
        let max_height = (viewport.height() - 80.0).max(160.0);
        ui.set_max_width(max_width);
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                copyable(
                    ui,
                    "Public CA certificate",
                    &directory.join("root-ca.pem").display().to_string(),
                );
                if let Some(info) = &self.info {
                    copyable(ui, "SHA-256 fingerprint", info.fingerprint_sha256());
                }
                if let Some(registration) = &self.registration {
                    copyable(
                        ui,
                        &format!("Trust store: {}", registration.target),
                        &registration.location.display().to_string(),
                    );
                }
                ui.separator();
                ui.weak(REGISTER_HELP);
                #[cfg(target_os = "macos")]
                ui.weak(KEYCHAIN_HELP);
                ui.add_enabled_ui(self.pending_action.is_none(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Check registration").clicked() {
                            self.start(Action::Check, directory, ui.ctx());
                        }
                        let registered = self.registration.as_ref().is_some_and(|registration| {
                            registration.status != CaTrustStatus::NotInstalled
                        });
                        if ui
                            .add_enabled(registered, egui::Button::new("Remove CA"))
                            .clicked()
                        {
                            self.start(Action::Remove, directory, ui.ctx());
                        }
                    });
                });
            });
    }

    /// Errors and follow-up guidance shown right below the toolbar so they
    /// are never hidden in the details menu.
    pub(in crate::ui) fn messages(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        if let Some(error) = &self.error {
            ui.colored_label(palette.error, format!("CA: {error}"));
        }
        #[cfg(target_os = "macos")]
        if matches!(self.pending_action, Some(Action::Install | Action::Remove)) {
            ui.colored_label(palette.info, KEYCHAIN_HELP);
        }
        if let Some(notice) = self.notice {
            ui.colored_label(palette.info, notice);
        }
    }

    fn start(&mut self, action: Action, directory: &Path, context: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.registration = None;
        let directory = directory.to_owned();
        let context = context.clone();
        let (sender, receiver) = mpsc::sync_channel(2);
        match std::thread::Builder::new()
            .name("ca-registration".to_owned())
            .spawn(move || {
                let result = registration(action, &directory, &sender);
                let _receiver_closed = sender.send(Event::Finished(action, result)).is_err();
                context.request_repaint();
            }) {
            Ok(_worker) => {
                self.pending = Some(receiver);
                self.pending_action = Some(action);
            }
            Err(error) => self.error = Some(format!("Could not start CA registration: {error}")),
        }
    }

    fn receive(&mut self) {
        while self.receive_event() {}
    }

    fn receive_event(&mut self) -> bool {
        let Some(receiver) = &self.pending else {
            return false;
        };
        let (action, result) = match receiver.try_recv() {
            Ok(Event::Prepared(info)) => {
                self.info = Some(info);
                return true;
            }
            Ok(Event::Finished(action, result)) => (action, result),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => (
                Action::Check,
                Err("CA registration worker stopped".to_owned()),
            ),
        };
        self.pending = None;
        self.pending_action = None;
        match result {
            Ok(registration) => {
                self.registration = Some(registration);
                self.notice = match action {
                    Action::Check => None,
                    Action::Install | Action::Remove => {
                        Some("Restart Chrome/Chromium after changing CA registration.")
                    }
                };
            }
            Err(error) => self.error = Some(error),
        }
        false
    }
}

fn registration(
    action: Action,
    directory: &Path,
    sender: &SyncSender<Event>,
) -> Result<Registration, String> {
    let info = CaInfo::load_or_create(directory).map_err(|error| error.to_string())?;
    sender
        .send(Event::Prepared(info.clone()))
        .map_err(|error| error.to_string())?;
    let store = CaTrustStore::for_current_user().map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let status = runtime
        .block_on(async {
            match action {
                Action::Check => store.status(&info).await,
                Action::Install => store.install(&info).await,
                Action::Remove => store.remove(&info).await,
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Registration {
        target: CaTrustStore::DESCRIPTION,
        location: store.location().to_owned(),
        status,
    })
}

/// One labelled value with a button that copies it.
fn copyable(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(label);
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(value.to_owned());
        }
    });
    ui.add(egui::Label::new(RichText::new(value).monospace()).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_offers_one_action_that_fits_the_status() {
        assert_eq!(Badge::Busy(Action::Check).primary(), None);
        assert_eq!(Badge::Unknown.primary(), Some(Action::Install));
        assert_eq!(Badge::Failed.primary(), Some(Action::Install));
        for (status, action) in [
            (CaTrustStatus::NotInstalled, Action::Install),
            (CaTrustStatus::Untrusted, Action::Install),
            (CaTrustStatus::Trusted, Action::Remove),
        ] {
            assert_eq!(Badge::Status(status).primary(), Some(action));
        }
    }

    #[test]
    fn badge_colors_follow_severity() {
        let palette = Palette::for_theme(crate::ui::theme::Theme::Dark);
        assert_eq!(
            Badge::Status(CaTrustStatus::Trusted).color(&palette),
            palette.success
        );
        assert_eq!(
            Badge::Status(CaTrustStatus::NotInstalled).color(&palette),
            palette.warning
        );
        assert_eq!(
            Badge::Status(CaTrustStatus::Untrusted).color(&palette),
            palette.error
        );
        assert_eq!(Badge::Failed.color(&palette), palette.error);
    }
}
