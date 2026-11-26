//! Assembles the app-owned dialogs and collects their actions.

use eframe::egui;

use super::{AppAction, InspectorApp};
use crate::ui::dialogs::har_dialog;

impl InspectorApp {
    pub(super) fn dialog_actions(
        &mut self,
        context: &egui::Context,
        body_dialog_open: bool,
        actions: &mut Vec<AppAction>,
    ) {
        actions.extend(
            self.session
                .body_saving
                .show_confirmation(context)
                .map(AppAction::ConfirmBodySave),
        );
        actions.extend(
            self.detail
                .show_replay(
                    context,
                    self.session.capture.proxy.is_some(),
                    &self.requests.records,
                )
                .into_iter()
                .map(AppAction::Replay),
        );
        if self.dialogs.har.is_open() {
            let counts = har_dialog::ScopeCounts {
                all: self.requests.records.len(),
                filtered: self.requests.filtered_records().len(),
            };
            actions.extend(
                self.dialogs
                    .har
                    .show(
                        context,
                        counts,
                        self.dialogs.save.is_busy(),
                        !body_dialog_open,
                    )
                    .map(AppAction::Har),
            );
        }
        actions.extend(self.show_bulk_save(context).map(AppAction::BulkSave));
        actions.extend(self.show_range_merge(context).map(AppAction::Range));
    }
}
