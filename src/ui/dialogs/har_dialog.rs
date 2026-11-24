//! The `Export HAR` window: which records, what to include, and where to save.

use std::path::PathBuf;

use eframe::egui;
use http_proxy::interop::har::HarOptions;

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

    /// Shows the window while it is open and returns the confirmed export.
    pub(crate) fn show(
        &mut self,
        context: &egui::Context,
        counts: ScopeCounts,
        busy: bool,
    ) -> Option<HarExport> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut confirmed = None;
        let viewport = context.content_rect();
        let center = viewport.center();
        let max_width = (viewport.width() - 32.0).clamp(240.0, 460.0);
        let max_height = (viewport.height() - 48.0).max(200.0);
        egui::Window::new("Export HAR")
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .max_width(max_width)
            .max_height(max_height)
            .vscroll(true)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| confirmed = self.contents(ui, counts, busy));
        let escaped = context.input(|input| input.key_pressed(egui::Key::Escape));
        self.open = self.open && open && confirmed.is_none() && !escaped;
        confirmed
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        counts: ScopeCounts,
        busy: bool,
    ) -> Option<HarExport> {
        ui.label("Records");
        ui.radio_value(
            &mut self.scope,
            HarScope::All,
            format!("All requests in the list ({})", counts.all),
        );
        ui.radio_value(
            &mut self.scope,
            HarScope::Filtered,
            format!("Requests matching the filter ({})", counts.filtered),
        );
        ui.weak("Only requests kept in the list since the last Clear are exported.");
        ui.separator();
        ui.label("Include (off by default)");
        ui.checkbox(&mut self.options.include_bodies, "Request and response bodies")
            .on_hover_text(
                "Uses the in-memory preview: 8 KiB per direction, up to 8 MiB for images. A HAR comment marks truncated or partial bodies. Bodies may contain passwords or tokens.",
            );
        ui.checkbox(
            &mut self.options.include_sensitive_headers,
            "Authorization and cookie header names",
        )
        .on_hover_text(
            "Adds Authorization, Proxy-Authorization, Cookie, and Set-Cookie with the value [REDACTED]. Their original values are never kept.",
        );
        ui.separator();
        ui.label("File");
        let path_width = ui.available_width().clamp(180.0, 420.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.path)
                .desired_width(path_width)
                .hint_text("Path of a new .har file"),
        );
        ui.weak("An existing file is never replaced.");
        let count = counts.of(self.scope);
        let ready = count > 0 && !self.path.trim().is_empty() && !busy;
        let mut confirmed = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new(format!("Export {count} requests")))
                .clicked()
            {
                confirmed = Some(HarExport {
                    scope: self.scope,
                    options: self.options,
                    path: PathBuf::from(self.path.trim()),
                });
            }
            if ui.button("Cancel").clicked() {
                self.open = false;
            }
        });
        confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
