//! The `Export HAR` window: which records, what to include, and where to save.

use crate::ui::i18n::{self, Language};
use crate::ui::style::tokens::Layout;
use crate::ui::workbench::field::{self, Field};
use std::path::PathBuf;

use eframe::egui;
use http_inspector_interop::har::HarOptions;

/// Which records an export contains.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum HarScope {
    /// Every record kept in the request list.
    #[default]
    All,
    /// Only the records that match the current filter and search.
    Filtered,
}

/// An export the user confirmed.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HarExport {
    pub(crate) scope: HarScope,
    pub(crate) options: HarOptions,
    pub(crate) path: PathBuf,
}

pub(crate) enum HarAction {
    Close,
    Export(HarExport),
}

/// Numbers of records each scope would export.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScopeCounts {
    pub(crate) all: usize,
    pub(crate) filtered: usize,
}

impl ScopeCounts {
    const fn of(self, scope: HarScope) -> usize {
        match scope {
            HarScope::All => self.all,
            HarScope::Filtered => self.filtered,
        }
    }
}

/// Window state. Options reset to the safe default each time it opens.
#[derive(Default)]
pub(crate) struct HarDialog {
    open: bool,
    path: String,
    scope: HarScope,
    options: HarOptions,
}

impl HarDialog {
    /// Opens the window with `path` as the suggested file and default options.
    pub(crate) fn open(&mut self, path: String) {
        *self = Self {
            open: true,
            path,
            ..Self::default()
        };
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) const fn close(&mut self) {
        self.open = false;
    }

    /// Shows the form and reports what the user chose.
    pub(crate) fn show(
        &mut self,
        context: &egui::Context,
        counts: ScopeCounts,
        busy: bool,
        escape_enabled: bool,
        lang: Language,
    ) -> Option<HarAction> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        let viewport = context.content_rect();
        let center = viewport.center();
        let max_width = Layout::HAR_WIDTH.fit(viewport.width());
        let max_height = Layout::HAR_HEIGHT.fit(viewport.height());
        egui::Window::new(lang.har().window_title)
            .id(egui::Id::unique("export-har"))
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .max_width(max_width)
            .max_height(max_height)
            .vscroll(true)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| action = self.contents(ui, counts, busy, lang));
        let escaped = escape_enabled && context.input(|input| input.key_pressed(egui::Key::Escape));
        action.or_else(|| (!open || escaped).then_some(HarAction::Close))
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        counts: ScopeCounts,
        busy: bool,
        lang: Language,
    ) -> Option<HarAction> {
        let text = lang.har();
        ui.label(text.records);
        let all_label = i18n::dialogs::har::scope_all(lang, counts.all);
        let filtered_label = i18n::dialogs::har::scope_filtered(lang, counts.filtered);
        ui.radio_value(&mut self.scope, HarScope::All, all_label);
        ui.radio_value(&mut self.scope, HarScope::Filtered, filtered_label);
        ui.weak(text.scope_note);
        ui.separator();
        ui.label(text.include_heading);
        ui.checkbox(&mut self.options.include_bodies, text.include_bodies)
            .on_hover_text(text.tooltips.include_bodies);
        ui.checkbox(
            &mut self.options.include_sensitive_headers,
            text.include_sensitive_headers,
        )
        .on_hover_text(text.tooltips.include_sensitive_headers);
        ui.separator();
        ui.label(text.file_label);
        let path_width = Layout::HAR_PATH_WIDTH.fit(ui.available_width());
        let _ = field::single_line(
            ui,
            &mut self.path,
            Field::new(path_width).hint(text.path_hint).monospace(),
            |_| {},
        )
        .on_hover_text(text.tooltips.path_no_overwrite);
        let count = counts.of(self.scope);
        let ready = count > 0 && !self.path.trim().is_empty() && !busy;
        let export_label = i18n::dialogs::har::export_button(lang, count);
        ui.horizontal_wrapped(|ui| {
            let export = ui
                .add_enabled(ready, egui::Button::new(export_label))
                .clicked();
            let cancel = ui.button(lang.shared().cancel).clicked();
            if export {
                Some(HarAction::Export(HarExport {
                    scope: self.scope,
                    options: self.options,
                    path: PathBuf::from(self.path.trim()),
                }))
            } else {
                cancel.then_some(HarAction::Close)
            }
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_reports_close_without_changing_the_window_state() {
        let mut dialog = HarDialog::default();
        dialog.open("fixture.har".to_owned());
        let context = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..egui::RawInput::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let action = dialog.show(
                ui.ctx(),
                ScopeCounts {
                    all: 1,
                    filtered: 1,
                },
                false,
                true,
                Language::English,
            );
            assert!(matches!(action, Some(HarAction::Close)));
            assert!(dialog.is_open());
        });
        output.textures_delta.clear();
        dialog.close();
        assert!(!dialog.is_open());
    }

    #[test]
    fn reopening_restores_the_safe_defaults() {
        let mut dialog = HarDialog::default();
        dialog.open("/tmp/first.har".into());
        dialog.options.include_bodies = true;
        dialog.options.include_sensitive_headers = true;
        dialog.scope = HarScope::Filtered;
        dialog.open("/tmp/second.har".into());
        assert!(dialog.open);
        assert_eq!(dialog.path, "/tmp/second.har");
        assert_eq!(dialog.scope, HarScope::All);
        assert_eq!(dialog.options, HarOptions::default());
    }
}
