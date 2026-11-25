//! HTTPS setup as three ordered steps: create the CA, register it, and turn
//! on decryption. Each step states whether it is done, why it is not, and
//! the button that moves it forward.

use std::path::Path;

use eframe::egui::{self, RichText};
use http_inspector_core::https::CaTrustStatus;

use super::ca_panel::{CaAction, CaPanel, CaState, REGISTER_HELP, RegistrationState};
use crate::ui::style::controls::{StepHeader, primary_button, step_card};
use crate::ui::style::tokens::{Space, Tone, TypeScale};

/// Warning shown wherever decryption can be turned on.
pub(crate) const AUTHORIZATION_WARNING: &str =
    "Only enable inspection for traffic that you are authorized to inspect.";

/// Id of the HTTPS setup popup, so that other screens can open it.
pub(crate) fn panel_id() -> egui::Id {
    egui::Id::unique("https-setup-panel")
}

/// The facts the steps are derived from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HttpsFacts {
    pub(crate) ca: CaState,
    pub(crate) decrypting: bool,
    pub(crate) proxy_running: bool,
    /// Trust store that registration changes, such as `NSS database`.
    pub(crate) trust_store: &'static str,
}

/// Progress of one step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepStatus {
    Done,
    Needed,
    /// Cannot be done yet; the detail says what comes first.
    Blocked,
    Working,
}

impl StepStatus {
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::Needed => "Needed",
            Self::Blocked => "Not yet",
            Self::Working => "Working…",
        }
    }

    pub(crate) const fn tone(self) -> Tone {
        match self {
            Self::Done => Tone::Success,
            Self::Needed => Tone::Warning,
            Self::Blocked | Self::Working => Tone::Neutral,
        }
    }
}

/// What a step button asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepAction {
    /// Create the CA (or retry) and check its registration.
    Create,
    Check,
    Register,
    Remove,
    TurnOn,
    TurnOff,
}

impl StepAction {
    /// The CA task this action starts, if it is a CA action.
    pub(crate) const fn ca_action(self) -> Option<CaAction> {
        match self {
            Self::Create | Self::Check => Some(CaAction::Check),
            Self::Register => Some(CaAction::Install),
            Self::Remove => Some(CaAction::Remove),
            Self::TurnOn | Self::TurnOff => None,
        }
    }
}

/// A labelled step button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StepButton {
    pub(crate) action: StepAction,
    pub(crate) label: &'static str,
    /// What else pressing it does, shown on hover.
    pub(crate) hover: Option<&'static str>,
}

const fn button(action: StepAction, label: &'static str) -> StepButton {
    StepButton {
        action,
        label,
        hover: None,
    }
}

/// Explains the restart behind the decryption buttons of a running proxy.
const RESTART_NOTE: &str = "Restarts the proxy on the same port, because the setting applies when it starts. \
     A browser opened with Open Browser closes; connections opened before stay tunnels.";

const fn restart_button(action: StepAction, label: &'static str) -> StepButton {
    StepButton {
        action,
        label,
        hover: Some(RESTART_NOTE),
    }
}

/// One step with its state, explanation, and buttons.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Step {
    pub(crate) title: &'static str,
    pub(crate) status: StepStatus,
    pub(crate) detail: String,
    /// The emphasized button that moves the step forward.
    pub(crate) primary: Option<StepButton>,
    pub(crate) secondary: Vec<StepButton>,
}

/// The three setup steps for `facts`.
pub(crate) fn steps(facts: &HttpsFacts) -> [Step; 3] {
    let registration = registration_step(facts);
    let registered = registration.status == StepStatus::Done;
    [
        create_step(facts.ca),
        registration,
        decryption_step(facts, registered),
    ]
}

fn create_step(ca: CaState) -> Step {
    let (status, detail, primary) = match ca {
        CaState::StorageUnavailable => (
            StepStatus::Blocked,
            "There is no app data folder to keep a CA in. Check HOME and platform storage settings.",
            None,
        ),
        CaState::Missing { failed: false } => (
            StepStatus::Needed,
            "A private CA on this computer signs a certificate for each site you decrypt. Its private key stays on this computer.",
            Some(button(StepAction::Create, "Create CA")),
        ),
        CaState::Missing { failed: true } => (
            StepStatus::Needed,
            "Creating the CA failed. The error is shown below.",
            Some(button(StepAction::Create, "Try again")),
        ),
        CaState::Creating => (StepStatus::Working, "Creating the CA…", None),
        CaState::Present(_) => (
            StepStatus::Done,
            "Created. The certificate path and fingerprint are under CA details.",
            None,
        ),
    };
    Step {
        title: "Create the CA",
        status,
        detail: detail.to_owned(),
        primary,
        secondary: Vec::new(),
    }
}

fn registration_step(facts: &HttpsFacts) -> Step {
    let store = facts.trust_store;
    let check = button(StepAction::Check, "Check again");
    let register = StepButton {
        hover: Some(REGISTER_HELP),
        ..button(StepAction::Register, "Register CA")
    };
    let remove = StepButton {
        hover: Some("Remove the CA that HTTP Inspector registered."),
        ..button(StepAction::Remove, "Remove CA")
    };
    let (status, detail, primary, secondary) = match facts.ca {
        CaState::Present(RegistrationState::Busy(_)) => (
            StepStatus::Working,
            registration_text(facts.ca).to_owned(),
            None,
            Vec::new(),
        ),
        CaState::Present(RegistrationState::Unknown) => (
            StepStatus::Needed,
            "Registration has not been checked yet.".to_owned(),
            Some(button(StepAction::Check, "Check")),
            Vec::new(),
        ),
        CaState::Present(RegistrationState::Failed) => (
            StepStatus::Needed,
            "The last check or change failed. The error is shown below.".to_owned(),
            Some(register),
            vec![check],
        ),
        CaState::Present(RegistrationState::Status(CaTrustStatus::NotInstalled)) => (
            StepStatus::Needed,
            format!(
                "Chrome/Chromium rejects decrypted sites until the CA is registered in {store}. Firefox opened with Open Browser trusts it on its own."
            ),
            Some(register),
            vec![check],
        ),
        CaState::Present(RegistrationState::Status(CaTrustStatus::Untrusted)) => (
            StepStatus::Needed,
            format!("The CA is in {store} but not trusted for websites. Register it again."),
            Some(register),
            vec![check, remove],
        ),
        CaState::Present(RegistrationState::Status(CaTrustStatus::Trusted)) => (
            StepStatus::Done,
            format!("Trusted for websites in {store}."),
            None,
            vec![check, remove],
        ),
        CaState::StorageUnavailable | CaState::Missing { .. } | CaState::Creating => (
            StepStatus::Blocked,
            "Create the CA first.".to_owned(),
            None,
            Vec::new(),
        ),
    };
    Step {
        title: "Register the CA",
        status,
        detail,
        primary,
        secondary,
    }
}

fn decryption_step(facts: &HttpsFacts, registered: bool) -> Step {
    let (status, detail, primary, secondary) = if facts.decrypting {
        if facts.proxy_running {
            (
                StepStatus::Done,
                "Decrypting HTTPS in new connections.".to_owned(),
                None,
                vec![restart_button(
                    StepAction::TurnOff,
                    "Restart without decryption",
                )],
            )
        } else {
            (
                StepStatus::Done,
                "On. Decryption starts with the proxy.".to_owned(),
                None,
                vec![button(StepAction::TurnOff, "Turn off")],
            )
        }
    } else if facts.ca == CaState::StorageUnavailable {
        (
            StepStatus::Blocked,
            "Needs a place to keep the CA (step 1).".to_owned(),
            None,
            Vec::new(),
        )
    } else {
        let mut detail =
            "HTTPS now passes through as tunnels with only the host and byte counts.".to_owned();
        if !registered {
            detail.push_str(" Until the CA is registered, only Firefox from Open Browser accepts decrypted sites.");
        }
        let primary = if facts.proxy_running {
            restart_button(StepAction::TurnOn, "Restart with decryption")
        } else {
            button(StepAction::TurnOn, "Turn on")
        };
        (StepStatus::Needed, detail, Some(primary), Vec::new())
    };
    Step {
        title: "Turn on decryption",
        status,
        detail,
        primary,
        secondary,
    }
}

const fn registration_text(ca: CaState) -> &'static str {
    match ca {
        CaState::Present(state) => state.text(),
        CaState::StorageUnavailable | CaState::Missing { .. } | CaState::Creating => "",
    }
}

/// Text and tone of the HTTPS status chip in the top bar.
pub(crate) const fn chip(facts: &HttpsFacts) -> (&'static str, Tone) {
    if !facts.decrypting {
        return ("HTTPS off", Tone::Neutral);
    }
    match facts.ca {
        CaState::Present(RegistrationState::Status(CaTrustStatus::Trusted)) => {
            ("HTTPS decrypting", Tone::Success)
        }
        CaState::Present(RegistrationState::Status(CaTrustStatus::NotInstalled)) => {
            ("HTTPS · CA not registered", Tone::Warning)
        }
        CaState::Present(RegistrationState::Status(CaTrustStatus::Untrusted)) => {
            ("HTTPS · CA not trusted", Tone::Warning)
        }
        CaState::Present(RegistrationState::Failed) | CaState::Missing { failed: true } => {
            ("HTTPS · CA error", Tone::Danger)
        }
        CaState::StorageUnavailable => ("HTTPS · no CA storage", Tone::Danger),
        CaState::Present(RegistrationState::Busy(_) | RegistrationState::Unknown)
        | CaState::Missing { failed: false }
        | CaState::Creating => ("HTTPS · checking CA…", Tone::Neutral),
    }
}

/// Shows the steps, CA details, and messages. Returns the button pressed.
pub(crate) fn show(
    ui: &mut egui::Ui,
    facts: &HttpsFacts,
    ca_panel: &CaPanel,
    directory: Option<&Path>,
) -> Option<StepAction> {
    let viewport = ui.ctx().content_rect();
    let width = (viewport.width() - 32.0).clamp(240.0, 400.0);
    let max_height = (viewport.height() - 80.0).max(160.0);
    ui.set_width(width);
    let content_width = width - ui.spacing().scroll.allocated_width();
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .show(ui, |ui| {
            ui.set_width(content_width);
            ui.label(
                RichText::new("HTTPS setup")
                    .strong()
                    .size(TypeScale::HEADING),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Decrypting shows the paths, headers, and bodies of HTTPS requests.",
                    )
                    .weak(),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(AUTHORIZATION_WARNING)
                        .size(TypeScale::SMALL)
                        .color(Tone::Warning.colors(ui.ctx().theme()).fg),
                )
                .wrap(),
            );
            ui.add_space(Space::SM);
            for (index, step) in steps(facts).iter().enumerate() {
                if let Some(action) = step_row(ui, index + 1, step) {
                    chosen = Some(action);
                }
                ui.add_space(Space::SM);
            }
            messages(ui, ca_panel);
            if let Some(directory) = directory
                && matches!(facts.ca, CaState::Present(_))
            {
                egui::CollapsingHeader::new("CA details")
                    .id_salt("https-ca-details")
                    .show(ui, |ui| ca_panel.details(ui, directory));
            }
        });
    chosen
}

/// One numbered step: marker, title, status, explanation, and buttons.
fn step_row(ui: &mut egui::Ui, number: usize, step: &Step) -> Option<StepAction> {
    let mut chosen = None;
    let has_actions =
        step.primary.is_some() || !step.secondary.is_empty() || step.status == StepStatus::Working;
    step_card(
        ui,
        &StepHeader {
            number: (step.status != StepStatus::Done).then_some(number),
            title: step.title,
            status: step.status.text(),
            tone: step.status.tone(),
            muted: step.status == StepStatus::Blocked,
        },
        &step.detail,
        has_actions.then_some(|ui: &mut egui::Ui| {
            if step.status == StepStatus::Working {
                ui.spinner();
            }
            if let Some(primary) = step.primary
                && with_hover(ui.add(primary_button(ui, primary.label)), primary).clicked()
            {
                chosen = Some(primary.action);
            }
            for secondary in &step.secondary {
                if with_hover(ui.button(secondary.label), *secondary).clicked() {
                    chosen = Some(secondary.action);
                }
            }
        }),
    );
    chosen
}

fn with_hover(response: egui::Response, button: StepButton) -> egui::Response {
    match button.hover {
        Some(hover) => response.on_hover_text(hover),
        None => response,
    }
}

fn messages(ui: &mut egui::Ui, ca_panel: &CaPanel) {
    let theme = ui.ctx().theme();
    if let Some(error) = ca_panel.error() {
        ui.add(
            egui::Label::new(
                RichText::new(format!("CA: {error}")).color(Tone::Danger.colors(theme).fg),
            )
            .wrap(),
        );
    }
    if let Some(notice) = ca_panel.notice() {
        ui.add(egui::Label::new(RichText::new(notice).color(Tone::Info.colors(theme).fg)).wrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(ca: CaState, decrypting: bool, proxy_running: bool) -> HttpsFacts {
        HttpsFacts {
            ca,
            decrypting,
            proxy_running,
            trust_store: "Chrome/Chromium (current user)",
        }
    }

    fn statuses(facts: &HttpsFacts) -> [StepStatus; 3] {
        steps(facts).map(|step| step.status)
    }

    #[test]
    fn a_fresh_install_needs_the_ca_and_blocks_registration() {
        let fresh = facts(CaState::Missing { failed: false }, false, true);
        assert_eq!(
            statuses(&fresh),
            [StepStatus::Needed, StepStatus::Blocked, StepStatus::Needed]
        );
        let [create, register, decrypt] = steps(&fresh);
        assert_eq!(
            create.primary.map(|button| button.action),
            Some(StepAction::Create)
        );
        assert!(register.detail.contains("Create the CA first"));
        assert_eq!(register.primary, None);
        // A running proxy is restarted rather than blocking the step.
        assert_eq!(
            decrypt.primary,
            Some(restart_button(
                StepAction::TurnOn,
                "Restart with decryption"
            ))
        );
        assert!(decrypt.detail.contains("only Firefox"));
    }

    #[test]
    fn every_unfinished_step_says_why_and_what_to_press() {
        let cases = [
            CaState::StorageUnavailable,
            CaState::Missing { failed: false },
            CaState::Missing { failed: true },
            CaState::Creating,
            CaState::Present(RegistrationState::Unknown),
            CaState::Present(RegistrationState::Failed),
            CaState::Present(RegistrationState::Busy(CaAction::Install)),
            CaState::Present(RegistrationState::Status(CaTrustStatus::NotInstalled)),
            CaState::Present(RegistrationState::Status(CaTrustStatus::Untrusted)),
            CaState::Present(RegistrationState::Status(CaTrustStatus::Trusted)),
        ];
        for ca in cases {
            for (decrypting, running) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                for step in steps(&facts(ca, decrypting, running)) {
                    assert!(!step.detail.is_empty(), "{ca:?} {}", step.title);
                    if step.status == StepStatus::Needed {
                        assert!(
                            step.primary.is_some(),
                            "{ca:?} {} has no action",
                            step.title
                        );
                    }
                    if step.status == StepStatus::Done {
                        assert_eq!(step.primary, None, "{ca:?} {}", step.title);
                    }
                }
            }
        }
    }

    #[test]
    fn a_trusted_ca_with_decryption_on_completes_every_step() {
        let ready = facts(
            CaState::Present(RegistrationState::Status(CaTrustStatus::Trusted)),
            true,
            true,
        );
        assert_eq!(statuses(&ready), [StepStatus::Done; 3]);
        assert_eq!(chip(&ready), ("HTTPS decrypting", Tone::Success));
        let [_, register, decrypt] = steps(&ready);
        assert!(
            register
                .secondary
                .iter()
                .any(|b| b.action == StepAction::Remove)
        );
        assert_eq!(
            decrypt.secondary,
            vec![restart_button(
                StepAction::TurnOff,
                "Restart without decryption"
            )]
        );
    }

    #[test]
    fn chip_states_the_problem_in_words_and_tone() {
        let off = facts(CaState::Missing { failed: false }, false, true);
        assert_eq!(chip(&off), ("HTTPS off", Tone::Neutral));
        let unregistered = facts(
            CaState::Present(RegistrationState::Status(CaTrustStatus::NotInstalled)),
            true,
            false,
        );
        assert_eq!(
            chip(&unregistered),
            ("HTTPS · CA not registered", Tone::Warning)
        );
        let failed = facts(CaState::Present(RegistrationState::Failed), true, false);
        assert_eq!(chip(&failed).1, Tone::Danger);
    }

    #[test]
    fn step_actions_map_to_ca_tasks() {
        assert_eq!(StepAction::Create.ca_action(), Some(CaAction::Check));
        assert_eq!(StepAction::Register.ca_action(), Some(CaAction::Install));
        assert_eq!(StepAction::Remove.ca_action(), Some(CaAction::Remove));
        assert_eq!(StepAction::TurnOn.ca_action(), None);
    }
}
