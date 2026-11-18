use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, Color32};
use http_proxy::{CaInfo, CaTrustStatus, CaTrustStore};

#[derive(Default)]
pub(super) struct CaPanel {
    info: Option<CaInfo>,
    registration: Option<Registration>,
    pending: Option<Receiver<Event>>,
    error: Option<String>,
    notice: Option<&'static str>,
    checked: bool,
}

struct Registration {
    target: &'static str,
    location: PathBuf,
    status: CaTrustStatus,
}

#[derive(Clone, Copy)]
enum Action {
    Check,
    Install,
    Remove,
}

enum Event {
    Prepared(CaInfo),
    Finished(Action, Result<Registration, String>),
}

impl CaPanel {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, directory: Option<&Path>) {
        self.receive();
        let Some(directory) = directory else {
            ui.colored_label(
                Color32::LIGHT_RED,
                "CA storage requires XDG_DATA_HOME or HOME.",
            );
            return;
        };
        if !self.checked {
            self.start(Action::Check, directory, ui.ctx());
            self.checked = true;
        }

        ui.label("Public CA certificate:");
        ui.monospace(directory.join("root-ca.pem").display().to_string());
        if let Some(info) = &self.info {
            ui.label("SHA-256 fingerprint:");
            ui.monospace(info.fingerprint_sha256());
        }
        ui.label("Register this CA to inspect HTTPS in Chrome/Chromium for your current user.");
        ui.weak("Other browsers and apps may require separate CA registration.");
        if let Some(registration) = &self.registration {
            ui.label(format!("Trust store: {}", registration.target));
            ui.monospace(registration.location.display().to_string());
            ui.label(match registration.status {
                CaTrustStatus::NotInstalled => "CA is not registered by HTTP Inspector.",
                CaTrustStatus::Trusted => "CA is registered and trusted for HTTPS.",
                CaTrustStatus::Untrusted => "CA is registered but is not trusted for HTTPS.",
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(notice) = self.notice {
            ui.label(notice);
        }
        let busy = self.pending.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                let trusted = self
                    .registration
                    .as_ref()
                    .is_some_and(|registration| registration.status == CaTrustStatus::Trusted);
                if ui
                    .add_enabled(!trusted, egui::Button::new("Register CA"))
                    .on_hover_text(
                        "Trust the public CA for HTTPS in your browser certificate store",
                    )
                    .clicked()
                {
                    self.start(Action::Install, directory, ui.ctx());
                }
                if ui.button("Check registration").clicked() {
                    self.start(Action::Check, directory, ui.ctx());
                }
                let registered = self
                    .registration
                    .as_ref()
                    .is_some_and(|registration| registration.status != CaTrustStatus::NotInstalled);
                if ui
                    .add_enabled(registered, egui::Button::new("Remove CA"))
                    .clicked()
                {
                    self.start(Action::Remove, directory, ui.ctx());
                }
            });
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Updating CA registration...");
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
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
