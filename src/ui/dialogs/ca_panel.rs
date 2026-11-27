use crate::ui::style::tokens::Layout;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SendError;
use std::time::Duration;

use crate::ui::AppAction;
use crate::ui::i18n::{self, Language, domain};
use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

use eframe::egui::{self, RichText};
use http_inspector_core::https::{
    CaInfo, CaTrustError, CaTrustStatus, CaTrustStore, HttpsSetupError,
};

use super::background_task::{BackgroundTask, TaskPoll};

#[derive(Default)]
pub(crate) struct CaPanel {
    info: Option<CaInfo>,
    registration: Option<Registration>,
    pending: Option<BackgroundTask<Event>>,
    pending_action: Option<Action>,
    error: Option<CaRegistrationError>,
    notice: Option<NoticeKind>,
    checked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NoticeKind {
    RestartBrowser,
}

impl NoticeKind {
    const fn text(self, lang: Language) -> &'static str {
        match self {
            Self::RestartBrowser => lang.ca().restart_browser_notice,
        }
    }
}

struct Registration {
    location: PathBuf,
    status: CaTrustStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Check,
    Install,
    Remove,
}

#[derive(Debug, thiserror::Error)]
enum CaRegistrationError {
    #[error(transparent)]
    Setup(#[from] HttpsSetupError),
    #[error(transparent)]
    Trust(#[from] CaTrustError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Could not start CA registration: {0}")]
    Start(std::io::Error),
    #[error(transparent)]
    PreparedSend(#[from] SendError<()>),
    #[error("CA registration worker stopped")]
    WorkerStopped,
}

impl CaRegistrationError {
    fn localized(&self, lang: Language) -> String {
        match self {
            Self::Setup(err) => domain::https_setup_error(lang, err),
            Self::Trust(err) => domain::ca_trust_error(lang, err),
            Self::Io(err) => err.to_string(),
            Self::Start(err) => i18n::dialogs::ca::start_failed(lang, err),
            Self::PreparedSend(err) => err.to_string(),
            Self::WorkerStopped => lang.ca().worker_stopped.to_owned(),
        }
    }
}

enum Event {
    Prepared(CaInfo),
    Finished(Action, Result<Registration, CaRegistrationError>),
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
    const fn text(self, lang: Language) -> &'static str {
        let text = lang.ca();
        match self {
            Self::Busy(Action::Check) => text.checking,
            Self::Busy(Action::Install) => text.registering,
            Self::Busy(Action::Remove) => text.removing,
            Self::Unknown => text.not_checked,
            Self::Failed => text.error,
            Self::Status(CaTrustStatus::NotInstalled) => text.not_registered,
            Self::Status(CaTrustStatus::Trusted) => text.trusted,
            Self::Status(CaTrustStatus::Untrusted) => text.registered_untrusted,
        }
    }

    /// Tone of the dot before the status; states still being checked have
    /// none.
    const fn tone(self) -> Option<Tone> {
        match self {
            Self::Busy(_) | Self::Unknown => None,
            Self::Status(CaTrustStatus::Trusted) => Some(Tone::Success),
            Self::Status(CaTrustStatus::NotInstalled) => Some(Tone::Warning),
            Self::Failed | Self::Status(CaTrustStatus::Untrusted) => Some(Tone::Danger),
        }
    }

    /// The one action the summary offers for this state, if any. A trusted
    /// CA needs none; removing it is in the details menu.
    const fn primary(self) -> Option<Action> {
        match self {
            Self::Busy(_) | Self::Status(CaTrustStatus::Trusted) => None,
            Self::Unknown | Self::Failed | Self::Status(_) => Some(Action::Install),
        }
    }
}

impl CaPanel {
    pub(crate) const fn refresh(&mut self) {
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

    pub(crate) fn poll(&mut self, context: &egui::Context, directory: Option<&Path>) {
        self.receive();
        if let Some(directory) = directory
            && !self.checked
            && self.pending_action.is_none()
        {
            self.start(Action::Check, directory, context);
            self.checked = true;
        }
    }

    /// Tone of the HTTPS indicator dot while decryption is on: the CA
    /// registration state, or `Danger` when the CA cannot be stored.
    pub(crate) const fn tone(&self, directory: Option<&Path>) -> Option<Tone> {
        match directory {
            Some(_) => self.badge().tone(),
            None => Some(Tone::Danger),
        }
    }

    /// Shows the registration state, the action it calls for, and the
    /// details menu, in one row of the HTTPS menu.
    pub(crate) fn summary(
        &self,
        ui: &mut egui::Ui,
        directory: Option<&Path>,
        lang: Language,
    ) -> Option<AppAction> {
        let text = lang.ca();
        let Some(directory) = directory else {
            indicator::dot_label(ui, Tone::Danger, text.storage_unavailable)
                .on_hover_text(text.tooltips.storage_unavailable_hint);
            return None;
        };
        ui.horizontal(|ui| self.summary_row(ui, directory, lang))
            .inner
    }

    fn summary_row(
        &self,
        ui: &mut egui::Ui,
        directory: &Path,
        lang: Language,
    ) -> Option<AppAction> {
        let text = lang.ca();
        let mut action = None;
        let badge = self.badge();
        ui.label("CA");
        if matches!(badge, Badge::Busy(_)) {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(tone) = badge.tone() {
            indicator::dot(ui, tone);
        }
        let badge_button = egui::Button::new(badge.text(lang)).frame(false);
        if ui
            .add_enabled(!matches!(badge, Badge::Busy(_)), badge_button)
            .on_hover_text(text.tooltips.badge_status_hint)
            .clicked()
        {
            action = Some(AppAction::Ca(Action::Check));
        }
        if badge.primary() == Some(Action::Install)
            && ui
                .button(text.register_ca)
                .on_hover_text(text.tooltips.register_help)
                .clicked()
        {
            action = Some(AppAction::Ca(Action::Install));
        }
        let menu = ui.menu_button(text.ca_details, |ui| self.details(ui, directory, lang));
        action.or_else(|| menu.inner.flatten())
    }

    /// Paths, fingerprint, and less frequent actions, shown only on request.
    fn details(&self, ui: &mut egui::Ui, directory: &Path, lang: Language) -> Option<AppAction> {
        let text = lang.ca();
        let mut action = None;
        let viewport = ui.ctx().content_rect();
        let max_width = Layout::CA_DETAILS_WIDTH.fit(viewport.width());
        let max_height = Layout::TOOLBAR_POPOVER_HEIGHT.fit(viewport.height());
        ui.set_max_width(max_width);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    action = copyable(
                        ui,
                        text.public_ca_cert,
                        &directory.join("root-ca.pem").display().to_string(),
                        lang,
                    );
                    if let Some(info) = &self.info {
                        action =
                            copyable(ui, text.sha256_fingerprint, info.fingerprint_sha256(), lang)
                                .or_else(|| action.take());
                    }
                    if let Some(registration) = &self.registration {
                        let target = domain::ca_trust_store_description(lang);
                        let label = i18n::dialogs::ca::trust_store_label(lang, target);
                        action = copyable(
                            ui,
                            &label,
                            &registration.location.display().to_string(),
                            lang,
                        )
                        .or_else(|| action.take());
                    }
                    ui.separator();
                    ui.add_enabled_ui(self.pending_action.is_none(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(text.check_registration).clicked() {
                                action = Some(AppAction::Ca(Action::Check));
                            }
                            let registered =
                                self.registration.as_ref().is_some_and(|registration| {
                                    registration.status != CaTrustStatus::NotInstalled
                                });
                            if ui
                                .add_enabled(registered, egui::Button::new(text.remove_ca))
                                .on_hover_text(text.tooltips.remove_ca_hint)
                                .clicked()
                            {
                                action = Some(AppAction::Ca(Action::Remove));
                            }
                        });
                    });
                });
        });
        action
    }

    /// Reports whether [`Self::messages`] has anything to show.
    pub(crate) const fn has_messages(&self) -> bool {
        self.error.is_some()
            || self.notice.is_some()
            || cfg!(target_os = "macos")
                && matches!(self.pending_action, Some(Action::Install | Action::Remove))
    }

    /// Hides the follow-up note after a registration change.
    pub(crate) const fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Errors and follow-up guidance shown right below the toolbar so they
    /// are never hidden in the HTTPS menu. Returns whether the follow-up
    /// note was dismissed.
    pub(crate) fn messages(&self, ui: &mut egui::Ui, lang: Language) -> bool {
        if let Some(error) = &self.error {
            indicator::dot_label(ui, Tone::Danger, format!("CA: {}", error.localized(lang)));
        }
        #[cfg(target_os = "macos")]
        if matches!(self.pending_action, Some(Action::Install | Action::Remove)) {
            ui.label(i18n::dialogs::ca::keychain_help(lang));
        }
        if let Some(notice) = self.notice {
            indicator::dot_label(ui, Tone::Warning, notice.text(lang));
            return ui.small_button(lang.shared().dismiss).clicked();
        }
        false
    }

    pub(crate) fn start(&mut self, action: Action, directory: &Path, context: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.registration = None;
        let directory = directory.to_owned();
        let (task, starter) = BackgroundTask::deferred();
        starter.start_reporting("ca-registration", context, move |report| {
            let result = registration(action, &directory, |info| report(Event::Prepared(info)));
            Ok(Event::Finished(action, result))
        });
        self.pending = Some(task);
        self.pending_action = Some(action);
    }

    fn receive(&mut self) {
        while self.receive_event() {}
    }

    fn receive_event(&mut self) -> bool {
        let Some(task) = &self.pending else {
            return false;
        };
        let (action, result) = match task.poll() {
            TaskPoll::Ready(Ok(Event::Prepared(info))) => {
                self.info = Some(info);
                return true;
            }
            TaskPoll::Ready(Ok(Event::Finished(action, result))) => (action, result),
            TaskPoll::Pending => return false,
            TaskPoll::Ready(Err(error)) => (Action::Check, Err(CaRegistrationError::Start(error))),
            TaskPoll::Disconnected => (Action::Check, Err(CaRegistrationError::WorkerStopped)),
        };
        self.pending = None;
        self.pending_action = None;
        match result {
            Ok(registration) => {
                self.registration = Some(registration);
                self.notice = match action {
                    Action::Check => None,
                    Action::Install | Action::Remove => Some(NoticeKind::RestartBrowser),
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
    report_prepared: impl FnOnce(CaInfo) -> Result<(), SendError<()>>,
) -> Result<Registration, CaRegistrationError> {
    let info = CaInfo::load_or_create(directory)?;
    report_prepared(info.clone())?;
    let store = CaTrustStore::for_current_user()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let status = runtime.block_on(async {
        match action {
            Action::Check => store.status(&info).await,
            Action::Install => store.install(&info).await,
            Action::Remove => store.remove(&info).await,
        }
    })?;
    Ok(Registration {
        location: store.location().to_owned(),
        status,
    })
}

/// One labelled value and the clipboard action offered beside it.
fn copyable(ui: &mut egui::Ui, label: &str, value: &str, lang: Language) -> Option<AppAction> {
    let copied = ui
        .horizontal_wrapped(|ui| {
            ui.strong(label);
            ui.small_button(lang.shared().copy).clicked()
        })
        .inner;
    ui.add(egui::Label::new(RichText::new(value).monospace()).wrap());
    copied.then(|| AppAction::CopyText(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_displays_the_prepared_send_error_verbatim() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        let error = registration(Action::Check, &directory.path().join("ca"), |info| {
            sender
                .send(Event::Prepared(info))
                .map_err(|_| SendError(()))
        })
        .err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(SendError(()).to_string())
        );
        Ok(())
    }

    #[test]
    fn badge_offers_registration_until_the_ca_is_trusted() {
        assert_eq!(Badge::Busy(Action::Check).primary(), None);
        assert_eq!(Badge::Unknown.primary(), Some(Action::Install));
        assert_eq!(Badge::Failed.primary(), Some(Action::Install));
        for (status, action) in [
            (CaTrustStatus::NotInstalled, Some(Action::Install)),
            (CaTrustStatus::Untrusted, Some(Action::Install)),
            (CaTrustStatus::Trusted, None),
        ] {
            assert_eq!(Badge::Status(status).primary(), action);
        }
    }

    #[test]
    fn badge_tones_follow_severity() {
        assert_eq!(
            Badge::Status(CaTrustStatus::Trusted).tone(),
            Some(Tone::Success)
        );
        assert_eq!(
            Badge::Status(CaTrustStatus::NotInstalled).tone(),
            Some(Tone::Warning)
        );
        assert_eq!(
            Badge::Status(CaTrustStatus::Untrusted).tone(),
            Some(Tone::Danger)
        );
        assert_eq!(Badge::Failed.tone(), Some(Tone::Danger));
        assert_eq!(Badge::Busy(Action::Check).tone(), None);
    }

    fn collect_text_shapes(shapes: &[egui::Shape], out: &mut Vec<(String, bool)>) {
        for shape in shapes {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.galley.job.justify));
                }
                egui::Shape::Vec(nested) => collect_text_shapes(nested, out),
                _ => {}
            }
        }
    }

    #[test]
    fn details_popup_text_is_not_justified_inside_menu_layout() {
        let panel = CaPanel {
            registration: Some(Registration {
                location: PathBuf::from(
                    "/tmp/http-inspector-test-trust-store/very/long/path/pki/nssdb",
                ),
                status: CaTrustStatus::NotInstalled,
            }),
            ..CaPanel::default()
        };
        let directory = PathBuf::from("/tmp/http-inspector-test-ca/very/long/directory/path");
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                panel.details(ui, &directory, Language::English);
            });
        });
        output.textures_delta.clear();
        let shapes: Vec<_> = output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect();
        let mut texts = Vec::new();
        collect_text_shapes(&shapes, &mut texts);
        assert!(
            texts.iter().any(|(text, _)| text.contains("root-ca.pem")),
            "expected CA certificate path to render"
        );
        assert!(
            texts.iter().any(|(text, _)| text.contains("pki/nssdb")),
            "expected trust store path to render"
        );
        assert!(
            texts.iter().all(|(_, justified)| !justified),
            "expected no justified text in CA details popup: {texts:?}"
        );
    }
}
