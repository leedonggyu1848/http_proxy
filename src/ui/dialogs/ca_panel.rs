use std::path::{Path, PathBuf};
use std::sync::mpsc::SendError;
use std::time::Duration;

use crate::ui::style::theme::Palette;

use eframe::egui::{self, RichText};
use http_inspector_core::https::{
    CaInfo, CaTrustError, CaTrustStatus, CaTrustStore, HttpsSetupError,
};

use super::background_task::{BackgroundTask, TaskPoll};

/// The app CA: whether it exists, its registration in the user's trust
/// store, and the background task that creates, checks, or changes it.
#[derive(Default)]
pub(crate) struct CaPanel {
    info: Option<CaInfo>,
    registration: Option<Registration>,
    pending: Option<BackgroundTask<Event>>,
    pending_action: Option<CaAction>,
    error: Option<String>,
    notice: Option<&'static str>,
    /// Whether the CA files exist, cached until [`Self::refresh`].
    present: Option<bool>,
    checked: bool,
}

struct Registration {
    target: &'static str,
    location: PathBuf,
    status: CaTrustStatus,
}

/// Work the CA task can do. `Check` also creates a missing CA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaAction {
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

enum Event {
    Prepared(CaInfo),
    Finished(CaAction, Result<Registration, CaRegistrationError>),
}

/// Where the app CA stands, from storage to trust.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaState {
    /// No data directory to keep a CA in.
    StorageUnavailable,
    /// No CA files yet; `failed` when the last attempt to create one failed.
    Missing { failed: bool },
    /// The CA is being created.
    Creating,
    /// The CA exists; its trust store registration is described.
    Present(RegistrationState),
}

/// Registration of an existing CA in the user's trust store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationState {
    Busy(CaAction),
    Unknown,
    Failed,
    Status(CaTrustStatus),
}

impl RegistrationState {
    /// Short present-tense description, such as `Registering…`.
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Busy(CaAction::Check) => "Checking…",
            Self::Busy(CaAction::Install) => "Registering…",
            Self::Busy(CaAction::Remove) => "Removing…",
            Self::Unknown => "Not checked",
            Self::Failed => "Error",
            Self::Status(CaTrustStatus::NotInstalled) => "Not registered",
            Self::Status(CaTrustStatus::Trusted) => "Trusted",
            Self::Status(CaTrustStatus::Untrusted) => "Registered, not trusted",
        }
    }
}

pub(crate) const REGISTER_HELP: &str = "Trust the public CA for HTTPS in Chrome/Chromium for your current user. \
     Other browsers and apps may require separate CA registration.";

#[cfg(target_os = "macos")]
const KEYCHAIN_HELP: &str = "macOS may request authorization. Approve the keychain dialog; cancellation leaves registration incomplete.";

impl CaPanel {
    /// Forgets cached CA facts so that they are read again when next needed.
    pub(crate) const fn refresh(&mut self) {
        self.checked = false;
        self.present = None;
    }

    /// Receives task results and, when `wanted`, checks the registration of
    /// an existing CA once. With `create`, a missing CA is created too, as
    /// turning on decryption does.
    pub(crate) fn update(
        &mut self,
        directory: Option<&Path>,
        wanted: bool,
        create: bool,
        context: &egui::Context,
    ) {
        self.receive();
        let Some(directory) = directory else {
            return;
        };
        if self.pending.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
            return;
        }
        let present = *self
            .present
            .get_or_insert_with(|| CaInfo::exists(directory));
        if !self.checked && (create || (wanted && present)) {
            self.start(CaAction::Check, directory, context);
            self.checked = true;
        }
    }

    /// The CA state for `directory`, the configured CA directory.
    pub(crate) const fn state(&self, directory: Option<&Path>) -> CaState {
        if directory.is_none() {
            return CaState::StorageUnavailable;
        }
        let present = matches!(self.present, Some(true)) || self.info.is_some();
        match (
            present,
            self.pending_action,
            &self.registration,
            &self.error,
        ) {
            (false, Some(_), _, _) => CaState::Creating,
            (false, None, _, error) => CaState::Missing {
                failed: error.is_some(),
            },
            (true, Some(action), _, _) => CaState::Present(RegistrationState::Busy(action)),
            (true, None, Some(registration), _) => {
                CaState::Present(RegistrationState::Status(registration.status))
            }
            (true, None, None, Some(_)) => CaState::Present(RegistrationState::Failed),
            (true, None, None, None) => CaState::Present(RegistrationState::Unknown),
        }
    }

    /// Trust store that registration changes, for example `NSS database`.
    pub(crate) fn trust_store(&self) -> &'static str {
        self.registration
            .as_ref()
            .map_or(CaTrustStore::DESCRIPTION, |registration| {
                registration.target
            })
    }

    /// Paths and fingerprint of the CA, shown on request in the HTTPS panel.
    pub(crate) fn details(&self, ui: &mut egui::Ui, directory: &Path) {
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
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
            ui.weak(REGISTER_HELP);
            #[cfg(target_os = "macos")]
            ui.weak(KEYCHAIN_HELP);
        });
    }

    /// The last CA error, if the last task failed.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Follow-up guidance after the last registration change.
    pub(crate) const fn notice(&self) -> Option<&'static str> {
        self.notice
    }

    /// Errors and follow-up guidance shown right below the toolbar so they
    /// are never hidden in the details menu.
    pub(crate) fn messages(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        if let Some(error) = &self.error {
            ui.colored_label(palette.error, format!("CA: {error}"));
        }
        #[cfg(target_os = "macos")]
        if matches!(
            self.pending_action,
            Some(CaAction::Install | CaAction::Remove)
        ) {
            ui.colored_label(palette.info, KEYCHAIN_HELP);
        }
        if let Some(notice) = self.notice {
            ui.colored_label(palette.info, notice);
        }
    }

    pub(crate) fn start(&mut self, action: CaAction, directory: &Path, context: &egui::Context) {
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
                self.present = Some(true);
                return true;
            }
            TaskPoll::Ready(Ok(Event::Finished(action, result))) => (action, result),
            TaskPoll::Pending => return false,
            TaskPoll::Ready(Err(error)) => {
                (CaAction::Check, Err(CaRegistrationError::Start(error)))
            }
            TaskPoll::Disconnected => (CaAction::Check, Err(CaRegistrationError::WorkerStopped)),
        };
        self.pending = None;
        self.pending_action = None;
        self.present = None;
        match result {
            Ok(registration) => {
                self.registration = Some(registration);
                self.notice = match action {
                    CaAction::Check => None,
                    CaAction::Install | CaAction::Remove => {
                        Some("Restart Chrome/Chromium after changing CA registration.")
                    }
                };
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        false
    }
}

fn registration(
    action: CaAction,
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
            CaAction::Check => store.status(&info).await,
            CaAction::Install => store.install(&info).await,
            CaAction::Remove => store.remove(&info).await,
        }
    })?;
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
    fn registration_displays_the_prepared_send_error_verbatim() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        let error = registration(CaAction::Check, &directory.path().join("ca"), |info| {
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
    fn state_follows_storage_files_task_and_registration() {
        let directory = Path::new("/nonexistent/http-inspector-ca");
        let mut panel = CaPanel::default();
        assert_eq!(panel.state(None), CaState::StorageUnavailable);
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Missing { failed: false }
        );
        panel.pending_action = Some(CaAction::Check);
        assert_eq!(panel.state(Some(directory)), CaState::Creating);
        panel.pending_action = None;
        panel.error = Some("unsafe directory".to_owned());
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Missing { failed: true }
        );
        panel.present = Some(true);
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Present(RegistrationState::Failed)
        );
        panel.error = None;
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Present(RegistrationState::Unknown)
        );
        panel.pending_action = Some(CaAction::Install);
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Present(RegistrationState::Busy(CaAction::Install))
        );
        panel.pending_action = None;
        panel.registration = Some(Registration {
            target: "NSS database",
            location: PathBuf::from("/nonexistent/nssdb"),
            status: CaTrustStatus::Trusted,
        });
        assert_eq!(
            panel.state(Some(directory)),
            CaState::Present(RegistrationState::Status(CaTrustStatus::Trusted))
        );
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
                target: "NSS database",
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
                panel.details(ui, &directory);
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
