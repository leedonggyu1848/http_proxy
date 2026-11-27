//! Process attribution and connection details for the selected request.

use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use http_inspector_core::process::{ProcessStatus, process_status};
use http_inspector_exchange::{Exchange, ProcessIdentity, ProcessInfo, RecordId};

use crate::ui::AppAction;
use crate::ui::i18n::{Language, process};
use crate::ui::style::tokens::{Layout, Space};
use crate::ui::workbench::{controls, tree};

const STATUS_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct ProcessPanel {
    selected: Option<RecordId>,
    command_open: bool,
    status: Option<(ProcessIdentity, Instant, ProcessStatus)>,
}

impl ProcessPanel {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        record: &Exchange,
        filter_active: bool,
        lang: Language,
        actions: &mut Vec<AppAction>,
    ) {
        let text = lang.process();
        if self.selected != Some(record.id()) {
            self.selected = Some(record.id());
            self.command_open = false;
        }
        let Some(connection) = record.connection() else {
            let message = if record.replay_of().is_some() {
                text.replay
            } else {
                text.unavailable
            };
            notice(ui, message);
            return;
        };
        let key_width = tree::key_width(ui.available_width());
        match connection.process() {
            Some(Ok(info)) => {
                let status = self.status(info.identity, ui.ctx());
                self.details(ui, info, status, key_width, lang);
                if !filter_active {
                    filter_button(ui, text.filter, info.identity, actions);
                }
            }
            Some(Err(reason)) => notice(ui, process::unavailable(lang, *reason)),
            None => {
                ui.ctx().request_repaint_after(STATUS_INTERVAL);
                notice(ui, text.pending);
            }
        }
        field(
            ui,
            key_width,
            text.client_port,
            &connection.peer().port().to_string(),
        );
        field(
            ui,
            key_width,
            text.connection_id,
            &connection.id().to_string(),
        );
        field(
            ui,
            key_width,
            text.requests,
            &connection.request_count().to_string(),
        );
    }

    fn status(&mut self, identity: ProcessIdentity, context: &egui::Context) -> ProcessStatus {
        let now = Instant::now();
        let (_, checked, status) = *self.status.insert(
            self.status
                .filter(|(current, checked, status)| {
                    *current == identity
                        && (*status == ProcessStatus::Exited
                            || now.duration_since(*checked) < STATUS_INTERVAL)
                })
                .unwrap_or_else(|| (identity, now, process_status(identity))),
        );
        if status != ProcessStatus::Exited {
            context
                .request_repaint_after((checked + STATUS_INTERVAL).saturating_duration_since(now));
        }
        status
    }

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        info: &ProcessInfo,
        status: ProcessStatus,
        width: f32,
        lang: Language,
    ) {
        let text = lang.process();
        field(ui, width, text.program, &info.name);
        if let Some(path) = info.executable.as_ref() {
            field(ui, width, text.executable, &path.to_string_lossy());
        }
        field(ui, width, text.pid, &info.identity.pid.to_string());
        if let Some(started) = info.started_at {
            field(ui, width, text.started, &wall_time(started));
        }
        let user = info.user.as_ref().map_or_else(
            || format!("UID {}", info.uid),
            |name| format!("{name} (UID {})", info.uid),
        );
        field(ui, width, text.user, &user);

        match status {
            ProcessStatus::Running => {}
            ProcessStatus::Exited => field(ui, width, text.state, text.exited),
            ProcessStatus::Unknown => field(ui, width, text.state, text.unknown),
        }
        if info.inspection_browser {
            field(ui, width, text.session, text.inspection_browser);
        }
        if !info.parents.is_empty() {
            parents(ui, info, lang);
        }
        if let Some(command) = info.redacted_command.as_deref() {
            self.command(ui, text.command, command);
        }
    }

    fn command(&mut self, ui: &mut egui::Ui, title: &str, command: &str) {
        let row = tree::row(
            ui,
            tree::TreeRow::new(0, title)
                .disclosure(tree::Disclosure::new(true, self.command_open))
                .clickable(),
            |ui| tree::group_title(ui, title, None),
        );
        if row.response.clicked() || row.toggled {
            self.command_open = !self.command_open;
        }
        if self.command_open {
            tree::row(ui, tree::TreeRow::new(1, title), |ui| {
                ui.add(egui::Label::new(tree::value_text(ui, command)).wrap());
            });
        }
    }
}

fn parents(ui: &mut egui::Ui, info: &ProcessInfo, lang: Language) {
    let text = lang.process();
    tree::group(ui, "process-parents", text.parents, None, |ui| {
        for parent in info.parents.iter().rev() {
            let value = format!("{} ({})", parent.name, parent.identity.pid);
            tree::row(ui, tree::TreeRow::new(1, &value), |ui| {
                if info
                    .browser
                    .as_ref()
                    .is_some_and(|browser| browser.identity == parent.identity)
                {
                    ui.label(tree::key_text(ui, text.browser));
                }
                ui.add(egui::Label::new(tree::value_text(ui, &value)).wrap());
            });
        }
    });
}

fn filter_button(
    ui: &mut egui::Ui,
    label: &str,
    identity: ProcessIdentity,
    actions: &mut Vec<AppAction>,
) {
    egui::Frame::new()
        .inner_margin(Layout::PANE_ROW_MARGIN)
        .show(ui, |ui| {
            controls::ghost_style(ui);
            if ui
                .add(controls::ghost(label, Layout::PANE_BUTTON_HEIGHT))
                .clicked()
            {
                actions.push(AppAction::ProcessFilter(Some(identity)));
            }
        });
}

fn field(ui: &mut egui::Ui, width: f32, key: &str, value: &str) {
    tree::row(ui, tree::TreeRow::new(0, key), |ui| {
        tree::key_value(
            ui,
            width,
            tree::key_text(ui, key),
            tree::value_text(ui, value),
        );
    });
}

fn notice(ui: &mut egui::Ui, message: impl Into<String>) {
    egui::Frame::new().inner_margin(Space::SM).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(message.into()).weak()).wrap());
    });
}

fn wall_time(time: SystemTime) -> String {
    let date = time::OffsetDateTime::from(time);
    format!(
        "{} {:02}:{:02}:{:02} UTC",
        date.date(),
        date.hour(),
        date.minute(),
        date.second()
    )
}
